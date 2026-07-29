//! 重复内容组、查询与只读验证服务。

use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result};

use crate::config::Config;
use crate::db::Database;
use crate::hashing::{full_hash, sample_hash};
use crate::model::{CopyView, DuplicateGroup, FileRecord, LookupResult};
use crate::util::display_path;
use crate::volume::{file_metadata, metadata_still_matches};

#[derive(Debug, Clone, Copy)]
pub struct DuplicateFilter {
    pub min_size: u64,
    pub min_copies: usize,
    pub online_only: bool,
    pub include_missing: bool,
    pub volume_id: Option<i64>,
}

impl Default for DuplicateFilter {
    fn default() -> Self {
        Self {
            min_size: 0,
            min_copies: 2,
            online_only: false,
            include_missing: false,
            volume_id: None,
        }
    }
}

pub fn duplicate_groups(
    database: &Database,
    filter: DuplicateFilter,
) -> Result<Vec<DuplicateGroup>> {
    let mut groups = Vec::new();
    let mut after_content_id = 0;
    loop {
        let content_page =
            database.duplicate_content_ids_page(after_content_id, 2, filter.min_size, 1_000)?;
        let Some(last) = content_page.last() else {
            break;
        };
        after_content_id = last.0;
        for (content_id, full_hash, file_size) in content_page {
            let records = database.records_by_content(content_id)?;
            let copies = records
                .iter()
                .filter(|record| include_record(record, filter))
                .map(copy_view)
                .collect::<Vec<_>>();
            if copies.len() < filter.min_copies {
                continue;
            }
            let online_storage_objects = copies
                .iter()
                .filter(|copy| copy.is_online && copy.status == "present")
                .map(storage_object_identity)
                .collect::<HashSet<_>>();
            let storage_objects = copies
                .iter()
                .map(storage_object_identity)
                .collect::<HashSet<_>>();
            let logical_volumes = copies
                .iter()
                .map(|copy| copy.volume_id)
                .collect::<HashSet<_>>();
            let physical_devices = copies
                .iter()
                .map(|copy| copy.physical_device_id.unwrap_or(-copy.volume_id))
                .collect::<HashSet<_>>();
            let offline_copies = copies
                .iter()
                .filter(|copy| copy.status == "offline_unverified")
                .count();
            let missing_copies = copies
                .iter()
                .filter(|copy| copy.status == "missing")
                .count();
            groups.push(DuplicateGroup {
                full_hash,
                file_size,
                known_copies: copies.len(),
                online_copies: online_storage_objects.len(),
                offline_copies,
                missing_copies,
                path_count: copies.len(),
                storage_object_count: storage_objects.len(),
                logical_volume_count: logical_volumes.len(),
                physical_device_count: physical_devices.len(),
                theoretical_reclaimable_bytes: file_size.saturating_mul(
                    u64::try_from(online_storage_objects.len().saturating_sub(1))
                        .unwrap_or(u64::MAX),
                ),
                copies,
            });
        }
    }
    groups.sort_by(|left, right| {
        right
            .theoretical_reclaimable_bytes
            .cmp(&left.theoretical_reclaimable_bytes)
            .then_with(|| right.file_size.cmp(&left.file_size))
            .then_with(|| left.full_hash.cmp(&right.full_hash))
    });
    Ok(groups)
}

