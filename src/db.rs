//! SQLite 持久化层。SQL 只在本模块中维护，调用方不拼接查询语句。

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};
use serde_json::json;

use crate::config::Config;
use crate::model::{
    FileMetadata, FileRecord, PhysicalDevice, Volume, VolumeIdentityConflict, VolumeIdentityState,
    VolumeRole,
};
use crate::util::{display_bytes, display_path, now, path_bytes, path_from_bytes};

const MIGRATIONS: &[(&str, &str)] = &[
    (
        "0001_initial",
        include_str!("../migrations/0001_initial.sql"),
    ),
    (
        "0002_volume_identity_safety",
        include_str!("../migrations/0002_volume_identity_safety.sql"),
    ),
    (
        "0003_hash_report_safety",
        include_str!("../migrations/0003_hash_report_safety.sql"),
    ),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataOutcome {
    New,
    Reused,
    Changed,
}

/// 已初始化且启用安全 SQLite 设置的数据库连接。
pub struct Database {
    connection: Connection,
    path: PathBuf,
}

pub struct VolumeUpsert<'a> {
    pub volume_uid: &'a str,
    pub marker_uid: Option<&'a str>,
    pub root: &'a Path,
    pub filesystem: Option<&'a str>,
    pub system_volume_uuid: Option<&'a str>,
    pub device_serial: Option<&'a str>,
    pub partition_uuid: Option<&'a str>,
    pub total_size: Option<i64>,
    pub physical_device_id: Option<i64>,
    pub identity_state: VolumeIdentityState,
    pub role: VolumeRole,
}

pub struct PhysicalDeviceUpsert<'a> {
    pub stable_uid: &'a str,
    pub media_uuid: Option<&'a str>,
    pub device_serial: Option<&'a str>,
    pub model: Option<&'a str>,
    pub transport: Option<&'a str>,
    pub total_size: Option<i64>,
}

pub struct VolumeConflictInput<'a> {
    pub existing_volume_id: i64,
    pub candidate_marker_uid: Option<&'a str>,
    pub candidate_root: &'a Path,
    pub candidate_filesystem: Option<&'a str>,
    pub candidate_system_volume_uuid: Option<&'a str>,
    pub candidate_partition_uuid: Option<&'a str>,
    pub candidate_media_uuid: Option<&'a str>,
    pub candidate_device_serial: Option<&'a str>,
    pub candidate_total_size: Option<i64>,
    pub candidate_physical_device_id: Option<i64>,
}

