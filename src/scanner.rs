//! 顺序、保守的文件系统扫描器。它不跟随符号链接，也不会在卷离线时标记缺失。

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use anyhow::Result;
use serde_json::json;
use tracing::{info, warn};
use walkdir::{DirEntry, WalkDir};

use crate::config::Config;
use crate::db::{Database, MetadataOutcome};
use crate::hashing::{full_hash, sample_hash};
use crate::model::{FileMetadata, FileRecord, ScanSummary, Volume};
use crate::util::now;
use crate::volume::{MARKER_FILE, file_metadata, metadata_still_matches};

#[derive(Clone)]
pub struct ScanOptions {
    pub full_hash: bool,
    pub metadata_only: bool,
    pub resume: bool,
    pub excludes: Vec<String>,
    pub max_readers: usize,
    /// 供嵌入式调用方和测试取消扫描；CLI 使用 Ctrl+C 信号处理器。
    pub cancel_flag: Option<Arc<AtomicBool>>,
    /// 只报告已提交的扫描进度，避免把尚未持久化的数据展示为完成。
    pub progress_callback: Option<ScanProgressCallback>,
}

pub type ScanProgressCallback = Arc<dyn Fn(&ScanSummary, Option<&str>) + Send + Sync>;

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            full_hash: false,
            metadata_only: false,
            resume: false,
            excludes: Vec::new(),
            max_readers: 1,
            cancel_flag: None,
            progress_callback: None,
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct HashStats {
    pub processed: u64,
    pub skipped: u64,
    pub sampled: u64,
    pub full_hashed: u64,
    pub errors: u64,
    pub bytes_read: u64,
    pub interrupted: bool,
}

pub type HashProgressCallback = Arc<dyn Fn(&HashStats, Option<&str>) + Send + Sync>;

pub fn scan(
    database: &mut Database,
    config: &Config,
    volume: &Volume,
    options: &ScanOptions,
) -> Result<ScanSummary> {
    if options.max_readers == 0 {
        anyhow::bail!("--max-readers 必须大于零");
    }
    if options.max_readers > 1 {
        warn!(
            requested = options.max_readers,
            "当前版本为了机械硬盘安全仍按单读取器顺序扫描"
        );
    }
    let root = &volume.mount_path;
    if !root.is_dir() {
        database.set_volume_online(volume.id, false)?;
        anyhow::bail!("卷当前离线，未扫描也未标记任何文件缺失: {}", root.display());
    }
    database.set_volume_online(volume.id, true)?;
    let mode = if options.full_hash {
        "full_hash"
    } else if options.metadata_only {
        "metadata_only"
    } else if options.resume {
        "resume"
    } else {
        "incremental"
    };
    let scan_id = database.create_scan_run(volume.id, mode)?;
    let started_at = now();
    let cancelled = if let Some(flag) = &options.cancel_flag {
        Arc::clone(flag)
    } else {
        let flag = interrupt_flag()?;
        flag.store(false, Ordering::SeqCst);
        flag
    };
    let mut summary = ScanSummary {
        scan_id,
        status: "running".to_owned(),
        discovered_count: 0,
        metadata_reused_count: 0,
        sampled_count: 0,
        full_hashed_count: 0,
        skipped_count: 0,
        missing_count: 0,
        error_count: 0,
        bytes_read: 0,
    };
    let mut last_path: Option<String> = None;
    let mut iteration_error = false;
    // JSONL 使用者需要可观察的进度；有回调时最多每 100 个已提交文件刷新一次，
    // 普通 CLI 仍保留配置的较大批量写入行为。
    let persistence_batch_size = if options.progress_callback.is_some() {
        config.batch_size.clamp(1, 100)
    } else {
        config.batch_size.max(1)
    };
    let mut pending_metadata = Vec::with_capacity(persistence_batch_size);
    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            should_descend(
                root,
                entry,
                &options.excludes,
                config.database_path.as_path(),
            )
        })
    {
        if cancelled.load(Ordering::SeqCst) {
            summary.status = "interrupted".to_owned();
            break;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                warn!(error = %error, "目录遍历错误");
                summary.error_count = summary.error_count.saturating_add(1);
                iteration_error = true;
                continue;
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        last_path = Some(path.to_string_lossy().into_owned());
        match file_metadata(root, path) {
            Ok(metadata) => {
                pending_metadata.push(metadata);
                if pending_metadata.len() >= persistence_batch_size {
                    if let Err(error) = persist_metadata_batch(
                        database,
                        volume.id,
                        scan_id,
                        &mut pending_metadata,
                        &mut summary,
                    ) {
                        let dropped = pending_metadata.len();
                        warn!(path = %path.display(), error = %error, "无法批量持久化文件元数据");
                        summary.error_count = summary
                            .error_count
                            .saturating_add(u64::try_from(dropped).unwrap_or(u64::MAX));
                        pending_metadata.clear();
                        iteration_error = true;
                    } else {
                        emit_progress(options, &summary, last_path.as_deref());
                        info!(
                            path = %path.display(), files = summary.discovered_count,
                            reused = summary.metadata_reused_count, "已批量持久化扫描进度"
                        );
                    }
                }
            }
            Err(error) => {
                warn!(path = %path.display(), error = %error, "无法收录文件元数据");
                summary.error_count = summary.error_count.saturating_add(1);
                iteration_error = true;
            }
        }
    }
    if !pending_metadata.is_empty() {
        if let Err(error) = persist_metadata_batch(
            database,
            volume.id,
            scan_id,
            &mut pending_metadata,
            &mut summary,
        ) {
            let dropped = pending_metadata.len();
            warn!(error = %error, "无法提交扫描末尾的元数据批次");
            summary.error_count = summary
                .error_count
                .saturating_add(u64::try_from(dropped).unwrap_or(u64::MAX));
            iteration_error = true;
        }
        emit_progress(options, &summary, last_path.as_deref());
    }
    if summary.status == "running" {
        if iteration_error {
            summary.status = "completed_with_errors".to_owned();
        } else {
            summary.status = "completed".to_owned();
        }
    }
    // 只有完整无错误地扫描仍在线的卷才能把未见文件标为 missing。
    if summary.status == "completed" {
        summary.missing_count =
            database.mark_missing_after_scan(volume.id, scan_id, &started_at)?;
    }
    if summary.status == "completed" && !options.metadata_only {
        let hashes = if options.full_hash {
            complete_hashes_with_cancel_and_progress(
                database,
                config,
                Some(volume.id),
                Some(&cancelled),
                None,
            )?
        } else {
            hash_duplicate_candidates(database, config)?
        };
        summary.sampled_count = hashes.sampled;
        summary.full_hashed_count = hashes.full_hashed;
        summary.error_count = summary.error_count.saturating_add(hashes.errors);
        summary.bytes_read = hashes.bytes_read;
        if hashes.interrupted {
            summary.status = "interrupted".to_owned();
        } else if hashes.errors > 0 {
            summary.status = "completed_with_errors".to_owned();
        }
    }
    let checkpoint = last_path
        .as_ref()
        .map(|path| json!({"last_path": path}).to_string());
    database.finish_scan_run(
        scan_id,
        &summary.status,
        summary.discovered_count,
        summary.metadata_reused_count,
        summary.sampled_count,
        summary.full_hashed_count,
        summary.skipped_count,
        summary.missing_count,
        summary.error_count,
        summary.bytes_read,
        checkpoint.as_deref(),
    )?;
    emit_progress(options, &summary, last_path.as_deref());
    Ok(summary)
}

