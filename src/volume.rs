//! 卷身份识别。卷名只用于展示，绝不作为身份主键。

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use uuid::Uuid;

use crate::db::{Database, PhysicalDeviceUpsert, VolumeConflictInput, VolumeUpsert};
use crate::model::{
    FileMetadata, PhysicalDeviceIdentityState, Volume, VolumeIdentityConflict, VolumeIdentityState,
    VolumeRole,
};
use crate::util::{display_path, path_bytes, to_ns};

pub const MARKER_FILE: &str = ".disk-indexer-volume-id";

#[derive(Debug, Clone, Copy)]
pub enum MarkerPolicy {
    WriteIfPossible,
    DoNotWrite,
}

#[derive(Debug, Clone)]
pub struct VolumeRegistration {
    /// 只有身份已安全确认时才会提供卷；`possible_clone` 时必须先人工处理冲突。
    pub volume: Option<Volume>,
    pub marker_uid: Option<String>,
    pub writable: bool,
    pub used_fallback_identity: bool,
    pub identity_state: VolumeIdentityState,
    pub conflict: Option<VolumeIdentityConflict>,
}

#[derive(Debug, Clone)]
pub struct VolumeIdentityInput {
    pub filesystem: String,
    pub system_volume_uuid: Option<String>,
    pub partition_uuid: Option<String>,
    /// 旧版本的卷级 media UUID；仅用于逻辑卷重连兼容，不能证明物理介质身份。
    pub media_uuid: Option<String>,
    /// 旧版本曾错误地将 DeviceIdentifier 写入此字段；不能用于物理设备计数。
    pub device_serial: Option<String>,
    pub whole_disk_identifier: Option<String>,
    pub whole_disk_media_uuid: Option<String>,
    pub hardware_serial: Option<String>,
    pub model: Option<String>,
    pub transport: Option<String>,
    pub total_size: Option<i64>,
}

pub fn register_volume(
    database: &mut Database,
    root: &Path,
    role: VolumeRole,
    marker_policy: MarkerPolicy,
) -> Result<VolumeRegistration> {
    register_volume_with_identity(database, root, role, marker_policy, None)
}

