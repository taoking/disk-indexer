//! 面向人和脚本的稳定报告及只读清理计划。

use std::fs::File;
use std::path::Path;

use anyhow::{Context, Result};
use chrono::Utc;
use serde::Serialize;
use std::collections::HashSet;
use std::sync::Arc;

use crate::config::Config;
use crate::db::Database;
use crate::duplicate::{DuplicateFilter, duplicate_groups, verify_record};
use crate::model::{
    CleanupPlan, CleanupPlanItem, DuplicateGroup, LookupResult, PhysicalDeviceIdentityState,
};
use crate::util::{human_size, now};

pub const JSON_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy)]
pub struct CleanupVerificationOptions {
    pub min_remaining_physical_devices: usize,
    pub verify_metadata: bool,
    pub verify_full_hash: bool,
}

pub type CleanupProgressCallback = Arc<dyn Fn(&CleanupPlanItem, usize, usize) + Send + Sync>;

impl Default for CleanupVerificationOptions {
    fn default() -> Self {
        Self {
            min_remaining_physical_devices: 1,
            verify_metadata: false,
            verify_full_hash: false,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct DuplicateReport<'a> {
    pub schema_version: u32,
    pub generated_at: String,
    pub database_path: String,
    pub groups: &'a [DuplicateGroup],
    pub warnings: Vec<String>,
}

#[must_use]
pub fn duplicate_report<'a>(
    database: &Database,
    groups: &'a [DuplicateGroup],
) -> DuplicateReport<'a> {
    DuplicateReport {
        schema_version: JSON_SCHEMA_VERSION,
        generated_at: now(),
        database_path: database.path().display().to_string(),
        groups,
        warnings: vec![
            "内容重复不等于副本多余；物理设备统计只计入已验证的整盘身份。".to_owned(),
            "理论可释放空间按每组保留一份计算，不是安全删除建议。".to_owned(),
        ],
    }
}

#[must_use]
pub fn render_duplicates_text(groups: &[DuplicateGroup]) -> String {
    if groups.is_empty() {
        return "未找到具有可信完整 BLAKE3 哈希的重复内容组。\n".to_owned();
    }
    let mut output = String::new();
    output.push_str("注意：内容重复不等于副本多余；理论可释放空间不是删除建议。\n\n");
    for group in groups {
        output.push_str("Duplicate Group\n");
        output.push_str(&format!("Hash: {}\n", group.full_hash));
        output.push_str(&format!("Size: {}\n", human_size(group.file_size)));
        output.push_str(&format!("Known copies: {}\n", group.known_copies));
        output.push_str(&format!("Paths: {}\n", group.path_count));
        output.push_str(&format!(
            "Storage objects: {}\n",
            group.storage_object_count
        ));
        output.push_str(&format!(
            "Logical volumes: {}\n",
            group.logical_volume_count
        ));
        output.push_str(&format!(
            "Verified physical devices: {}\n",
            group.verified_physical_device_count
        ));
        output.push_str(&format!(
            "Unknown or unverified device identities: {}\n",
            group.unknown_physical_device_count
        ));
        output.push_str(&format!("Online copies: {}\n", group.online_copies));
        output.push_str(&format!("Offline copies: {}\n", group.offline_copies));
        output.push_str(&format!(
            "Theoretical reclaimable (keep one): {}\n\n",
            human_size(group.theoretical_reclaimable_bytes)
        ));
        for copy in &group.copies {
            let label = if copy.status == "offline_unverified" {
                "OFFLINE_UNVERIFIED".to_owned()
            } else if copy.status == "missing" {
                "MISSING".to_owned()
            } else if copy.status != "present" {
                copy.status.to_ascii_uppercase()
            } else if copy.is_online && copy.role == "primary" {
                "KEEP_CANDIDATE".to_owned()
            } else if copy.is_online {
                "REDUNDANT_CANDIDATE".to_owned()
            } else {
                "OFFLINE_UNVERIFIED".to_owned()
            };
            output.push_str(&format!(
                "[{label}]\nVolume: {}\nRole: {}\nPath: {}\nStatus: {}\n\n",
                copy.volume, copy.role, copy.path, copy.status
            ));
        }
    }
    output
}