fn emit_progress(options: &ScanOptions, summary: &ScanSummary, current_path: Option<&str>) {
    if let Some(callback) = &options.progress_callback {
        callback(summary, current_path);
    }
}

fn persist_metadata_batch(
    database: &mut Database,
    volume_id: i64,
    scan_id: i64,
    pending: &mut Vec<FileMetadata>,
    summary: &mut ScanSummary,
) -> Result<()> {
    let outcomes = database.observe_files(volume_id, scan_id, pending)?;
    for outcome in outcomes {
        summary.discovered_count = summary.discovered_count.saturating_add(1);
        if outcome == MetadataOutcome::Reused {
            summary.metadata_reused_count = summary.metadata_reused_count.saturating_add(1);
        }
    }
    pending.clear();
    Ok(())
}

/// 对已知在线卷中同大小候选补齐抽样和必要完整哈希。
pub fn hash_duplicate_candidates(database: &mut Database, config: &Config) -> Result<HashStats> {
    let mut stats = HashStats::default();
    let volumes = database
        .volumes()?
        .into_iter()
        .map(|volume| (volume.id, volume))
        .collect::<HashMap<_, _>>();
    let mut after_id = 0;
    loop {
        let candidates = database.candidate_files_page(after_id, config.query_page_size)?;
        let Some(last) = candidates.last() else {
            break;
        };
        after_id = last.id;
        for record in &candidates {
            if record.sample_hash.is_none() {
                hash_sample_for_record(database, config, record, &volumes, &mut stats)?;
            }
        }
    }
    // 第二轮由 SQLite 逐条判断抽样哈希是否有匹配，避免构建全量 HashMap。
    let mut after_id = 0;
    loop {
        let candidates = database.candidate_files_page(after_id, config.query_page_size)?;
        let Some(last) = candidates.last() else {
            break;
        };
        after_id = last.id;
        for record in candidates {
            if let Some(sample) = &record.sample_hash
                && record.full_hash.is_none()
                && database.sample_hash_has_duplicate(sample, record.file_size)?
            {
                hash_full_for_record(database, config, &record, &volumes, &mut stats)?;
            }
        }
    }
    Ok(stats)
}