/// 注册卷时允许平台适配层显式提供身份信息；主要用于可靠的测试和未来平台集成。
/// 调用方不能藉此绕过冲突检查。
pub fn register_volume_with_identity(
    database: &mut Database,
    root: &Path,
    role: VolumeRole,
    marker_policy: MarkerPolicy,
    identity_override: Option<VolumeIdentityInput>,
) -> Result<VolumeRegistration> {
    let root = root
        .canonicalize()
        .with_context(|| format!("无法访问卷根目录 {}", root.display()))?;
    if !root.is_dir() {
        bail!("卷根目录不是目录: {}", root.display());
    }
    let marker_path = root.join(MARKER_FILE);
    let existing_marker = read_marker(&marker_path)?;
    let writable = is_writable(&root);
    let marker_uid = match (existing_marker, marker_policy, writable) {
        (Some(marker), _, _) => Some(marker),
        (None, MarkerPolicy::WriteIfPossible, true) => Some(write_marker(&marker_path)?),
        (None, _, _) => None,
    };
    let identity = identity_override.unwrap_or(volume_identity(&root)?);
    let physical_identity = physical_device_identity(&root, &identity);
    let physical_device = database.upsert_physical_device(PhysicalDeviceUpsert {
        stable_uid: &physical_identity.stable_uid,
        whole_disk_identifier: identity.whole_disk_identifier.as_deref(),
        whole_disk_media_uuid: identity.whole_disk_media_uuid.as_deref(),
        hardware_serial: identity.hardware_serial.as_deref(),
        identity_state: physical_identity.state,
        identity_source: physical_identity.source,
        media_uuid: identity.media_uuid.as_deref(),
        device_serial: identity.device_serial.as_deref(),
        model: identity.model.as_deref(),
        transport: identity.transport.as_deref(),
        total_size: identity.total_size,
    })?;
    let default_identity_state = if identity.has_stable_identifier() {
        VolumeIdentityState::Verified
    } else {
        VolumeIdentityState::Fallback
    };

    if let Some(marker) = marker_uid.as_deref() {
        let existing_volumes = database.volumes_by_marker(marker)?;
        if let Some(existing) = existing_volumes
            .iter()
            .find(|volume| volume.mount_path == root)
        {
            let volume = upsert_registered_volume(
                database,
                existing,
                &root,
                &identity,
                physical_device.id,
                role,
                default_identity_state,
            )?;
            return Ok(VolumeRegistration {
                volume: Some(volume),
                marker_uid,
                writable,
                used_fallback_identity: false,
                identity_state: default_identity_state,
                conflict: None,
            });
        }
        let matching = existing_volumes
            .iter()
            .filter(|volume| {
                identity_is_consistent(volume, &identity)
                    && physical_identity_is_consistent(database, volume, &identity)
            })
            .collect::<Vec<_>>();
        if matching.len() == 1 {
            let volume = upsert_registered_volume(
                database,
                matching[0],
                &root,
                &identity,
                physical_device.id,
                role,
                VolumeIdentityState::Verified,
            )?;
            database.record_volume_event(
                Some(volume.id),
                None,
                "volume_relinked_by_verified_identity",
                None,
                Some(serde_json::json!({"mount_path": display_path(&root)})),
            )?;
            return Ok(VolumeRegistration {
                volume: Some(volume),
                marker_uid,
                writable,
                used_fallback_identity: false,
                identity_state: VolumeIdentityState::Verified,
                conflict: None,
            });
        }
        if let Some(existing) = existing_volumes.first() {
            let conflict = database.record_volume_conflict(VolumeConflictInput {
                existing_volume_id: existing.id,
                candidate_marker_uid: Some(marker),
                candidate_root: &root,
                candidate_filesystem: Some(&identity.filesystem),
                candidate_system_volume_uuid: identity.system_volume_uuid.as_deref(),
                candidate_partition_uuid: identity.partition_uuid.as_deref(),
                candidate_media_uuid: identity.media_uuid.as_deref(),
                candidate_device_serial: identity.device_serial.as_deref(),
                candidate_total_size: identity.total_size,
                candidate_physical_device_id: Some(physical_device.id),
            })?;
            return Ok(VolumeRegistration {
                volume: None,
                marker_uid,
                writable,
                used_fallback_identity: false,
                identity_state: VolumeIdentityState::PossibleClone,
                conflict: Some(conflict),
            });
        }
    }

    let volume_uid = fallback_volume_uid(&root, &identity);
    let volume = database.upsert_volume(VolumeUpsert {
        volume_uid: &volume_uid,
        marker_uid: marker_uid.as_deref(),
        root: &root,
        filesystem: Some(&identity.filesystem),
        system_volume_uuid: identity.system_volume_uuid.as_deref(),
        device_serial: identity
            .device_serial
            .as_deref()
            .or(identity.media_uuid.as_deref()),
        partition_uuid: identity.partition_uuid.as_deref(),
        total_size: identity.total_size,
        physical_device_id: Some(physical_device.id),
        identity_state: default_identity_state,
        role,
    })?;
    Ok(VolumeRegistration {
        volume: Some(volume),
        marker_uid,
        writable,
        used_fallback_identity: true,
        identity_state: default_identity_state,
        conflict: None,
    })
}

fn upsert_registered_volume(
    database: &mut Database,
    existing: &Volume,
    root: &Path,
    identity: &VolumeIdentityInput,
    physical_device_id: i64,
    role: VolumeRole,
    identity_state: VolumeIdentityState,
) -> Result<Volume> {
    database.upsert_volume(VolumeUpsert {
        volume_uid: &existing.volume_uid,
        marker_uid: existing.marker_uid.as_deref(),
        root,
        filesystem: Some(&identity.filesystem),
        system_volume_uuid: identity.system_volume_uuid.as_deref(),
        device_serial: identity
            .device_serial
            .as_deref()
            .or(identity.media_uuid.as_deref()),
        partition_uuid: identity.partition_uuid.as_deref(),
        total_size: identity.total_size,
        physical_device_id: Some(physical_device_id),
        identity_state,
        role,
    })
}