pub fn write_csv(path: &Path, groups: &[DuplicateGroup]) -> Result<()> {
    let file =
        File::create(path).with_context(|| format!("无法创建 CSV 报告 {}", path.display()))?;
    let mut writer = csv::Writer::from_writer(file);
    writer.write_record([
        "full_hash",
        "file_size",
        "known_copies",
        "path_count",
        "storage_object_count",
        "logical_volume_count",
        "physical_device_count",
        "verified_physical_device_count",
        "unknown_physical_device_count",
        "volume_id",
        "volume",
        "role",
        "path",
        "status",
        "is_online",
        "physical_device_id",
        "storage_object_key",
    ])?;
    for group in groups {
        for copy in &group.copies {
            writer.write_record([
                group.full_hash.as_str(),
                &group.file_size.to_string(),
                &group.known_copies.to_string(),
                &group.path_count.to_string(),
                &group.storage_object_count.to_string(),
                &group.logical_volume_count.to_string(),
                &group.physical_device_count.to_string(),
                &group.verified_physical_device_count.to_string(),
                &group.unknown_physical_device_count.to_string(),
                &copy.volume_id.to_string(),
                copy.volume.as_str(),
                copy.role.as_str(),
                copy.path.as_str(),
                copy.status.as_str(),
                &copy.is_online.to_string(),
                &copy
                    .physical_device_id
                    .map_or_else(String::new, |id| id.to_string()),
                copy.storage_object_key.as_deref().unwrap_or(""),
            ])?;
        }
    }
    writer.flush()?;
    Ok(())
}

#[must_use]
pub fn render_lookup_text(result: &LookupResult) -> String {
    let mut output = format!(
        "文件: {}\n大小: {}\n抽样指纹: {}\n完整哈希: {}\n精确匹配: {}\n缓存状态: {}\n索引元数据匹配: {}\n需要重新计算: {}\n索引中曾出现: {}\n已知副本: {}\n在线副本: {}\n离线副本: {}\n",
        result.path,
        human_size(result.file_size),
        result.sample_hash.as_deref().unwrap_or("未计算"),
        result.full_hash.as_deref().unwrap_or("未计算"),
        if result.exact { "是" } else { "否" },
        result.cache_state,
        if result.metadata_matches_index {
            "是"
        } else {
            "否"
        },
        if result.requires_rehash { "是" } else { "否" },
        if result.has_appeared_before {
            "是"
        } else {
            "否或无法精确确认"
        },
        result.known_copies,
        result.online_copies,
        result.offline_copies
    );
    for copy in &result.copies {
        output.push_str(&format!(
            "- {}:{} [{}] {}\n",
            copy.volume, copy.path, copy.status, copy.role
        ));
    }
    for warning in &result.warnings {
        output.push_str(&format!("警告: {warning}\n"));
    }
    output
}

pub fn create_cleanup_plan(
    database: &Database,
    target_volume_id: i64,
    keep_volume_id: i64,
    min_remaining_copies: usize,
) -> Result<CleanupPlan> {
    create_cleanup_plan_with_options(
        database,
        target_volume_id,
        keep_volume_id,
        min_remaining_copies,
        CleanupVerificationOptions::default(),
    )
}