pub fn lookup(
    database: &Database,
    config: &Config,
    path: &Path,
    use_full_hash: bool,
) -> Result<LookupResult> {
    let path = path
        .canonicalize()
        .with_context(|| format!("无法访问待查询文件 {}", path.display()))?;
    let metadata = std::fs::metadata(&path)
        .with_context(|| format!("无法读取待查询文件 {}", path.display()))?;
    if !metadata.is_file() {
        anyhow::bail!("lookup 只接受普通文件: {}", path.display());
    }
    let file_size = metadata.len();
    let volume = database.find_volume_for_path(&path)?;
    let direct = volume.as_ref().and_then(|volume| {
        path.strip_prefix(&volume.mount_path)
            .ok()
            .and_then(|relative| {
                database
                    .file_record_by_path(volume.id, &crate::util::path_bytes(relative))
                    .ok()
                    .flatten()
            })
    });
    let mut warnings = Vec::new();
    let direct_metadata_matches = direct.as_ref().is_some_and(|record| {
        file_metadata(
            &volume
                .as_ref()
                .expect("direct record only exists on a registered volume")
                .mount_path,
            &path,
        )
        .is_ok_and(|actual| metadata_matches_index(record, &actual))
    });
    let (sample, full, records, exact, cache_state, requires_rehash) = if use_full_hash {
        let hash = full_hash(&path, config.read_buffer_bytes)?;
        let records = database.records_by_full_hash(&hash.hex, file_size)?;
        (
            None,
            Some(hash.hex),
            records,
            true,
            if direct.is_some() && !direct_metadata_matches {
                "cache_stale"
            } else if direct.is_some() {
                "cache_valid"
            } else {
                "cache_unverified"
            }
            .to_owned(),
            false,
        )
    } else if let Some(record) = &direct {
        if !direct_metadata_matches {
            warnings.push(
                "索引记录的大小、修改时间、inode 或设备号已过期；未复用旧完整哈希。请传入 --full-hash 重新计算。"
                    .to_owned(),
            );
            (
                None,
                None,
                vec![record.clone()],
                false,
                "cache_stale".to_owned(),
                true,
            )
        } else if let Some(full) = &record.full_hash {
            let records = database.records_by_full_hash(full, file_size)?;
            (
                record.sample_hash.clone(),
                Some(full.clone()),
                records,
                true,
                "cache_valid".to_owned(),
                false,
            )
        } else {
            let sample = sample_hash(&path, config.sample_bytes, config.read_buffer_bytes)?.hex;
            let records = database.records_by_sample_hash(&sample, file_size)?;
            warnings.push(
                "当前仅有抽样指纹；它不能证明内容完全相同。传入 --full-hash 以得到精确结论。"
                    .to_owned(),
            );
            (
                Some(sample),
                None,
                records,
                false,
                "cache_unverified".to_owned(),
                true,
            )
        }
    } else {
        let sample = sample_hash(&path, config.sample_bytes, config.read_buffer_bytes)?.hex;
        let records = database.records_by_sample_hash(&sample, file_size)?;
        warnings.push(
            "文件路径不在已注册卷内，且未计算完整哈希；结果仅是候选，不是重复结论。".to_owned(),
        );
        (
            Some(sample),
            None,
            records,
            false,
            "cache_unverified".to_owned(),
            true,
        )
    };
    let copies = records.iter().map(copy_view).collect::<Vec<_>>();
    let direct_seen = direct.is_some();
    let first_seen_at = records
        .iter()
        .map(|record| record.first_seen_at.clone())
        .min();
    let last_seen_at = records
        .iter()
        .map(|record| record.last_seen_at.clone())
        .max();
    Ok(LookupResult {
        path: display_path(&path),
        file_size,
        sample_hash: sample,
        full_hash: full,
        hash_state: if exact {
            "full".to_owned()
        } else if cache_state == "cache_stale" {
            "stale".to_owned()
        } else {
            "sampled".to_owned()
        },
        exact,
        cache_state,
        metadata_matches_index: direct.is_some() && direct_metadata_matches,
        requires_rehash,
        has_appeared_before: direct_seen || (exact && !records.is_empty()),
        known_copies: copies.len(),
        online_copies: copies
            .iter()
            .filter(|copy| copy.is_online && copy.status == "present")
            .count(),
        offline_copies: copies.iter().filter(|copy| !copy.is_online).count(),
        copies,
        first_seen_at,
        last_seen_at,
        warnings,
    })
}

/// 验证数据库记录与文件现状；只修改数据库审计字段，不会触碰原文件。
pub fn verify_record(
    database: &mut Database,
    config: &Config,
    record: &FileRecord,
    verify_full_hash: bool,
) -> Result<()> {
    let volume = database.volume_by_id(record.volume_id)?;
    let path = record.absolute_path(&volume.mount_path);
    let actual = match file_metadata(&volume.mount_path, &path) {
        Ok(metadata) => metadata,
        Err(error) => {
            database.mark_verified(record.id, Some(&error.to_string()))?;
            return Err(error);
        }
    };
    if actual.file_size != record.file_size
        || actual.modified_at_ns != record.modified_at_ns
        || actual.inode != record.inode
        || actual.device_id != record.device_id
    {
        let error = "文件元数据已改变";
        database.mark_verified(record.id, Some(error))?;
        anyhow::bail!("{}: {}", error, path.display());
    }
    if verify_full_hash {
        let Some(expected) = &record.full_hash else {
            anyhow::bail!(
                "记录没有可信完整哈希，不能进行完整哈希验证: {}",
                path.display()
            );
        };
        let actual_hash = full_hash(&path, config.read_buffer_bytes)?;
        if !metadata_still_matches(&path, &actual)? {
            anyhow::bail!("验证期间文件发生变化: {}", path.display());
        }
        if &actual_hash.hex != expected {
            database.mark_verified(record.id, Some("完整哈希不匹配"))?;
            anyhow::bail!("完整哈希不匹配: {}", path.display());
        }
    }
    database.mark_verified(record.id, None)?;
    Ok(())
}

fn include_record(record: &FileRecord, filter: DuplicateFilter) -> bool {
    if let Some(volume_id) = filter.volume_id {
        if record.volume_id != volume_id {
            return false;
        }
    }
    if record.status == "present" {
        return !filter.online_only || record.volume_online;
    }
    if filter.online_only {
        return false;
    }
    filter.include_missing
}

#[must_use]
pub fn copy_view(record: &FileRecord) -> CopyView {
    let status = if !record.volume_online && record.status == "present" {
        "offline_unverified".to_owned()
    } else {
        record.status.clone()
    };
    CopyView {
        file_copy_id: record.id,
        volume_id: record.volume_id,
        volume_uid: record.volume_uid.clone(),
        volume: record.volume_name.clone(),
        role: record.volume_role.as_str().to_owned(),
        path: display_path(&record.relative_path),
        status,
        is_online: record.volume_online,
        physical_device_id: record.physical_device_id,
        storage_object_key: record.storage_object_key.clone(),
        link_group_id: record.link_group_id.clone(),
        last_error: record.last_error.clone(),
    }
}

fn metadata_matches_index(record: &FileRecord, metadata: &crate::model::FileMetadata) -> bool {
    record.file_size == metadata.file_size
        && record.modified_at_ns == metadata.modified_at_ns
        && record.inode == metadata.inode
        && record.device_id == metadata.device_id
}

fn storage_object_identity(copy: &CopyView) -> String {
    copy.storage_object_key
        .clone()
        .unwrap_or_else(|| format!("path:{}", copy.file_copy_id))
}