/// 仅在稳定标识至少有一项一致、且没有任何稳定标识互相矛盾时返回 true。
fn identity_is_consistent(existing: &Volume, candidate: &VolumeIdentityInput) -> bool {
    let mut matched = false;
    for (existing_value, candidate_value) in [
        (&existing.system_volume_uuid, &candidate.system_volume_uuid),
        (&existing.partition_uuid, &candidate.partition_uuid),
    ] {
        if let (Some(existing_value), Some(candidate_value)) = (existing_value, candidate_value) {
            if existing_value != candidate_value {
                return false;
            }
            matched = true;
        }
    }
    if let Some(existing_device) = &existing.device_serial {
        let candidate_devices = [
            candidate.media_uuid.as_deref(),
            candidate.device_serial.as_deref(),
        ];
        if candidate_devices
            .iter()
            .any(|value| *value == Some(existing_device))
        {
            matched = true;
        } else if candidate_devices.iter().any(Option::is_some) {
            return false;
        }
    }
    matched
}

/// 逻辑卷 UUID 可以帮助重连，但克隆的逻辑卷可能带有同一 UUID。历史设备已经有可信
/// 整盘级身份时，新挂载点也必须没有与该身份冲突的证据。
fn physical_identity_is_consistent(
    database: &Database,
    existing: &Volume,
    candidate: &VolumeIdentityInput,
) -> bool {
    let Some(physical_device_id) = existing.physical_device_id else {
        return true;
    };
    let Ok(device) = database.physical_device_by_id(physical_device_id) else {
        return false;
    };
    if !device.identity_state.is_verified() {
        return true;
    }
    for (existing_value, candidate_value) in [
        (
            device.hardware_serial.as_deref(),
            candidate.hardware_serial.as_deref(),
        ),
        (
            device.whole_disk_media_uuid.as_deref(),
            candidate.whole_disk_media_uuid.as_deref(),
        ),
    ] {
        if let (Some(existing_value), Some(candidate_value)) = (existing_value, candidate_value) {
            if existing_value != candidate_value {
                return false;
            }
        }
    }
    true
}

/// 显式重连也必须证明同一稳定身份；不能凭 marker 或路径强行覆盖历史卷。
pub fn relink_volume(database: &mut Database, volume_id: i64, root: &Path) -> Result<Volume> {
    let root = root
        .canonicalize()
        .with_context(|| format!("无法访问卷根目录 {}", root.display()))?;
    if !root.is_dir() {
        bail!("卷根目录不是目录: {}", root.display());
    }
    let existing = database.volume_by_id(volume_id)?;
    let candidate_marker = read_marker(&root.join(MARKER_FILE))?;
    if existing.marker_uid.is_some() && candidate_marker != existing.marker_uid {
        bail!("重连拒绝：目标路径的 marker 与历史卷不一致");
    }
    let identity = volume_identity(&root)?;
    if !identity_is_consistent(&existing, &identity)
        || !physical_identity_is_consistent(database, &existing, &identity)
    {
        bail!("重连拒绝：目标路径没有可验证的一致稳定设备身份");
    }
    let physical_identity = physical_device_identity(&root, &identity);
    let physical_device = database.upsert_physical_device(PhysicalDeviceUpsert {
        stable_uid: &physical_identity.stable_uid,
        whole_disk_identifier: identity.whole_disk_identifier.as_deref(),
        whole_disk_media_uuid: identity.whole_disk_media_uuid.as_deref(),
        hardware_serial: identity.hardware_serial.as_deref(),
        identity_state: physical_identity.state,
        identity_source: physical_identity.source,
        media_uuid: identity.media_uuid.as_deref(),
        device_serial: identity.device_serial.as_deref(),
        model: identity.model.as_deref(),
        transport: identity.transport.as_deref(),
        total_size: identity.total_size,
    })?;
    let previous_path = display_path(&existing.mount_path);
    let relinked = upsert_registered_volume(
        database,
        &existing,
        &root,
        &identity,
        physical_device.id,
        existing.role,
        VolumeIdentityState::ManualLink,
    )?;
    database.record_volume_event(
        Some(relinked.id),
        None,
        "volume_relinked_manually",
        Some(serde_json::json!({"mount_path": previous_path})),
        Some(serde_json::json!({"mount_path": display_path(&root)})),
    )?;
    Ok(relinked)
}