pub fn create_cleanup_plan_with_options(
    database: &Database,
    target_volume_id: i64,
    keep_volume_id: i64,
    min_remaining_copies: usize,
    options: CleanupVerificationOptions,
) -> Result<CleanupPlan> {
    if min_remaining_copies == 0 {
        anyhow::bail!("--min-remaining-copies 必须至少为 1");
    }
    if options.min_remaining_physical_devices == 0 {
        anyhow::bail!("--min-remaining-physical-devices 必须至少为 1");
    }
    database.volume_by_id(target_volume_id)?;
    database.volume_by_id(keep_volume_id)?;
    let groups = duplicate_groups(
        database,
        DuplicateFilter {
            ..DuplicateFilter::default()
        },
    )?;
    let schema_version = database.schema_version()?;
    let generated_at = now();
    let mut items = Vec::new();
    for group in groups {
        let all_known_copies = group.copies;
        for candidate_delete in all_known_copies
            .iter()
            .filter(|copy| copy.volume_id == target_volume_id)
            .cloned()
        {
            let keep_candidates = all_known_copies
                .iter()
                .filter(|copy| {
                    copy.volume_id == keep_volume_id && copy.is_online && copy.status == "present"
                })
                .cloned()
                .collect::<Vec<_>>();
            let remaining_online = all_known_copies
                .iter()
                .filter(|copy| {
                    copy.file_copy_id != candidate_delete.file_copy_id
                        && copy.is_online
                        && copy.status == "present"
                })
                .collect::<Vec<_>>();
            let remaining_storage_objects = remaining_online
                .iter()
                .map(|copy| storage_object_identity(copy))
                .collect::<HashSet<_>>();
            let remaining_volumes = remaining_online
                .iter()
                .map(|copy| copy.volume_id)
                .collect::<HashSet<_>>();
            let remaining_physical_devices = remaining_online
                .iter()
                .filter(|copy| {
                    copy.physical_identity_state == PhysicalDeviceIdentityState::Verified
                })
                .filter_map(|copy| copy.physical_device_id)
                .collect::<HashSet<_>>();
            let candidate_storage_object = storage_object_identity(&candidate_delete);
            let mut blocked_reasons = Vec::new();
            if !candidate_delete.is_online || candidate_delete.status != "present" {
                blocked_reasons.push("目标副本不是当前在线且存在的文件。".to_owned());
            }
            if keep_candidates.is_empty() {
                blocked_reasons.push("指定保留卷没有在线、可验证的完整哈希副本。".to_owned());
            }
            if remaining_storage_objects.len() < min_remaining_copies {
                blocked_reasons.push(format!(
                    "移除此路径后仅剩 {} 个在线独立存储对象，少于要求的 {min_remaining_copies} 个。",
                    remaining_storage_objects.len()
                ));
            }
            if remaining_physical_devices.len() < options.min_remaining_physical_devices {
                blocked_reasons.push(format!(
                    "移除此路径后仅剩 {} 个已验证物理设备，少于要求的 {} 个；未知或推断身份不计入安全阈值。",
                    remaining_physical_devices.len(),
                    options.min_remaining_physical_devices
                ));
            }
            if remaining_storage_objects.contains(&candidate_storage_object) {
                blocked_reasons.push(
                    "目标路径与同一存储对象的另一硬链接共享数据；删除该路径不会释放完整文件空间。"
                        .to_owned(),
                );
            }
            items.push(CleanupPlanItem {
                full_hash: group.full_hash.clone(),
                file_size: group.file_size,
                candidate_delete,
                keep_candidates,
                all_known_copies: all_known_copies.clone(),
                all_remaining_candidates: remaining_online.iter().map(|copy| (*copy).clone()).collect(),
                verified_remaining_copies: Vec::new(),
                failed_remaining_copies: Vec::new(),
                unknown_identity_copies: remaining_online
                    .iter()
                    .filter(|copy| {
                        copy.physical_identity_state != PhysicalDeviceIdentityState::Verified
                    })
                    .map(|copy| (*copy).clone())
                    .collect(),
                generated_at: generated_at.clone(),
                database_schema_version: schema_version,
                status: if blocked_reasons.is_empty() {
                    "candidate_unverified".to_owned()
                } else {
                    "blocked".to_owned()
                },
                verification_mode: "none".to_owned(),
                verified_at: None,
                remaining_path_copies: remaining_online.len(),
                remaining_storage_objects: remaining_storage_objects.len(),
                remaining_volumes: remaining_volumes.len(),
                remaining_physical_devices: remaining_physical_devices.len(),
                verified_remaining_path_copies: 0,
                verified_remaining_storage_objects: 0,
                verified_remaining_logical_volumes: 0,
                verified_remaining_physical_devices: 0,
                candidate_metadata_state: "unverified".to_owned(),
                keeper_metadata_state: "unverified".to_owned(),
                verification_failures: Vec::new(),
                physical_identity_warnings: Vec::new(),
                blocked_reasons,
                risk_notice: "本计划只提供人工审核候选项，工具不会删除、移动或修改原始文件。内容重复不等于备份冗余。".to_owned(),
            });
        }
    }
    Ok(CleanupPlan {
        schema_version,
        generated_at,
        database_path: database.path().display().to_string(),
        target_volume_id,
        keep_volume_id,
        min_remaining_copies,
        min_remaining_physical_devices: options.min_remaining_physical_devices,
        verification_mode: "none".to_owned(),
        verification_protocol_version: 1,
        verified_at: None,
        cancelled: false,
        completed_items: 0,
        blocked_items: 0,
        verified_items: 0,
        warnings: vec![
            "计划只依据当前数据库和在线状态；生成后应在人工操作前再次验证文件。".to_owned(),
            "该工具没有也不会在本阶段提供永久删除或移动命令。".to_owned(),
        ],
        items,
    })
}

pub fn create_cleanup_plan_with_verification(
    database: &mut Database,
    config: &Config,
    target_volume_id: i64,
    keep_volume_id: i64,
    min_remaining_copies: usize,
    options: CleanupVerificationOptions,
) -> Result<CleanupPlan> {
    create_cleanup_plan_with_verification_and_progress(
        database,
        config,
        target_volume_id,
        keep_volume_id,
        min_remaining_copies,
        options,
        None,
    )
}

