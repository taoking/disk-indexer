use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VolumeRole {
    Primary,
    LocalBackup,
    OffsiteBackup,
    Temporary,
    LegacyBackup,
    Unknown,
}

impl VolumeRole {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::LocalBackup => "local_backup",
            Self::OffsiteBackup => "offsite_backup",
            Self::Temporary => "temporary",
            Self::LegacyBackup => "legacy_backup",
            Self::Unknown => "unknown",
        }
    }
}

impl std::str::FromStr for VolumeRole {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "primary" => Ok(Self::Primary),
            "local_backup" => Ok(Self::LocalBackup),
            "offsite_backup" => Ok(Self::OffsiteBackup),
            "temporary" => Ok(Self::Temporary),
            "legacy_backup" => Ok(Self::LegacyBackup),
            "unknown" => Ok(Self::Unknown),
            _ => Err(format!("不支持的卷角色: {value}")),
        }
    }
}

/// 卷身份判定的可信程度。`possible_clone` 绝不参与自动合并。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VolumeIdentityState {
    Verified,
    Fallback,
    PossibleClone,
    Conflict,
    ManualLink,
}

impl VolumeIdentityState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::Fallback => "fallback",
            Self::PossibleClone => "possible_clone",
            Self::Conflict => "conflict",
            Self::ManualLink => "manual_link",
        }
    }
}

impl std::str::FromStr for VolumeIdentityState {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "verified" => Ok(Self::Verified),
            "fallback" => Ok(Self::Fallback),
            "possible_clone" => Ok(Self::PossibleClone),
            "conflict" => Ok(Self::Conflict),
            "manual_link" => Ok(Self::ManualLink),
            _ => Err(format!("不支持的卷身份状态: {value}")),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Volume {
    pub id: i64,
    pub volume_uid: String,
    pub marker_uid: Option<String>,
    pub volume_name: String,
    pub filesystem: Option<String>,
    pub mount_path: PathBuf,
    pub system_volume_uuid: Option<String>,
    pub device_serial: Option<String>,
    pub partition_uuid: Option<String>,
    pub total_size: Option<i64>,
    pub physical_device_id: Option<i64>,
    pub identity_state: VolumeIdentityState,
    pub role: VolumeRole,
    pub is_online: bool,
    pub first_seen_at: String,
    pub last_seen_at: String,
}

#[derive(Debug, Clone)]
pub struct PhysicalDevice {
    pub id: i64,
    pub stable_uid: String,
    pub media_uuid: Option<String>,
    pub device_serial: Option<String>,
    pub model: Option<String>,
    pub transport: Option<String>,
    pub total_size: Option<i64>,
    pub first_seen_at: String,
    pub last_seen_at: String,
}

#[derive(Debug, Clone)]
pub struct VolumeIdentityConflict {
    pub id: i64,
    pub existing_volume_id: i64,
    pub existing_volume_uid: String,
    pub candidate_marker_uid: Option<String>,
    pub candidate_mount_path: PathBuf,
    pub candidate_filesystem: Option<String>,
    pub candidate_system_volume_uuid: Option<String>,
    pub candidate_partition_uuid: Option<String>,
    pub candidate_media_uuid: Option<String>,
    pub candidate_device_serial: Option<String>,
    pub candidate_total_size: Option<i64>,
    pub candidate_physical_device_id: Option<i64>,
    pub state: String,
    pub resolution: Option<String>,
    pub resolved_volume_id: Option<i64>,
    pub detected_at: String,
    pub resolved_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FileRecord {
    pub id: i64,
    pub volume_id: i64,
    pub volume_uid: String,
    pub volume_name: String,
    pub volume_role: VolumeRole,
    pub volume_online: bool,
    pub physical_device_id: Option<i64>,
    pub relative_path: PathBuf,
    pub file_size: u64,
    pub modified_at_ns: Option<i64>,
    pub created_at_ns: Option<i64>,
    pub inode: Option<i64>,
    pub device_id: Option<i64>,
    pub storage_object_key: Option<String>,
    pub link_group_id: Option<String>,
    pub content_id: Option<i64>,
    pub sample_hash: Option<String>,
    pub full_hash: Option<String>,
    pub hash_state: String,
    pub status: String,
    pub last_error: Option<String>,
    pub first_seen_at: String,
    pub last_seen_at: String,
    pub last_verified_at: Option<String>,
}

impl FileRecord {
    #[must_use]
    pub fn absolute_path(&self, mount_path: &std::path::Path) -> PathBuf {
        mount_path.join(&self.relative_path)
    }
}

#[derive(Debug, Clone)]
pub struct FileMetadata {
    pub relative_path: PathBuf,
    pub filename: PathBuf,
    pub file_size: u64,
    pub modified_at_ns: Option<i64>,
    pub created_at_ns: Option<i64>,
    pub inode: Option<i64>,
    pub device_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScanSummary {
    pub scan_id: i64,
    pub status: String,
    pub discovered_count: u64,
    pub metadata_reused_count: u64,
    pub sampled_count: u64,
    pub full_hashed_count: u64,
    pub skipped_count: u64,
    pub missing_count: u64,
    pub error_count: u64,
    pub bytes_read: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CopyView {
    pub file_copy_id: i64,
    pub volume_id: i64,
    pub volume_uid: String,
    pub volume: String,
    pub role: String,
    pub path: String,
    pub status: String,
    pub is_online: bool,
    pub physical_device_id: Option<i64>,
    pub storage_object_key: Option<String>,
    pub link_group_id: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DuplicateGroup {
    pub full_hash: String,
    pub file_size: u64,
    pub known_copies: usize,
    pub online_copies: usize,
    pub offline_copies: usize,
    pub missing_copies: usize,
    pub path_count: usize,
    pub storage_object_count: usize,
    pub logical_volume_count: usize,
    pub physical_device_count: usize,
    pub theoretical_reclaimable_bytes: u64,
    pub copies: Vec<CopyView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LookupResult {
    pub path: String,
    pub file_size: u64,
    pub sample_hash: Option<String>,
    pub full_hash: Option<String>,
    pub hash_state: String,
    pub exact: bool,
    pub cache_state: String,
    pub metadata_matches_index: bool,
    pub requires_rehash: bool,
    pub has_appeared_before: bool,
    pub known_copies: usize,
    pub online_copies: usize,
    pub offline_copies: usize,
    pub copies: Vec<CopyView>,
    pub first_seen_at: Option<String>,
    pub last_seen_at: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CleanupPlan {
    pub schema_version: i64,
    pub generated_at: String,
    pub database_path: String,
    pub target_volume_id: i64,
    pub keep_volume_id: i64,
    pub min_remaining_copies: usize,
    pub min_remaining_physical_devices: usize,
    pub verification_mode: String,
    pub verified_at: Option<String>,
    pub warnings: Vec<String>,
    pub items: Vec<CleanupPlanItem>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CleanupPlanItem {
    pub full_hash: String,
    pub file_size: u64,
    pub candidate_delete: CopyView,
    pub keep_candidates: Vec<CopyView>,
    pub all_known_copies: Vec<CopyView>,
    pub generated_at: String,
    pub database_schema_version: i64,
    pub status: String,
    pub verification_mode: String,
    pub verified_at: Option<String>,
    pub remaining_path_copies: usize,
    pub remaining_storage_objects: usize,
    pub remaining_volumes: usize,
    pub remaining_physical_devices: usize,
    pub candidate_metadata_state: String,
    pub keeper_metadata_state: String,
    pub blocked_reasons: Vec<String>,
    pub risk_notice: String,
}