pub fn resolve_conflict_as_new_volume(
    database: &mut Database,
    conflict_id: i64,
    role: VolumeRole,
) -> Result<Volume> {
    database.resolve_conflict_as_new_volume(
        conflict_id,
        &format!("manual:clone:{}", Uuid::new_v4()),
        role,
    )
}

pub fn file_metadata(root: &Path, path: &Path) -> Result<FileMetadata> {
    let metadata =
        fs::metadata(path).with_context(|| format!("无法读取文件元数据 {}", path.display()))?;
    let relative_path = crate::db::safe_relative_path(root, path)?;
    let filename = path
        .file_name()
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("文件没有名称: {}", display_path(path)))?;
    #[cfg(unix)]
    let (inode, device_id) = {
        use std::os::unix::fs::MetadataExt;
        (
            i64::try_from(metadata.ino()).ok(),
            i64::try_from(metadata.dev()).ok(),
        )
    };
    #[cfg(not(unix))]
    let (inode, device_id) = (None, None);
    Ok(FileMetadata {
        relative_path,
        filename,
        file_size: metadata.len(),
        modified_at_ns: to_ns(metadata.modified()),
        created_at_ns: to_ns(metadata.created()),
        inode,
        device_id,
    })
}

pub fn metadata_still_matches(path: &Path, expected: &FileMetadata) -> Result<bool> {
    let actual = fs::metadata(path)
        .with_context(|| format!("哈希后无法重新读取文件元数据 {}", path.display()))?;
    let modified_at_ns = to_ns(actual.modified());
    #[cfg(unix)]
    let (inode, device_id) = {
        use std::os::unix::fs::MetadataExt;
        (
            i64::try_from(actual.ino()).ok(),
            i64::try_from(actual.dev()).ok(),
        )
    };
    #[cfg(not(unix))]
    let (inode, device_id) = (None, None);
    Ok(actual.len() == expected.file_size
        && modified_at_ns == expected.modified_at_ns
        && inode == expected.inode
        && device_id == expected.device_id)
}