/// `hash complete` 使用的保守全量完整哈希入口。
pub fn complete_hashes(
    database: &mut Database,
    config: &Config,
    only_volume: Option<i64>,
) -> Result<HashStats> {
    complete_hashes_with_cancel_and_progress(database, config, only_volume, None, None)
}

pub fn complete_hashes_with_cancel(
    database: &mut Database,
    config: &Config,
    only_volume: Option<i64>,
    cancel_flag: Option<&AtomicBool>,
) -> Result<HashStats> {
    complete_hashes_with_cancel_and_progress(database, config, only_volume, cancel_flag, None)
}

pub fn complete_hashes_with_cancel_and_progress(
    database: &mut Database,
    config: &Config,
    only_volume: Option<i64>,
    cancel_flag: Option<&AtomicBool>,
    progress_callback: Option<&HashProgressCallback>,
) -> Result<HashStats> {
    let mut stats = HashStats::default();
    let volumes = database
        .volumes()?
        .into_iter()
        .map(|volume| (volume.id, volume))
        .collect::<HashMap<_, _>>();
    for volume in volumes
        .values()
        .filter(|volume| only_volume.is_none_or(|id| id == volume.id))
    {
        if cancel_flag.is_some_and(|flag| flag.load(Ordering::SeqCst)) {
            stats.interrupted = true;
            break;
        }
        if !volume.mount_path.is_dir() {
            database.set_volume_online(volume.id, false)?;
            continue;
        }
        database.set_volume_online(volume.id, true)?;
        let mut after_id = 0;
        loop {
            let records =
                database.files_for_volume_page(volume.id, after_id, config.query_page_size)?;
            let Some(last) = records.last() else {
                break;
            };
            after_id = last.id;
            for record in records {
                if cancel_flag.is_some_and(|flag| flag.load(Ordering::SeqCst)) {
                    stats.interrupted = true;
                    break;
                }
                if record.full_hash.is_none() {
                    hash_full_for_record(database, config, &record, &volumes, &mut stats)?;
                } else {
                    stats.skipped = stats.skipped.saturating_add(1);
                }
                stats.processed = stats.processed.saturating_add(1);
                emit_hash_progress(
                    progress_callback,
                    &stats,
                    Some(&record.absolute_path(&volume.mount_path)),
                );
            }
            if stats.interrupted {
                break;
            }
        }
    }
    emit_hash_progress(progress_callback, &stats, None);
    Ok(stats)
}

fn emit_hash_progress(
    callback: Option<&HashProgressCallback>,
    stats: &HashStats,
    current_path: Option<&Path>,
) {
    if let Some(callback) = callback {
        let current_path = current_path.map(|path| path.to_string_lossy().into_owned());
        callback(stats, current_path.as_deref());
    }
}

fn hash_sample_for_record(
    database: &mut Database,
    config: &Config,
    record: &FileRecord,
    volumes: &HashMap<i64, Volume>,
    stats: &mut HashStats,
) -> Result<()> {
    let Some(volume) = volumes.get(&record.volume_id) else {
        return Ok(());
    };
    let path = record.absolute_path(&volume.mount_path);
    let before = match file_metadata(&volume.mount_path, &path) {
        Ok(metadata) if metadata_matches_record(&metadata, record) => metadata,
        Ok(_) => {
            database.mark_hash_problem(
                record.id,
                "读取前元数据与索引不一致；等待重新扫描",
                true,
            )?;
            stats.errors = stats.errors.saturating_add(1);
            return Ok(());
        }
        Err(error) => {
            mark_access_failure(database, volume, record, &error, stats)?;
            return Ok(());
        }
    };
    match sample_hash(&path, config.sample_bytes, config.read_buffer_bytes) {
        Ok(hash) => {
            if !metadata_still_matches(&path, &before)? {
                database.mark_hash_problem(record.id, "抽样期间文件发生变化；指纹已作废", true)?;
                stats.errors = stats.errors.saturating_add(1);
                return Ok(());
            }
            database.attach_hashes(record.id, record.file_size, Some(&hash.hex), None)?;
            stats.sampled = stats.sampled.saturating_add(1);
            stats.bytes_read = stats.bytes_read.saturating_add(hash.bytes_read);
        }
        Err(error) => mark_access_failure(database, volume, record, &error, stats)?,
    }
    Ok(())
}