pub fn create_cleanup_plan_with_verification_and_progress(
    database: &mut Database,
    config: &Config,
    target_volume_id: i64,
    keep_volume_id: i64,
    min_remaining_copies: usize,
    options: CleanupVerificationOptions,
    progress_callback: Option<&CleanupProgressCallback>,
) -> Result<CleanupPlan> {
    // 调用方即使遗漏刷新在线状态，严格模式也绝不能把已卸载卷计入剩余副本。
    database.refresh_volume_online_states()?;
    let mut plan = create_cleanup_plan_with_options(
        database,
        target_volume_id,
        keep_volume_id,
        min_remaining_copies,
        options,
    )?;
    if !options.verify_metadata && !options.verify_full_hash {
        return Ok(plan);
    }
    let verification_mode = if options.verify_full_hash {
        "full_hash"
    } else {
        "metadata"
    };
    let verified_at = now();
    let mut verification_cache = std::collections::HashMap::new();
    let total_items = plan.items.len();
    for (index, item) in plan.items.iter_mut().enumerate() {
        item.verification_mode = verification_mode.to_owned();
        item.verified_at = Some(verified_at.clone());
        let candidate_result = verify_copy_once(
            database,
            config,
            item.candidate_delete.file_copy_id,
            options.verify_full_hash,
            &mut verification_cache,
        );
        item.candidate_metadata_state = if !candidate_result.verified {
            item.blocked_reasons.push(format!(
                "候选副本验证失败: {}",
                candidate_result.failure_message()
            ));
            item.verification_failures.push(format!(
                "候选副本 {}: {}",
                item.candidate_delete.path,
                candidate_result.failure_message()
            ));
            "failed".to_owned()
        } else {
            "verified".to_owned()
        };

        // 每个独立 storage object 只验证一个代表路径；只有这些实际验证成功的代表才能
        // 进入后续路径、存储对象、卷和物理设备安全计数。
        let mut representatives = std::collections::BTreeMap::new();
        for copy in &item.all_remaining_candidates {
            representatives
                .entry(storage_object_identity(copy))
                .or_insert_with(|| copy.clone());
        }
        let mut representative_results = std::collections::HashMap::new();
        for (storage_key, representative) in &representatives {
            let result = verify_copy_once(
                database,
                config,
                representative.file_copy_id,
                options.verify_full_hash,
                &mut verification_cache,
            );
            representative_results.insert(storage_key.clone(), result);
        }
        item.verified_remaining_copies = representatives
            .iter()
            .filter_map(|(storage_key, copy)| {
                representative_results
                    .get(storage_key)
                    .filter(|result| result.verified)
                    .map(|_| copy.clone())
            })
            .collect();
        item.failed_remaining_copies = representatives
            .iter()
            .filter_map(|(storage_key, copy)| {
                representative_results
                    .get(storage_key)
                    .filter(|result| !result.verified)
                    .map(|result| {
                        item.verification_failures.push(format!(
                            "剩余副本 {}: {}",
                            copy.path,
                            result.failure_message()
                        ));
                        copy.clone()
                    })
            })
            .collect();
        for copy in &item.unknown_identity_copies {
            item.physical_identity_warnings.push(format!(
                "{} 的物理设备身份为 {}，不计入安全物理设备阈值。",
                copy.path,
                copy.physical_identity_state.as_str()
            ));
        }
        for copy in item
            .all_known_copies
            .iter()
            .filter(|copy| copy.physical_identity_state == PhysicalDeviceIdentityState::Conflict)
        {
            item.blocked_reasons.push(format!(
                "{} 的整盘物理设备身份存在冲突，不能作为清理计划依据。",
                copy.path
            ));
        }

        let verified_storage_objects = item
            .verified_remaining_copies
            .iter()
            .map(storage_object_identity)
            .collect::<HashSet<_>>();
        let verified_volumes = item
            .verified_remaining_copies
            .iter()
            .map(|copy| copy.volume_id)
            .collect::<HashSet<_>>();
        let verified_physical_devices = item
            .verified_remaining_copies
            .iter()
            .filter(|copy| copy.physical_identity_state == PhysicalDeviceIdentityState::Verified)
            .filter_map(|copy| copy.physical_device_id)
            .collect::<HashSet<_>>();
        item.verified_remaining_path_copies = item.verified_remaining_copies.len();
        item.verified_remaining_storage_objects = verified_storage_objects.len();
        item.verified_remaining_logical_volumes = verified_volumes.len();
        item.verified_remaining_physical_devices = verified_physical_devices.len();

        let verified_keeper = item.keep_candidates.iter().any(|keeper| {
            representative_results
                .get(&storage_object_identity(keeper))
                .is_some_and(|result| result.verified)
        });
        item.keeper_metadata_state = if verified_keeper {
            "verified".to_owned()
        } else {
            item.blocked_reasons
                .push("指定保留卷没有通过验证的独立存储对象。".to_owned());
            "failed".to_owned()
        };
        if item.verified_remaining_storage_objects < min_remaining_copies {
            item.blocked_reasons.push(format!(
                "验证后仅剩 {} 个独立存储对象，少于要求的 {min_remaining_copies} 个。",
                item.verified_remaining_storage_objects
            ));
        }
        if item.verified_remaining_physical_devices < options.min_remaining_physical_devices {
            item.blocked_reasons.push(format!(
                "验证后仅剩 {} 个已验证物理设备，少于要求的 {} 个。",
                item.verified_remaining_physical_devices, options.min_remaining_physical_devices
            ));
        }
        item.status = if item.blocked_reasons.is_empty() {
            "verified_candidate".to_owned()
        } else if item.candidate_metadata_state == "failed"
            && item
                .verification_failures
                .iter()
                .any(|failure| failure_is_stale(failure))
        {
            "stale".to_owned()
        } else {
            "blocked".to_owned()
        };
        if let Some(callback) = progress_callback {
            callback(item, index.saturating_add(1), total_items);
        }
    }
    plan.verification_mode = verification_mode.to_owned();
    plan.verified_at = Some(verified_at);
    plan.completed_items = plan.items.len();
    plan.blocked_items = plan
        .items
        .iter()
        .filter(|item| item.status == "blocked" || item.status == "stale")
        .count();
    plan.verified_items = plan
        .items
        .iter()
        .filter(|item| item.status == "verified_candidate")
        .count();
    Ok(plan)
}

