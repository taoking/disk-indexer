//! 卷身份识别。卷名只用于展示，绝不作为身份主键。

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use uuid::Uuid;

use crate::db::{Database, VolumeUpsert};
use crate::model::{FileMetadata, Volume, VolumeRole};
use crate::util::{display_path, path_bytes, to_ns};

pub const MARKER_FILE: &str = ".disk-indexer-volume-id";

#[derive(Debug, Clone, Copy)]
pub enum MarkerPolicy {
    WriteIfPossible,
    DoNotWrite,
}

#[derive(Debug, Clone)]
pub struct VolumeRegistration {
    pub volume: Volume,
    pub marker_uid: Option<String>,
    pub writable: bool,
    pub used_fallback_identity: bool,
}

#[derive(Debug, Clone)]
struct VolumeIdentity {
    filesystem: String,
    system_volume_uuid: Option<String>,
    device_serial: Option<String>,
    partition_uuid: Option<String>,
    total_size: Option<i64>,
}

pub fn register_volume(
    database: &mut Database,
    root: &Path,
    role: VolumeRole,
    marker_policy: MarkerPolicy,
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
    let identity = volume_identity(&root)?;
    let volume_uid = marker_uid.as_ref().map_or_else(
        || fallback_volume_uid(&root, &identity),
        |marker| format!("marker:{marker}"),
    );
    if let Some(existing) = database.volume_by_uid(&volume_uid)? {
        // 两处仍在线的不同根目录却持有同一标记是高风险冲突；禁止静默合并。
        if existing.mount_path != root && existing.mount_path.is_dir() {
            bail!(
                "卷身份冲突：{} 与 {} 同时使用 volume UID {}；请不要复制标记文件",
                existing.mount_path.display(),
                root.display(),
                volume_uid
            );
        }
    }
    let volume = database.upsert_volume(VolumeUpsert {
        volume_uid: &volume_uid,
        marker_uid: marker_uid.as_deref(),
        root: &root,
        filesystem: Some(&identity.filesystem),
        system_volume_uuid: identity.system_volume_uuid.as_deref(),
        device_serial: identity.device_serial.as_deref(),
        partition_uuid: identity.partition_uuid.as_deref(),
        total_size: identity.total_size,
        role,
    })?;
    Ok(VolumeRegistration {
        volume,
        marker_uid,
        writable,
        used_fallback_identity: volume_uid.starts_with("fallback:"),
    })
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

fn volume_identity(root: &Path) -> Result<VolumeIdentity> {
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
        Ok(VolumeIdentity {
            filesystem: fallback,
            system_volume_uuid: None,
            device_serial: None,
            partition_uuid: None,
            total_size: None,
        })
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        Ok(VolumeIdentity {
            filesystem: "platform-device:unknown".to_owned(),
            system_volume_uuid: None,
            device_serial: None,
            partition_uuid: None,
            total_size: None,
        })
    }
}

fn fallback_volume_uid(root: &Path, identity: &VolumeIdentity) -> String {
    let mut input = Vec::new();
    let has_stable_platform_identity = identity.system_volume_uuid.is_some()
        || identity.partition_uuid.is_some()
        || identity.device_serial.is_some();
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
    input.extend_from_slice(identity.filesystem.as_bytes());
    input.push(0);
    input.extend_from_slice(&identity.total_size.unwrap_or_default().to_le_bytes());
    // 没有任何系统稳定标识时才纳入路径，避免把不同根误合并；有 UUID 时重挂载路径不影响 UID。
    if !has_stable_platform_identity {
        input.extend_from_slice(&path_bytes(root));
    }
    format!("fallback:v1:{}", blake3::hash(&input).to_hex())
}

#[cfg(target_os = "macos")]
fn macos_volume_identity(root: &Path, fallback_filesystem: String) -> Option<VolumeIdentity> {
    let output = std::process::Command::new("diskutil")
        .args(["info", "-plist"])
        .arg(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let plist = String::from_utf8(output.stdout).ok()?;
    let filesystem = plist_value(&plist, "FilesystemType")
        .or_else(|| plist_value(&plist, "FilesystemName"))
        .unwrap_or(fallback_filesystem);
    Some(VolumeIdentity {
        filesystem,
        system_volume_uuid: plist_value(&plist, "VolumeUUID"),
        device_serial: plist_value(&plist, "MediaUUID"),
        partition_uuid: plist_value(&plist, "DiskUUID"),
        total_size: plist_value(&plist, "TotalSize").and_then(|value| value.parse().ok()),
    })
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