fn hash_full_for_record(
    database: &mut Database,
    config: &Config,
    record: &FileRecord,
    volumes: &HashMap<i64, Volume>,
    stats: &mut HashStats,
) -> Result<()> {
    let Some(volume) = volumes.get(&record.volume_id) else {
        return Ok(());
    };
    let path = record.absolute_path(&volume.mount_path);
    let before = match file_metadata(&volume.mount_path, &path) {
        Ok(metadata) if metadata_matches_record(&metadata, record) => metadata,
        Ok(_) => {
            database.mark_hash_problem(
                record.id,
                "读取前元数据与索引不一致；等待重新扫描",
                true,
            )?;
            stats.errors = stats.errors.saturating_add(1);
            return Ok(());
        }
        Err(error) => {
            mark_access_failure(database, volume, record, &error, stats)?;
            return Ok(());
        }
    };
    match full_hash(&path, config.read_buffer_bytes) {
        Ok(hash) => {
            if !metadata_still_matches(&path, &before)? {
                database.mark_hash_problem(
                    record.id,
                    "完整哈希期间文件发生变化；完整哈希未被信任",
                    true,
                )?;
                stats.errors = stats.errors.saturating_add(1);
                return Ok(());
            }
            database.attach_hashes(
                record.id,
                record.file_size,
                record.sample_hash.as_deref(),
                Some(&hash.hex),
            )?;
            stats.full_hashed = stats.full_hashed.saturating_add(1);
            stats.bytes_read = stats.bytes_read.saturating_add(hash.bytes_read);
        }
        Err(error) => mark_access_failure(database, volume, record, &error, stats)?,
    }
    Ok(())
}

fn mark_access_failure(
    database: &mut Database,
    volume: &Volume,
    record: &FileRecord,
    error: &anyhow::Error,
    stats: &mut HashStats,
) -> Result<()> {
    if !volume.mount_path.is_dir() {
        database.set_volume_online(volume.id, false)?;
        return Ok(());
    }
    database.mark_hash_problem(record.id, &error.to_string(), false)?;
    stats.errors = stats.errors.saturating_add(1);
    Ok(())
}

fn metadata_matches_record(metadata: &FileMetadata, record: &FileRecord) -> bool {
    metadata.file_size == record.file_size
        && metadata.modified_at_ns == record.modified_at_ns
        && metadata.inode == record.inode
        && metadata.device_id == record.device_id
}

fn should_descend(
    root: &Path,
    entry: &DirEntry,
    patterns: &[String],
    database_path: &Path,
) -> bool {
    if entry.path() == root {
        return true;
    }
    if entry.path() == database_path {
        return false;
    }
    let Ok(relative) = entry.path().strip_prefix(root) else {
        return false;
    };
    let mut components = relative.components();
    let first = components
        .next()
        .map(|component| component.as_os_str().to_string_lossy());
    if matches!(
        first.as_deref(),
        Some(".Spotlight-V100" | ".Trashes" | ".fseventsd" | ".disk-indexer-quarantine")
    ) {
        return false;
    }
    if relative == Path::new(MARKER_FILE) {
        return false;
    }
    let text = relative.to_string_lossy();
    !patterns.iter().any(|pattern| glob_match(pattern, &text))
}

fn glob_match(pattern: &str, text: &str) -> bool {
    let pattern = pattern.as_bytes();
    let text = text.as_bytes();
    let (mut pattern_index, mut text_index, mut star, mut checkpoint) =
        (0usize, 0usize, None, 0usize);
    while text_index < text.len() {
        if pattern_index < pattern.len()
            && (pattern[pattern_index] == b'?' || pattern[pattern_index] == text[text_index])
        {
            pattern_index += 1;
            text_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
            star = Some(pattern_index);
            pattern_index += 1;
            checkpoint = text_index;
        } else if let Some(star_index) = star {
            pattern_index = star_index + 1;
            checkpoint += 1;
            text_index = checkpoint;
        } else {
            return false;
        }
    }
    while pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
        pattern_index += 1;
    }
    pattern_index == pattern.len()
}

pub fn interrupt_flag() -> Result<Arc<AtomicBool>> {
    static FLAG: OnceLock<Result<Arc<AtomicBool>, String>> = OnceLock::new();
    match FLAG.get_or_init(|| {
        let signal = Arc::new(AtomicBool::new(false));
        let handler_flag = Arc::clone(&signal);
        ctrlc::set_handler(move || {
            handler_flag.store(true, Ordering::SeqCst);
        })
        .map(|()| signal)
        .map_err(|error| error.to_string())
    }) {
        Ok(flag) => Ok(Arc::clone(flag)),
        Err(error) => anyhow::bail!("无法安装 Ctrl+C 处理器: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::glob_match;

    #[test]
    fn simple_glob_patterns_work() {
        assert!(glob_match("*.tmp", "nested/file.tmp"));
        assert!(glob_match("a?c", "abc"));
        assert!(!glob_match("a?c", "ac"));
    }
}