impl Database {
    pub fn open(config: &Config) -> Result<Self> {
        if let Some(parent) = config.database_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("无法创建数据库目录 {}", parent.display()))?;
        }
        let connection = Connection::open(&config.database_path)
            .with_context(|| format!("无法打开数据库 {}", config.database_path.display()))?;
        connection
            .busy_timeout(std::time::Duration::from_secs(10))
            .context("无法设置 SQLite busy_timeout")?;
        connection.execute_batch(
            "PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON; PRAGMA synchronous = NORMAL;",
        )?;
        let mut database = Self {
            connection,
            path: config.database_path.clone(),
        };
        database.migrate()?;
        Ok(database)
    }

    fn migrate(&mut self) -> Result<()> {
        self.connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version TEXT PRIMARY KEY,
                applied_at TEXT NOT NULL
            );",
        )?;
        for (version, sql) in MIGRATIONS {
            let applied: Option<String> = self
                .connection
                .query_row(
                    "SELECT version FROM schema_migrations WHERE version = ?1",
                    [version],
                    |row| row.get(0),
                )
                .optional()?;
            if applied.is_none() {
                let transaction = self
                    .connection
                    .transaction()
                    .with_context(|| format!("无法开始数据库迁移事务 {version}"))?;
                transaction
                    .execute_batch(sql)
                    .with_context(|| format!("无法执行数据库迁移 {version}"))?;
                transaction.execute(
                    "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
                    params![version, now()],
                )?;
                transaction
                    .commit()
                    .with_context(|| format!("无法提交数据库迁移 {version}"))?;
            }
        }
        self.backfill_volume_identity_links()?;
        Ok(())
    }

    fn backfill_volume_identity_links(&mut self) -> Result<()> {
        let legacy_volumes = {
            let mut statement = self.connection.prepare(
                "SELECT id, system_volume_uuid, partition_uuid, device_serial, filesystem,
                        total_size, mount_path FROM volumes WHERE physical_device_id IS NULL",
            )?;
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<i64>>(5)?,
                        row.get::<_, Vec<u8>>(6)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (
            volume_id,
            system_uuid,
            partition_uuid,
            legacy_media_uuid,
            filesystem,
            total_size,
            mount_path,
        ) in legacy_volumes
        {
            let mut stable_input = Vec::new();
            for value in [&system_uuid, &partition_uuid, &legacy_media_uuid]
                .into_iter()
                .flatten()
            {
                stable_input.extend_from_slice(value.as_bytes());
                stable_input.push(0);
            }
            if stable_input.is_empty() {
                stable_input
                    .extend_from_slice(filesystem.as_deref().unwrap_or("unknown").as_bytes());
                stable_input.push(0);
                stable_input.extend_from_slice(&total_size.unwrap_or_default().to_le_bytes());
                stable_input.extend_from_slice(&mount_path);
            }
            let stable_uid = format!(
                "physical:legacy:v1:{}",
                blake3::hash(&stable_input).to_hex()
            );
            let physical_device = self.upsert_physical_device(PhysicalDeviceUpsert {
                stable_uid: &stable_uid,
                media_uuid: legacy_media_uuid.as_deref(),
                device_serial: None,
                model: None,
                transport: None,
                total_size,
            })?;
            let identity_state =
                if system_uuid.is_some() || partition_uuid.is_some() || legacy_media_uuid.is_some()
                {
                    VolumeIdentityState::Verified
                } else {
                    VolumeIdentityState::Fallback
                };
            self.connection.execute(
                "UPDATE volumes SET physical_device_id = ?1, identity_state = ?2, updated_at = ?3
                 WHERE id = ?4",
                params![
                    physical_device.id,
                    identity_state.as_str(),
                    now(),
                    volume_id
                ],
            )?;
            self.record_volume_event(
                Some(volume_id),
                None,
                "legacy_physical_identity_backfilled",
                None,
                Some(json!({"physical_device_id": physical_device.id})),
            )?;
        }
        Ok(())
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn schema_version(&self) -> Result<i64> {
        let value =
            self.connection
                .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
                    row.get(0)
                })?;
        Ok(value)
    }

    pub fn upsert_volume(&mut self, input: VolumeUpsert<'_>) -> Result<Volume> {
        let VolumeUpsert {
            volume_uid,
            marker_uid,
            root,
            filesystem,
            system_volume_uuid,
            device_serial,
            partition_uuid,
            total_size,
            physical_device_id,
            identity_state,
            role,
        } = input;
        let timestamp = now();
        let volume_name = root
            .file_name()
            .filter(|name| !name.is_empty())
            .map_or_else(
                || display_path(root),
                |name| name.to_string_lossy().into_owned(),
            );
        let root_bytes = path_bytes(root);
        let existing: Option<i64> = self
            .connection
            .query_row(
                "SELECT id FROM volumes WHERE volume_uid = ?1",
                [volume_uid],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(id) = existing {
            self.connection.execute(
                "UPDATE volumes SET marker_uid = COALESCE(?1, marker_uid), volume_name = ?2,
                    filesystem = COALESCE(?3, filesystem), mount_path = ?4, mount_path_display = ?5,
                    system_volume_uuid = COALESCE(?6, system_volume_uuid),
                    device_serial = COALESCE(?7, device_serial), partition_uuid = COALESCE(?8, partition_uuid),
                    total_size = COALESCE(?9, total_size), physical_device_id = COALESCE(?10, physical_device_id),
                    identity_state = ?11, role = ?12, is_online = 1,
                    last_seen_at = ?13, updated_at = ?13 WHERE id = ?14",
                params![
                    marker_uid,
                    volume_name,
                    filesystem,
                    root_bytes,
                    display_path(root),
                    system_volume_uuid,
                    device_serial,
                    partition_uuid,
                    total_size,
                    physical_device_id,
                    identity_state.as_str(),
                    role.as_str(),
                    timestamp,
                    id
                ],
            )?;
            return self.volume_by_id(id);
        }
        self.connection.execute(
            "INSERT INTO volumes (
                volume_uid, marker_uid, volume_name, filesystem, mount_path, mount_path_display,
                system_volume_uuid, device_serial, partition_uuid, total_size, role, is_online,
                physical_device_id, identity_state, first_seen_at, last_seen_at, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 1, ?12, ?13, ?14, ?14, ?14, ?14)",
            params![
                volume_uid,
                marker_uid,
                volume_name,
                filesystem,
                root_bytes,
                display_path(root),
                system_volume_uuid,
                device_serial,
                partition_uuid,
                total_size,
                role.as_str(),
                physical_device_id,
                identity_state.as_str(),
                timestamp
            ],
        )?;
        self.volume_by_id(self.connection.last_insert_rowid())
    }

    pub fn upsert_physical_device(
        &mut self,
        input: PhysicalDeviceUpsert<'_>,
    ) -> Result<PhysicalDevice> {
        let timestamp = now();
        let existing: Option<i64> = self
            .connection
            .query_row(
                "SELECT id FROM physical_devices WHERE stable_uid = ?1",
                [input.stable_uid],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(id) = existing {
            self.connection.execute(
                "UPDATE physical_devices SET media_uuid = COALESCE(?1, media_uuid),
                    device_serial = COALESCE(?2, device_serial), model = COALESCE(?3, model),
                    transport = COALESCE(?4, transport), total_size = COALESCE(?5, total_size),
                    last_seen_at = ?6, updated_at = ?6 WHERE id = ?7",
                params![
                    input.media_uuid,
                    input.device_serial,
                    input.model,
                    input.transport,
                    input.total_size,
                    timestamp,
                    id
                ],
            )?;
            return self.physical_device_by_id(id);
        }
        self.connection.execute(
            "INSERT INTO physical_devices (
                stable_uid, media_uuid, device_serial, model, transport, total_size,
                first_seen_at, last_seen_at, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?7, ?7)",
            params![
                input.stable_uid,
                input.media_uuid,
                input.device_serial,
                input.model,
                input.transport,
                input.total_size,
                timestamp
            ],
        )?;
        self.physical_device_by_id(self.connection.last_insert_rowid())
    }

    pub fn physical_device_by_id(&self, id: i64) -> Result<PhysicalDevice> {
        self.connection
            .query_row(
                "SELECT id, stable_uid, media_uuid, device_serial, model, transport, total_size,
                        first_seen_at, last_seen_at FROM physical_devices WHERE id = ?1",
                [id],
                row_to_physical_device,
            )
            .optional()?
            .ok_or_else(|| anyhow!("未找到物理设备 ID {id}"))
    }

    pub fn volume_by_id(&self, id: i64) -> Result<Volume> {
        self.connection
            .query_row(
                "SELECT id, volume_uid, marker_uid, volume_name, filesystem, mount_path,
                        system_volume_uuid, device_serial, partition_uuid, total_size, physical_device_id,
                        identity_state, role, is_online, first_seen_at, last_seen_at
                 FROM volumes WHERE id = ?1",
                [id],
                row_to_volume,
            )
            .optional()?
            .ok_or_else(|| anyhow!("未找到卷 ID {id}"))
    }

    pub fn volume_by_uid(&self, uid: &str) -> Result<Option<Volume>> {
        self.connection
            .query_row(
                "SELECT id, volume_uid, marker_uid, volume_name, filesystem, mount_path,
                        system_volume_uuid, device_serial, partition_uuid, total_size, physical_device_id,
                        identity_state, role, is_online, first_seen_at, last_seen_at
                 FROM volumes WHERE volume_uid = ?1",
                [uid],
                row_to_volume,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn volumes_by_marker(&self, marker_uid: &str) -> Result<Vec<Volume>> {
        let mut statement = self.connection.prepare(
            "SELECT id, volume_uid, marker_uid, volume_name, filesystem, mount_path,
                    system_volume_uuid, device_serial, partition_uuid, total_size, physical_device_id,
                    identity_state, role, is_online, first_seen_at, last_seen_at
             FROM volumes WHERE marker_uid = ?1 ORDER BY id",
        )?;
        statement
            .query_map([marker_uid], row_to_volume)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn volumes(&self) -> Result<Vec<Volume>> {
        let mut statement = self.connection.prepare(
            "SELECT id, volume_uid, marker_uid, volume_name, filesystem, mount_path,
                    system_volume_uuid, device_serial, partition_uuid, total_size, physical_device_id,
                    identity_state, role, is_online, first_seen_at, last_seen_at
             FROM volumes ORDER BY volume_name, id",
        )?;
        statement
            .query_map([], row_to_volume)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn record_volume_conflict(
        &mut self,
        input: VolumeConflictInput<'_>,
    ) -> Result<VolumeIdentityConflict> {
        let candidate_root = path_bytes(input.candidate_root);
        let existing_open: Option<i64> = self
            .connection
            .query_row(
                "SELECT id FROM volume_identity_conflicts
                 WHERE existing_volume_id = ?1 AND candidate_mount_path = ?2 AND state = 'open'",
                params![input.existing_volume_id, candidate_root],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(id) = existing_open {
            self.connection.execute(
                "UPDATE volume_identity_conflicts SET candidate_marker_uid = ?1,
                    candidate_filesystem = ?2, candidate_system_volume_uuid = ?3,
                    candidate_partition_uuid = ?4, candidate_media_uuid = ?5,
                    candidate_device_serial = ?6, candidate_total_size = ?7,
                    candidate_physical_device_id = ?8, updated_at = ?9 WHERE id = ?10",
                params![
                    input.candidate_marker_uid,
                    input.candidate_filesystem,
                    input.candidate_system_volume_uuid,
                    input.candidate_partition_uuid,
                    input.candidate_media_uuid,
                    input.candidate_device_serial,
                    input.candidate_total_size,
                    input.candidate_physical_device_id,
                    now(),
                    id
                ],
            )?;
            return self.volume_conflict_by_id(id);
        }
        let timestamp = now();
        self.connection.execute(
            "INSERT INTO volume_identity_conflicts (
                existing_volume_id, candidate_marker_uid, candidate_mount_path,
                candidate_mount_path_display, candidate_filesystem, candidate_system_volume_uuid,
                candidate_partition_uuid, candidate_media_uuid, candidate_device_serial,
                candidate_total_size, candidate_physical_device_id, state, detected_at, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'open', ?12, ?12, ?12)",
            params![
                input.existing_volume_id,
                input.candidate_marker_uid,
                candidate_root,
                display_path(input.candidate_root),
                input.candidate_filesystem,
                input.candidate_system_volume_uuid,
                input.candidate_partition_uuid,
                input.candidate_media_uuid,
                input.candidate_device_serial,
                input.candidate_total_size,
                input.candidate_physical_device_id,
                timestamp
            ],
        )?;
        let conflict_id = self.connection.last_insert_rowid();
        self.record_volume_event(
            Some(input.existing_volume_id),
            Some(conflict_id),
            "marker_conflict_detected",
            None,
            Some(json!({"candidate_path": display_path(input.candidate_root)})),
        )?;
        self.volume_conflict_by_id(conflict_id)
    }

    pub fn volume_conflict_by_id(&self, id: i64) -> Result<VolumeIdentityConflict> {
        self.connection
            .query_row(
                "SELECT c.id, c.existing_volume_id, v.volume_uid, c.candidate_marker_uid,
                        c.candidate_mount_path, c.candidate_filesystem, c.candidate_system_volume_uuid,
                        c.candidate_partition_uuid, c.candidate_media_uuid, c.candidate_device_serial,
                        c.candidate_total_size, c.candidate_physical_device_id, c.state, c.resolution,
                        c.resolved_volume_id, c.detected_at, c.resolved_at
                 FROM volume_identity_conflicts c JOIN volumes v ON v.id = c.existing_volume_id
                 WHERE c.id = ?1",
                [id],
                row_to_volume_conflict,
            )
            .optional()?
            .ok_or_else(|| anyhow!("未找到卷身份冲突 ID {id}"))
    }

    pub fn open_volume_conflicts(&self) -> Result<Vec<VolumeIdentityConflict>> {
        let mut statement = self.connection.prepare(
            "SELECT c.id, c.existing_volume_id, v.volume_uid, c.candidate_marker_uid,
                    c.candidate_mount_path, c.candidate_filesystem, c.candidate_system_volume_uuid,
                    c.candidate_partition_uuid, c.candidate_media_uuid, c.candidate_device_serial,
                    c.candidate_total_size, c.candidate_physical_device_id, c.state, c.resolution,
                    c.resolved_volume_id, c.detected_at, c.resolved_at
             FROM volume_identity_conflicts c JOIN volumes v ON v.id = c.existing_volume_id
             WHERE c.state = 'open' ORDER BY c.detected_at DESC, c.id DESC",
        )?;
        statement
            .query_map([], row_to_volume_conflict)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn record_volume_event(
        &mut self,
        volume_id: Option<i64>,
        conflict_id: Option<i64>,
        event_type: &str,
        old_value: Option<serde_json::Value>,
        new_value: Option<serde_json::Value>,
    ) -> Result<()> {
        self.connection.execute(
            "INSERT INTO volume_events(volume_id, conflict_id, event_type, old_value_json, new_value_json, occurred_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                volume_id,
                conflict_id,
                event_type,
                old_value.map(|value| value.to_string()),
                new_value.map(|value| value.to_string()),
                now()
            ],
        )?;
        Ok(())
    }

    pub fn resolve_conflict_as_new_volume(
        &mut self,
        conflict_id: i64,
        volume_uid: &str,
        role: VolumeRole,
    ) -> Result<Volume> {
        let transaction = self.connection.transaction()?;
        let conflict = transaction
            .query_row(
                "SELECT c.id, c.existing_volume_id, v.volume_uid, c.candidate_marker_uid,
                        c.candidate_mount_path, c.candidate_filesystem, c.candidate_system_volume_uuid,
                        c.candidate_partition_uuid, c.candidate_media_uuid, c.candidate_device_serial,
                        c.candidate_total_size, c.candidate_physical_device_id, c.state, c.resolution,
                        c.resolved_volume_id, c.detected_at, c.resolved_at
                 FROM volume_identity_conflicts c JOIN volumes v ON v.id = c.existing_volume_id
                 WHERE c.id = ?1",
                [conflict_id],
                row_to_volume_conflict,
            )
            .optional()?
            .ok_or_else(|| anyhow!("未找到卷身份冲突 ID {conflict_id}"))?;
        if conflict.state != "open" {
            bail!("卷身份冲突 {conflict_id} 已处理，不能重复解决");
        }
        let timestamp = now();
        let volume_name = conflict
            .candidate_mount_path
            .file_name()
            .filter(|name| !name.is_empty())
            .map_or_else(
                || display_path(&conflict.candidate_mount_path),
                |name| name.to_string_lossy().into_owned(),
            );
        transaction.execute(
            "INSERT INTO volumes (
                volume_uid, marker_uid, volume_name, filesystem, mount_path, mount_path_display,
                system_volume_uuid, device_serial, partition_uuid, total_size, role, is_online,
                physical_device_id, identity_state, first_seen_at, last_seen_at, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 1, ?12, 'possible_clone',
                       ?13, ?13, ?13, ?13)",
            params![
                volume_uid,
                conflict.candidate_marker_uid,
                volume_name,
                conflict.candidate_filesystem,
                path_bytes(&conflict.candidate_mount_path),
                display_path(&conflict.candidate_mount_path),
                conflict.candidate_system_volume_uuid,
                conflict.candidate_device_serial,
                conflict.candidate_partition_uuid,
                conflict.candidate_total_size,
                role.as_str(),
                conflict.candidate_physical_device_id,
                timestamp
            ],
        )?;
        let new_volume_id = transaction.last_insert_rowid();
        transaction.execute(
            "UPDATE volume_identity_conflicts SET state = 'resolved', resolution = 'as_new_volume',
                resolved_volume_id = ?1, resolved_at = ?2, updated_at = ?2 WHERE id = ?3",
            params![new_volume_id, timestamp, conflict_id],
        )?;
        let details =
            json!({"new_volume_id": new_volume_id, "resolution": "as_new_volume"}).to_string();
        transaction.execute(
            "INSERT INTO volume_events(volume_id, conflict_id, event_type, old_value_json, new_value_json, occurred_at)
             VALUES (?1, ?2, 'marker_conflict_resolved_as_new_volume', NULL, ?3, ?4)",
            params![conflict.existing_volume_id, conflict_id, details, timestamp],
        )?;
        transaction.execute(
            "INSERT INTO volume_events(volume_id, conflict_id, event_type, old_value_json, new_value_json, occurred_at)
             VALUES (?1, ?2, 'volume_created_from_marker_conflict', NULL, ?3, ?4)",
            params![new_volume_id, conflict_id, json!({"source_volume_id": conflict.existing_volume_id}).to_string(), timestamp],
        )?;
        transaction.commit()?;
        self.volume_by_id(new_volume_id)
    }

    pub fn refresh_volume_online_states(&mut self) -> Result<()> {
        let volumes = self.volumes()?;
        let timestamp = now();
        for volume in volumes {
            let online = volume.mount_path.is_dir();
            self.connection.execute(
                "UPDATE volumes SET is_online = ?1, updated_at = ?2 WHERE id = ?3",
                params![i64::from(online), timestamp, volume.id],
            )?;
        }
        Ok(())
    }

    pub fn set_volume_online(&mut self, volume_id: i64, online: bool) -> Result<()> {
        self.connection.execute(
            "UPDATE volumes SET is_online = ?1, last_seen_at = CASE WHEN ?1 = 1 THEN ?2 ELSE last_seen_at END,
                updated_at = ?2 WHERE id = ?3",
            params![i64::from(online), now(), volume_id],
        )?;
        Ok(())
    }

    pub fn create_scan_run(&mut self, volume_id: i64, mode: &str) -> Result<i64> {
        let timestamp = now();
        self.connection.execute(
            "INSERT INTO scan_runs(volume_id, root_relative_path, mode, status, started_at, created_at, updated_at)
             VALUES (?1, X'', ?2, 'running', ?3, ?3, ?3)",
            params![volume_id, mode, timestamp],
        )?;
        Ok(self.connection.last_insert_rowid())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn finish_scan_run(
        &mut self,
        scan_id: i64,
        status: &str,
        discovered: u64,
        reused: u64,
        sampled: u64,
        full_hashed: u64,
        skipped: u64,
        missing: u64,
        errors: u64,
        bytes_read: u64,
        checkpoint: Option<&str>,
    ) -> Result<()> {
        self.connection.execute(
            "UPDATE scan_runs SET status = ?1, finished_at = ?2, discovered_count = ?3,
                metadata_reused_count = ?4, sampled_count = ?5, full_hashed_count = ?6,
                skipped_count = ?7, missing_count = ?8, error_count = ?9, bytes_read = ?10,
                checkpoint = ?11, updated_at = ?2 WHERE id = ?12",
            params![
                status,
                now(),
                discovered,
                reused,
                sampled,
                full_hashed,
                skipped,
                missing,
                errors,
                bytes_read,
                checkpoint,
                scan_id
            ],
        )?;
        Ok(())
    }

    pub fn observe_file(
        &mut self,
        volume_id: i64,
        scan_id: i64,
        metadata: &FileMetadata,
    ) -> Result<MetadataOutcome> {
        let mut outcomes =
            self.observe_files(volume_id, scan_id, std::slice::from_ref(metadata))?;
        outcomes
            .pop()
            .ok_or_else(|| anyhow!("内部错误：单文件批次没有扫描结果"))
    }

    /// 将一批扫描元数据放入同一个 SQLite 事务，避免每个小文件都单独提交。
    pub fn observe_files(
        &mut self,
        volume_id: i64,
        scan_id: i64,
        metadata: &[FileMetadata],
    ) -> Result<Vec<MetadataOutcome>> {
        let existing = metadata
            .iter()
            .map(|entry| self.file_record_by_path(volume_id, &path_bytes(&entry.relative_path)))
            .collect::<Result<Vec<_>>>()?;
        let transaction = self.connection.transaction()?;
        let mut outcomes = Vec::with_capacity(metadata.len());
        for (entry, record) in metadata.iter().zip(existing) {
            outcomes.push(observe_file_in_transaction(
                &transaction,
                volume_id,
                scan_id,
                entry,
                record,
            )?);
        }
        transaction.commit()?;
        Ok(outcomes)
    }

    pub fn mark_missing_after_scan(
        &mut self,
        volume_id: i64,
        scan_id: i64,
        started_at: &str,
    ) -> Result<u64> {
        let transaction = self.connection.transaction()?;
        let mut statement = transaction.prepare(
            "SELECT id FROM file_copies WHERE volume_id = ?1 AND status = 'present' AND last_seen_at < ?2",
        )?;
        let ids = statement
            .query_map(params![volume_id, started_at], |row| row.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        for id in &ids {
            transaction.execute(
                "UPDATE file_copies SET status = 'missing', updated_at = ?1 WHERE id = ?2",
                params![now(), id],
            )?;
            insert_event(
                &transaction,
                *id,
                Some(scan_id),
                "marked_missing",
                Some(json!({"status": "present"})),
                Some(json!({"status": "missing"})),
            )?;
        }
        transaction.commit()?;
        Ok(u64::try_from(ids.len()).unwrap_or(u64::MAX))
    }

    pub fn file_record_by_path(
        &self,
        volume_id: i64,
        relative_path: &[u8],
    ) -> Result<Option<FileRecord>> {
        self.connection
            .query_row(
                &file_record_select("WHERE f.volume_id = ?1 AND f.relative_path = ?2"),
                params![volume_id, relative_path],
                row_to_file_record,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn file_record_by_id(&self, id: i64) -> Result<Option<FileRecord>> {
        self.connection
            .query_row(
                &file_record_select("WHERE f.id = ?1"),
                [id],
                row_to_file_record,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn candidate_files(&self) -> Result<Vec<FileRecord>> {
        let sql = format!(
            "{} AND f.status = 'present' AND v.is_online = 1 AND f.file_size IN (
                SELECT file_size FROM file_copies WHERE status = 'present' GROUP BY file_size HAVING COUNT(*) > 1
             ) ORDER BY f.file_size, f.id",
            file_record_select("WHERE 1 = 1")
        );
        let mut statement = self.connection.prepare(&sql)?;
        statement
            .query_map([], row_to_file_record)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn files_for_volume(&self, volume_id: i64) -> Result<Vec<FileRecord>> {
        let sql = format!(
            "{} WHERE f.volume_id = ?1 AND f.status = 'present' ORDER BY f.id",
            file_record_select("")
        );
        let mut statement = self.connection.prepare(&sql)?;
        statement
            .query_map([volume_id], row_to_file_record)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn attach_hashes(
        &mut self,
        file_id: i64,
        file_size: u64,
        sample_hash: Option<&str>,
        full_hash: Option<&str>,
    ) -> Result<()> {
        let transaction = self.connection.transaction()?;
        let timestamp = now();
        let content_id = if let Some(full) = full_hash {
            transaction.execute(
                "INSERT INTO contents(file_size, sample_hash, sample_algorithm, full_hash, hash_algorithm,
                    hash_state, first_seen_at, last_seen_at, created_at, updated_at)
                 VALUES (?1, ?2, 'blake3-sample-v1', ?3, 'blake3', 'full', ?4, ?4, ?4, ?4)
                 ON CONFLICT(full_hash, file_size) DO UPDATE SET last_seen_at = excluded.last_seen_at,
                    updated_at = excluded.updated_at, sample_hash = COALESCE(contents.sample_hash, excluded.sample_hash)",
                params![file_size, sample_hash, full, timestamp],
            )?;
            transaction.query_row(
                "SELECT id FROM contents WHERE full_hash = ?1 AND file_size = ?2",
                params![full, file_size],
                |row| row.get::<_, i64>(0),
            )?
        } else {
            0
        };
        transaction.execute(
            "UPDATE file_copies SET sample_hash = COALESCE(?1, sample_hash),
                sample_algorithm = CASE WHEN ?1 IS NULL THEN sample_algorithm ELSE 'blake3-sample-v1' END,
                full_hash = COALESCE(?2, full_hash), hash_algorithm = 'blake3',
                hash_state = CASE WHEN ?2 IS NOT NULL THEN 'full' WHEN ?1 IS NOT NULL THEN 'sampled' ELSE hash_state END,
                content_id = CASE WHEN ?2 IS NOT NULL THEN ?3 ELSE content_id END,
                last_error = NULL, updated_at = ?4, last_verified_at = CASE WHEN ?2 IS NOT NULL THEN ?4 ELSE last_verified_at END
             WHERE id = ?5",
            params![sample_hash, full_hash, if full_hash.is_some() { Some(content_id) } else { None }, timestamp, file_id],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn mark_hash_problem(&mut self, file_id: i64, error: &str, changed: bool) -> Result<()> {
        let status = if changed { "changed" } else { "unreadable" };
        self.connection.execute(
            "UPDATE file_copies SET hash_state = 'failed', status = ?1, last_error = ?2, updated_at = ?3 WHERE id = ?4",
            params![status, error, now(), file_id],
        )?;
        Ok(())
    }

    pub fn records_by_content(&self, content_id: i64) -> Result<Vec<FileRecord>> {
        let sql = format!(
            "{} WHERE f.content_id = ?1 ORDER BY v.volume_name, f.relative_path",
            file_record_select("")
        );
        let mut statement = self.connection.prepare(&sql)?;
        statement
            .query_map([content_id], row_to_file_record)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn records_by_full_hash(&self, full_hash: &str, file_size: u64) -> Result<Vec<FileRecord>> {
        let sql = format!(
            "{} WHERE f.full_hash = ?1 AND f.file_size = ?2 ORDER BY v.volume_name, f.relative_path",
            file_record_select("")
        );
        let mut statement = self.connection.prepare(&sql)?;
        statement
            .query_map(params![full_hash, file_size], row_to_file_record)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn records_by_sample_hash(
        &self,
        sample_hash: &str,
        file_size: u64,
    ) -> Result<Vec<FileRecord>> {
        let sql = format!(
            "{} WHERE f.sample_hash = ?1 AND f.file_size = ?2 ORDER BY v.volume_name, f.relative_path",
            file_record_select("")
        );
        let mut statement = self.connection.prepare(&sql)?;
        statement
            .query_map(params![sample_hash, file_size], row_to_file_record)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn mark_verified(&mut self, file_id: i64, error: Option<&str>) -> Result<()> {
        let timestamp = now();
        self.connection.execute(
            "UPDATE file_copies SET last_verified_at = ?1, last_error = ?2, updated_at = ?1 WHERE id = ?3",
            params![timestamp, error, file_id],
        )?;
        Ok(())
    }

    pub fn duplicate_content_ids(
        &self,
        min_copies: usize,
        min_size: u64,
    ) -> Result<Vec<(i64, String, u64)>> {
        let threshold = i64::try_from(min_copies).context("副本数量超出 SQLite 整数范围")?;
        let mut statement = self.connection.prepare(
            "SELECT c.id, c.full_hash, c.file_size
             FROM contents c JOIN file_copies f ON f.content_id = c.id
             WHERE c.full_hash IS NOT NULL AND c.file_size >= ?1 AND f.status = 'present'
             GROUP BY c.id HAVING COUNT(*) >= ?2
             ORDER BY c.file_size DESC, COUNT(*) DESC, c.full_hash",
        )?;
        statement
            .query_map(params![min_size, threshold], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn scan_runs(&self) -> Result<Vec<serde_json::Value>> {
        let mut statement = self.connection.prepare(
            "SELECT s.id, s.volume_id, v.volume_name, s.mode, s.status, s.started_at, s.finished_at,
                    s.discovered_count, s.metadata_reused_count, s.sampled_count, s.full_hashed_count,
                    s.skipped_count, s.missing_count, s.error_count, s.bytes_read, s.checkpoint
             FROM scan_runs s JOIN volumes v ON v.id = s.volume_id ORDER BY s.started_at DESC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(json!({
                "id": row.get::<_, i64>(0)?, "volume_id": row.get::<_, i64>(1)?,
                "volume": row.get::<_, String>(2)?, "mode": row.get::<_, String>(3)?,
                "status": row.get::<_, String>(4)?, "started_at": row.get::<_, String>(5)?,
                "finished_at": row.get::<_, Option<String>>(6)?, "discovered_count": row.get::<_, i64>(7)?,
                "metadata_reused_count": row.get::<_, i64>(8)?, "sampled_count": row.get::<_, i64>(9)?,
                "full_hashed_count": row.get::<_, i64>(10)?, "skipped_count": row.get::<_, i64>(11)?,
                "missing_count": row.get::<_, i64>(12)?, "error_count": row.get::<_, i64>(13)?,
                "bytes_read": row.get::<_, i64>(14)?, "checkpoint": row.get::<_, Option<String>>(15)?,
            }))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn scan_run(&self, id: i64) -> Result<Option<serde_json::Value>> {
        Ok(self.scan_runs()?.into_iter().find(|run| run["id"] == id))
    }

    pub fn find_volume_for_path(&self, path: &Path) -> Result<Option<Volume>> {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let mut matching = self
            .volumes()?
            .into_iter()
            .filter(|volume| canonical.starts_with(&volume.mount_path))
            .collect::<Vec<_>>();
        matching.sort_by_key(|volume| std::cmp::Reverse(volume.mount_path.components().count()));
        Ok(matching.into_iter().next())
    }
}

fn observe_file_in_transaction(
    transaction: &Transaction<'_>,
    volume_id: i64,
    scan_id: i64,
    metadata: &FileMetadata,
    existing: Option<FileRecord>,
) -> Result<MetadataOutcome> {
    let timestamp = now();
    let relative_bytes = path_bytes(&metadata.relative_path);
    let storage_object_key = storage_object_key(transaction, volume_id, metadata)?;
    let link_group_id = storage_object_key.clone();
    let outcome = if let Some(record) = existing {
        if metadata_matches(&record, metadata) {
            transaction.execute(
                "UPDATE file_copies SET status = 'present', last_error = NULL, last_seen_at = ?1,
                        storage_object_key = ?2, link_group_id = ?3, updated_at = ?1 WHERE id = ?4",
                params![timestamp, storage_object_key, link_group_id, record.id],
            )?;
            if record.status != "present" {
                insert_event(
                    transaction,
                    record.id,
                    Some(scan_id),
                    "restored",
                    None,
                    Some(json!({"status": "present"})),
                )?;
            }
            MetadataOutcome::Reused
        } else {
            let previous = json!({
                "file_size": record.file_size,
                "modified_at_ns": record.modified_at_ns,
                "inode": record.inode,
                "content_id": record.content_id,
                "full_hash": record.full_hash,
            });
            transaction.execute(
                    "UPDATE file_copies SET filename = ?1, filename_display = ?2, file_size = ?3,
                        modified_at_ns = ?4, created_at_ns = ?5, inode = ?6, device_id = ?7,
                        content_id = NULL, sample_hash = NULL, sample_algorithm = NULL, full_hash = NULL,
                        hash_state = 'none', status = 'present', last_error = NULL, storage_object_key = ?8,
                        link_group_id = ?9, last_seen_at = ?10, updated_at = ?10 WHERE id = ?11",
                    params![
                        path_bytes(&metadata.filename),
                        display_path(&metadata.filename),
                        metadata.file_size,
                        metadata.modified_at_ns,
                        metadata.created_at_ns,
                        metadata.inode,
                        metadata.device_id,
                        storage_object_key,
                        link_group_id,
                        timestamp,
                        record.id
                    ],
                )?;
            insert_event(
                transaction,
                record.id,
                Some(scan_id),
                "content_changed",
                Some(previous),
                Some(json!({"file_size": metadata.file_size})),
            )?;
            MetadataOutcome::Changed
        }
    } else {
        transaction.execute(
            "INSERT INTO file_copies (
                    volume_id, relative_path, relative_path_display, filename, filename_display,
                    file_size, modified_at_ns, created_at_ns, inode, device_id, storage_object_key, link_group_id,
                    status, first_seen_at, last_seen_at, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 'present', ?13, ?13, ?13, ?13)",
            params![
                volume_id,
                relative_bytes,
                display_path(&metadata.relative_path),
                path_bytes(&metadata.filename),
                display_path(&metadata.filename),
                metadata.file_size,
                metadata.modified_at_ns,
                metadata.created_at_ns,
                metadata.inode,
                metadata.device_id,
                storage_object_key,
                link_group_id,
                timestamp
            ],
        )?;
        let file_id = transaction.last_insert_rowid();
        insert_event(
            transaction,
            file_id,
            Some(scan_id),
            "discovered",
            None,
            Some(json!({"file_size": metadata.file_size})),
        )?;
        MetadataOutcome::New
    };
    Ok(outcome)
}

fn metadata_matches(record: &FileRecord, metadata: &FileMetadata) -> bool {
    record.file_size == metadata.file_size
        && record.modified_at_ns == metadata.modified_at_ns
        && record.inode == metadata.inode
        && record.device_id == metadata.device_id
}

fn storage_object_key(
    transaction: &Transaction<'_>,
    volume_id: i64,
    metadata: &FileMetadata,
) -> Result<Option<String>> {
    let Some(physical_device_id) = transaction.query_row(
        "SELECT physical_device_id FROM volumes WHERE id = ?1",
        [volume_id],
        |row| row.get::<_, Option<i64>>(0),
    )?
    else {
        return Ok(None);
    };
    let (Some(device_id), Some(inode)) = (metadata.device_id, metadata.inode) else {
        return Ok(None);
    };
    Ok(Some(format!("v1:{physical_device_id}:{device_id}:{inode}")))
}

fn insert_event(
    transaction: &Transaction<'_>,
    file_copy_id: i64,
    scan_run_id: Option<i64>,
    event_type: &str,
    old_value: Option<serde_json::Value>,
    new_value: Option<serde_json::Value>,
) -> Result<()> {
    transaction.execute(
        "INSERT INTO file_events(file_copy_id, scan_run_id, event_type, old_value_json, new_value_json, occurred_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            file_copy_id,
            scan_run_id,
            event_type,
            old_value.map(|value| value.to_string()),
            new_value.map(|value| value.to_string()),
            now()
        ],
    )?;
    Ok(())
}

fn row_to_volume(row: &Row<'_>) -> rusqlite::Result<Volume> {
    let identity_state: String = row.get(11)?;
    let role: String = row.get(12)?;
    Ok(Volume {
        id: row.get(0)?,
        volume_uid: row.get(1)?,
        marker_uid: row.get(2)?,
        volume_name: row.get(3)?,
        filesystem: row.get(4)?,
        mount_path: path_from_bytes(&row.get::<_, Vec<u8>>(5)?),
        system_volume_uuid: row.get(6)?,
        device_serial: row.get(7)?,
        partition_uuid: row.get(8)?,
        total_size: row.get(9)?,
        physical_device_id: row.get(10)?,
        identity_state: identity_state
            .parse()
            .unwrap_or(VolumeIdentityState::Fallback),
        role: role.parse().unwrap_or(VolumeRole::Unknown),
        is_online: row.get::<_, i64>(13)? != 0,
        first_seen_at: row.get(14)?,
        last_seen_at: row.get(15)?,
    })
}

fn row_to_physical_device(row: &Row<'_>) -> rusqlite::Result<PhysicalDevice> {
    Ok(PhysicalDevice {
        id: row.get(0)?,
        stable_uid: row.get(1)?,
        media_uuid: row.get(2)?,
        device_serial: row.get(3)?,
        model: row.get(4)?,
        transport: row.get(5)?,
        total_size: row.get(6)?,
        first_seen_at: row.get(7)?,
        last_seen_at: row.get(8)?,
    })
}

fn row_to_volume_conflict(row: &Row<'_>) -> rusqlite::Result<VolumeIdentityConflict> {
    Ok(VolumeIdentityConflict {
        id: row.get(0)?,
        existing_volume_id: row.get(1)?,
        existing_volume_uid: row.get(2)?,
        candidate_marker_uid: row.get(3)?,
        candidate_mount_path: path_from_bytes(&row.get::<_, Vec<u8>>(4)?),
        candidate_filesystem: row.get(5)?,
        candidate_system_volume_uuid: row.get(6)?,
        candidate_partition_uuid: row.get(7)?,
        candidate_media_uuid: row.get(8)?,
        candidate_device_serial: row.get(9)?,
        candidate_total_size: row.get(10)?,
        candidate_physical_device_id: row.get(11)?,
        state: row.get(12)?,
        resolution: row.get(13)?,
        resolved_volume_id: row.get(14)?,
        detected_at: row.get(15)?,
        resolved_at: row.get(16)?,
    })
}

fn file_record_select(where_clause: &str) -> String {
    format!(
        "SELECT f.id, f.volume_id, v.volume_uid, v.volume_name, v.role, v.is_online, v.physical_device_id,
                f.relative_path, f.file_size, f.modified_at_ns, f.created_at_ns, f.inode, f.device_id,
                f.storage_object_key, f.link_group_id, f.content_id, f.sample_hash, f.full_hash,
                f.hash_state, f.status, f.last_error, f.first_seen_at, f.last_seen_at, f.last_verified_at
         FROM file_copies f JOIN volumes v ON v.id = f.volume_id {where_clause}"
    )
}

fn row_to_file_record(row: &Row<'_>) -> rusqlite::Result<FileRecord> {
    let role: String = row.get(4)?;
    Ok(FileRecord {
        id: row.get(0)?,
        volume_id: row.get(1)?,
        volume_uid: row.get(2)?,
        volume_name: row.get(3)?,
        volume_role: role.parse().unwrap_or(VolumeRole::Unknown),
        volume_online: row.get::<_, i64>(5)? != 0,
        physical_device_id: row.get(6)?,
        relative_path: path_from_bytes(&row.get::<_, Vec<u8>>(7)?),
        file_size: row.get(8)?,
        modified_at_ns: row.get(9)?,
        created_at_ns: row.get(10)?,
        inode: row.get(11)?,
        device_id: row.get(12)?,
        storage_object_key: row.get(13)?,
        link_group_id: row.get(14)?,
        content_id: row.get(15)?,
        sample_hash: row.get(16)?,
        full_hash: row.get(17)?,
        hash_state: row.get(18)?,
        status: row.get(19)?,
        last_error: row.get(20)?,
        first_seen_at: row.get(21)?,
        last_seen_at: row.get(22)?,
        last_verified_at: row.get(23)?,
    })
}

pub fn safe_relative_path(root: &Path, path: &Path) -> Result<PathBuf> {
    let relative = path.strip_prefix(root).with_context(|| {
        format!(
            "路径 {} 不在扫描根目录 {} 下",
            path.display(),
            root.display()
        )
    })?;
    if relative.as_os_str().is_empty() || relative.is_absolute() {
        bail!("不安全的相对路径: {}", display_path(relative));
    }
    Ok(relative.to_path_buf())
}

#[must_use]
pub fn record_display_path(record: &FileRecord) -> String {
    display_bytes(&path_bytes(&record.relative_path))
}