fn read_marker(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(value) => {
            let marker = value.trim();
            Uuid::parse_str(marker)
                .with_context(|| format!("卷标记文件 {} 不包含有效 UUID", path.display()))?;
            Ok(Some(marker.to_owned()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("无法读取卷标记 {}", path.display())),
    }
}

fn write_marker(path: &Path) -> Result<String> {
    let marker = Uuid::new_v4().to_string();
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("无法创建卷标记 {}", path.display()))?;
    file.write_all(marker.as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(marker)
}

fn is_writable(root: &Path) -> bool {
    let probe = root.join(format!(".{MARKER_FILE}.write-probe-{}", Uuid::new_v4()));
    match OpenOptions::new().write(true).create_new(true).open(&probe) {
        Ok(file) => {
            drop(file);
            fs::remove_file(probe).is_ok()
        }
        Err(_) => false,
    }
}

impl VolumeIdentityInput {
    fn has_stable_identifier(&self) -> bool {
        self.system_volume_uuid.is_some()
            || self.partition_uuid.is_some()
            || self.media_uuid.is_some()
            || self.device_serial.is_some()
            || self.whole_disk_media_uuid.is_some()
            || self.hardware_serial.is_some()
    }
}

fn volume_identity(root: &Path) -> Result<VolumeIdentityInput> {
    let metadata =
        fs::metadata(root).with_context(|| format!("无法读取卷根目录元数据 {}", root.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let fallback = format!("unix-device:{}", metadata.dev());
        #[cfg(target_os = "macos")]
        if let Some(identity) = macos_volume_identity(root, fallback.clone()) {
            return Ok(identity);
        }
        Ok(VolumeIdentityInput {
            filesystem: fallback,
            system_volume_uuid: None,
            partition_uuid: None,
            media_uuid: None,
            device_serial: None,
            whole_disk_identifier: None,
            whole_disk_media_uuid: None,
            hardware_serial: None,
            model: None,
            transport: None,
            total_size: None,
        })
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        Ok(VolumeIdentityInput {
            filesystem: "platform-device:unknown".to_owned(),
            system_volume_uuid: None,
            partition_uuid: None,
            media_uuid: None,
            device_serial: None,
            whole_disk_identifier: None,
            whole_disk_media_uuid: None,
            hardware_serial: None,
            model: None,
            transport: None,
            total_size: None,
        })
    }
}

fn fallback_volume_uid(root: &Path, identity: &VolumeIdentityInput) -> String {
    let mut input = Vec::new();
    let has_stable_platform_identity = identity.system_volume_uuid.is_some()
        || identity.partition_uuid.is_some()
        || identity.media_uuid.is_some()
        || identity.device_serial.is_some()
        || identity.whole_disk_media_uuid.is_some()
        || identity.hardware_serial.is_some();
    if let Some(uuid) = &identity.system_volume_uuid {
        input.extend_from_slice(uuid.as_bytes());
        input.push(0);
    }
    if let Some(uuid) = &identity.partition_uuid {
        input.extend_from_slice(uuid.as_bytes());
        input.push(0);
    }
    if let Some(serial) = &identity.device_serial {
        input.extend_from_slice(serial.as_bytes());
        input.push(0);
    }
    if let Some(media_uuid) = &identity.media_uuid {
        input.extend_from_slice(media_uuid.as_bytes());
        input.push(0);
    }
    if let Some(media_uuid) = &identity.whole_disk_media_uuid {
        input.extend_from_slice(media_uuid.as_bytes());
        input.push(0);
    }
    if let Some(serial) = &identity.hardware_serial {
        input.extend_from_slice(serial.as_bytes());
        input.push(0);
    }
    input.extend_from_slice(identity.filesystem.as_bytes());
    input.push(0);
    input.extend_from_slice(&identity.total_size.unwrap_or_default().to_le_bytes());
    // 没有任何系统稳定标识时才纳入路径，避免把不同根误合并；有 UUID 时重挂载路径不影响 UID。
    if !has_stable_platform_identity {
        input.extend_from_slice(&path_bytes(root));
    }
    format!("fallback:v1:{}", blake3::hash(&input).to_hex())
}

struct PhysicalDeviceIdentity {
    stable_uid: String,
    state: PhysicalDeviceIdentityState,
    source: &'static str,
}

fn physical_device_identity(root: &Path, identity: &VolumeIdentityInput) -> PhysicalDeviceIdentity {
    let mut input = Vec::new();
    if let Some(serial) = &identity.hardware_serial {
        input.extend_from_slice(b"hardware-serial:");
        input.extend_from_slice(serial.as_bytes());
        return PhysicalDeviceIdentity {
            stable_uid: format!("physical:v2:{}", blake3::hash(&input).to_hex()),
            state: PhysicalDeviceIdentityState::Verified,
            source: "hardware_serial",
        };
    }
    if let Some(media_uuid) = &identity.whole_disk_media_uuid {
        input.extend_from_slice(b"whole-disk-media-uuid:");
        input.extend_from_slice(media_uuid.as_bytes());
        return PhysicalDeviceIdentity {
            stable_uid: format!("physical:v2:{}", blake3::hash(&input).to_hex()),
            state: PhysicalDeviceIdentityState::Verified,
            source: "whole_disk_media_uuid",
        };
    }
    if let Some(identifier) = &identity.whole_disk_identifier {
        input.extend_from_slice(b"whole-disk-identifier:");
        input.extend_from_slice(identifier.as_bytes());
        return PhysicalDeviceIdentity {
            stable_uid: format!("physical:inferred:v2:{}", blake3::hash(&input).to_hex()),
            state: PhysicalDeviceIdentityState::Inferred,
            source: "whole_disk_identifier",
        };
    }
    // 未知记录只用于保存观察历史，绝不作为安全计数。路径仅用于避免把所有未知卷合并为
    // 同一条数据库记录，不能被解释为物理介质独立性。
    input.extend_from_slice(identity.filesystem.as_bytes());
    input.push(0);
    input.extend_from_slice(&identity.total_size.unwrap_or_default().to_le_bytes());
    input.push(0);
    input.extend_from_slice(&path_bytes(root));
    PhysicalDeviceIdentity {
        stable_uid: format!("physical:unknown:v2:{}", blake3::hash(&input).to_hex()),
        state: PhysicalDeviceIdentityState::Unknown,
        source: "fallback",
    }
}

#[cfg(target_os = "macos")]
fn macos_volume_identity(root: &Path, fallback_filesystem: String) -> Option<VolumeIdentityInput> {
    let plist = diskutil_info_plist(root)?;
    let filesystem = plist_value(&plist, "FilesystemType")
        .or_else(|| plist_value(&plist, "FilesystemName"))
        .unwrap_or(fallback_filesystem);
    let current_identifier = plist_value(&plist, "DeviceIdentifier");
    let whole_disk_identifier = plist_value(&plist, "ParentWholeDisk").or_else(|| {
        plist_bool(&plist, "Whole")
            .filter(|whole| *whole)
            .and(current_identifier.clone())
    });
    let whole_plist = whole_disk_identifier
        .as_deref()
        .and_then(|identifier| diskutil_info_plist(Path::new(identifier)));
    let whole = whole_plist.as_deref();
    Some(VolumeIdentityInput {
        filesystem,
        system_volume_uuid: plist_value(&plist, "VolumeUUID"),
        partition_uuid: plist_value(&plist, "DiskUUID"),
        media_uuid: plist_value(&plist, "MediaUUID"),
        // `DeviceIdentifier` 可能是 disk4s2，只能描述分区，故不写入旧 serial 字段。
        device_serial: None,
        whole_disk_identifier,
        whole_disk_media_uuid: whole.and_then(|value| plist_value(value, "MediaUUID")),
        hardware_serial: whole.and_then(plist_hardware_serial),
        model: whole
            .and_then(|value| plist_value(value, "MediaName"))
            .or_else(|| whole.and_then(|value| plist_value(value, "DeviceModel"))),
        transport: whole
            .and_then(|value| plist_value(value, "BusProtocol"))
            .or_else(|| plist_value(&plist, "BusProtocol")),
        total_size: whole
            .and_then(|value| plist_value(value, "TotalSize"))
            .or_else(|| plist_value(&plist, "TotalSize"))
            .and_then(|value| value.parse().ok()),
    })
}

#[cfg(target_os = "macos")]
fn diskutil_info_plist(target: &Path) -> Option<String> {
    let output = std::process::Command::new("diskutil")
        .args(["info", "-plist"])
        .arg(target)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).ok())
        .flatten()
}

