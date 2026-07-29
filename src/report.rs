//! 面向人和脚本的稳定报告及只读清理计划。

use std::fs::File;
use std::path::Path;

use anyhow::{Context, Result};
use chrono::Utc;
use serde::Serialize;

use crate::db::Database;
use crate::duplicate::{DuplicateFilter, duplicate_groups};
use crate::model::{CleanupPlan, CleanupPlanItem, DuplicateGroup, LookupResult};
use crate::util::{human_size, now};

pub const JSON_SCHEMA_VERSION: u32 = 1;

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
            "内容重复不等于副本多余；请按物理介质、异地备份和恢复需求评估。".to_owned(),
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
        output.push_str(&format!("Online copies: {}\n", group.online_copies));
        output.push_str(&format!("Offline copies: {}\n", group.offline_copies));
        output.push_str(&format!(
            "Theoretical reclaimable (keep one): {}\n\n",
            human_size(group.theoretical_reclaimable_bytes)
        ));
        for copy in &group.copies {
            let label = if copy.status == "offline" {
                "OFFLINE"
            } else if copy.status == "missing" {
                "MISSING"
            } else if copy.role == "primary" {
                "KEEP CANDIDATE"
            } else {
                "REDUNDANT CANDIDATE"
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
        "volume_id",
        "volume",
        "role",
        "path",
        "status",
        "is_online",
    ])?;
    for group in groups {
        for copy in &group.copies {
            writer.write_record([
                group.full_hash.as_str(),
                &group.file_size.to_string(),
                &group.known_copies.to_string(),
                &copy.volume_id.to_string(),
                copy.volume.as_str(),
                copy.role.as_str(),
                copy.path.as_str(),
                copy.status.as_str(),
                &copy.is_online.to_string(),
            ])?;
        }
    }
    writer.flush()?;
    Ok(())
}

#[must_use]
pub fn render_lookup_text(result: &LookupResult) -> String {
    let mut output = format!(
        "文件: {}\n大小: {}\n抽样指纹: {}\n完整哈希: {}\n索引中曾出现: {}\n已知副本: {}\n在线副本: {}\n离线副本: {}\n",
        result.path,
        human_size(result.file_size),
        result.sample_hash.as_deref().unwrap_or("未计算"),
        result.full_hash.as_deref().unwrap_or("未计算"),
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
    if min_remaining_copies == 0 {
        anyhow::bail!("--min-remaining-copies 必须至少为 1");
    }
    database.volume_by_id(target_volume_id)?;
    database.volume_by_id(keep_volume_id)?;
    let groups = duplicate_groups(
        database,
        DuplicateFilter {
            include_missing: true,
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
                .count();
            let mut blocked_reasons = Vec::new();
            if !candidate_delete.is_online || candidate_delete.status != "present" {
                blocked_reasons.push("目标副本不是当前在线且存在的文件。".to_owned());
            }
            if keep_candidates.is_empty() {
                blocked_reasons.push("指定保留卷没有在线、可验证的完整哈希副本。".to_owned());
            }
            if remaining_online < min_remaining_copies {
                blocked_reasons.push(format!(
                    "移除此副本后仅剩 {remaining_online} 个在线副本，少于要求的 {min_remaining_copies} 个。"
                ));
            }
            items.push(CleanupPlanItem {
                full_hash: group.full_hash.clone(),
                file_size: group.file_size,
                candidate_delete,
                keep_candidates,
                all_known_copies: all_known_copies.clone(),
                generated_at: generated_at.clone(),
                database_schema_version: schema_version,
                status: if blocked_reasons.is_empty() {
                    "candidate_only".to_owned()
                } else {
                    "blocked".to_owned()
                },
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
        warnings: vec![
            "计划只依据当前数据库和在线状态；生成后应在人工操作前再次验证文件。".to_owned(),
            "该工具没有也不会在本阶段提供永久删除或移动命令。".to_owned(),
        ],
        items,
    })
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
        .filter(|item| item.status == "candidate_only")
        .count();
    let blocked = plan.items.len().saturating_sub(candidates);
    format!(
        "清理计划已生成：{} 个仅供人工审核的候选项，{} 个已阻止。未对任何原始文件执行操作。生成时间: {}（UTC）\n",
        candidates,
        blocked,
        Utc::now().to_rfc3339()
    )
}