#[derive(Clone)]
struct CopyVerificationResult {
    verified: bool,
    failure: Option<String>,
}

impl CopyVerificationResult {
    fn failure_message(&self) -> &str {
        self.failure.as_deref().unwrap_or("未知验证错误")
    }
}

fn verify_copy_once(
    database: &mut Database,
    config: &Config,
    file_copy_id: i64,
    verify_full_hash: bool,
    cache: &mut std::collections::HashMap<i64, CopyVerificationResult>,
) -> CopyVerificationResult {
    if let Some(result) = cache.get(&file_copy_id) {
        return result.clone();
    }
    let result = (|| {
        let record = database
            .file_record_by_id(file_copy_id)?
            .context("清理候选记录不存在")?;
        verify_record(database, config, &record, verify_full_hash)
    })();
    let result = match result {
        Ok(()) => CopyVerificationResult {
            verified: true,
            failure: None,
        },
        Err(error) => CopyVerificationResult {
            verified: false,
            failure: Some(format!("{error:#}")),
        },
    };
    cache.insert(file_copy_id, result.clone());
    result
}

fn failure_is_stale(failure: &str) -> bool {
    ["文件元数据已改变", "完整哈希不匹配", "验证期间文件发生变化"]
        .iter()
        .any(|marker| failure.contains(marker))
}

pub fn write_cleanup_plan(path: &Path, plan: &CleanupPlan) -> Result<()> {
    let file =
        File::create(path).with_context(|| format!("无法创建清理计划 {}", path.display()))?;
    serde_json::to_writer_pretty(file, plan).context("无法序列化清理计划")?;
    Ok(())
}

#[must_use]
pub fn render_cleanup_plan_text(plan: &CleanupPlan) -> String {
    let candidates = plan
        .items
        .iter()
        .filter(|item| item.status == "candidate_unverified" || item.status == "verified_candidate")
        .count();
    let blocked = plan.items.len().saturating_sub(candidates);
    format!(
        "清理计划已生成：{} 个仅供人工审核的候选项，{} 个已阻止。未对任何原始文件执行操作。生成时间: {}（UTC）\n",
        candidates,
        blocked,
        Utc::now().to_rfc3339()
    )
}

fn storage_object_identity(copy: &crate::model::CopyView) -> String {
    copy.storage_object_key
        .clone()
        .unwrap_or_else(|| format!("path:{}", copy.file_copy_id))
}