#[cfg(target_os = "macos")]
fn plist_hardware_serial(xml: &str) -> Option<String> {
    ["DeviceSerial", "SerialNumber", "MediaSerialNumber"]
        .into_iter()
        .find_map(|key| plist_value(xml, key))
}

#[cfg(target_os = "macos")]
fn plist_value(xml: &str, key: &str) -> Option<String> {
    let marker = format!("<key>{key}</key>");
    let after_key = xml.split_once(&marker)?.1;
    let after_open = after_key
        .trim_start()
        .strip_prefix("<string>")
        .or_else(|| after_key.trim_start().strip_prefix("<integer>"))?;
    after_open
        .split_once('<')
        .map(|(value, _)| value.to_owned())
}

#[cfg(target_os = "macos")]
fn plist_bool(xml: &str, key: &str) -> Option<bool> {
    let marker = format!("<key>{key}</key>");
    let value = xml.split_once(&marker)?.1.trim_start();
    if value.starts_with("<true/>") {
        Some(true)
    } else if value.starts_with("<false/>") {
        Some(false)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{MARKER_FILE, file_metadata, metadata_still_matches, read_marker};

    #[test]
    fn marker_rejects_invalid_uuid() {
        let dir = tempdir().expect("temp directory");
        fs::write(dir.path().join(MARKER_FILE), "not-a-uuid\n").expect("marker write");
        assert!(read_marker(&dir.path().join(MARKER_FILE)).is_err());
    }

    #[test]
    fn detects_file_change_between_pre_and_post_hash_metadata() {
        let dir = tempdir().expect("temp directory");
        let path = dir.path().join("file");
        fs::write(&path, b"before").expect("initial file");
        let before = file_metadata(dir.path(), &path).expect("before metadata");
        fs::write(&path, b"after-content-changed").expect("change file");
        assert!(!metadata_still_matches(&path, &before).expect("after metadata"));
    }
}
