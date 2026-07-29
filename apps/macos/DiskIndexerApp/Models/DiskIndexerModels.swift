import Foundation

struct VolumeSummary: Codable, Identifiable, Hashable {
    let id: Int
    let volumeUID: String
    let markerUID: String?
    let volumeName: String
    let filesystem: String?
    let mountPath: String
    let role: String
    let isOnline: Bool
    let identityState: String
    let physicalDeviceID: Int?
    let systemVolumeUUID: String?
    let deviceSerial: String?
    let totalSize: Int?
    let lastSeenAt: String

    enum CodingKeys: String, CodingKey {
        case id
        case volumeUID = "volume_uid"
        case markerUID = "marker_uid"
        case volumeName = "volume_name"
        case filesystem
        case mountPath = "mount_path"
        case role
        case isOnline = "is_online"
        case identityState = "identity_state"
        case physicalDeviceID = "physical_device_id"
        case systemVolumeUUID = "system_volume_uuid"
        case deviceSerial = "device_serial"
        case totalSize = "total_size"
        case lastSeenAt = "last_seen_at"
    }
}

struct VolumeAddResponse: Codable {
    let status: String
    let volume: VolumeSummary?
    let identityConflict: VolumeConflict?

    enum CodingKeys: String, CodingKey {
        case status
        case volume
        case identityConflict = "identity_conflict"
    }
}

struct VolumeConflict: Codable, Identifiable {
    let id: Int
    let existingVolumeID: Int
    let candidateMountPath: String

    enum CodingKeys: String, CodingKey {
        case id
        case existingVolumeID = "existing_volume_id"
        case candidateMountPath = "candidate_mount_path"
    }
}

struct TaskRecord: Codable, Identifiable, Hashable {
    let id: Int
    let taskUID: String
    let operation: String
    let status: String
    let startedAt: String
    let finishedAt: String?

    enum CodingKeys: String, CodingKey {
        case id
        case taskUID = "task_uid"
        case operation
        case status
        case startedAt = "started_at"
        case finishedAt = "finished_at"
    }
}

struct DashboardStats: Codable, Hashable {
    let schemaVersion: Int
    let registeredVolumes: Int
    let onlineVolumes: Int
    let offlineVolumes: Int
    let indexedFiles: Int
    let fullHashedFiles: Int
    let trustedDuplicateGroups: Int
    let verifiableOnlineCopies: Int
    let theoreticalReclaimableBytes: Int64

    enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case registeredVolumes = "registered_volumes"
        case onlineVolumes = "online_volumes"
        case offlineVolumes = "offline_volumes"
        case indexedFiles = "indexed_files"
        case fullHashedFiles = "full_hashed_files"
        case trustedDuplicateGroups = "trusted_duplicate_groups"
        case verifiableOnlineCopies = "verifiable_online_copies"
        case theoreticalReclaimableBytes = "theoretical_reclaimable_bytes"
    }
}

struct CopyViewModel: Codable, Identifiable, Hashable {
    let id: Int
    let volumeID: Int
    let volume: String
    let role: String
    let path: String
    let status: String
    let isOnline: Bool
    let physicalDeviceID: Int?
    let storageObjectKey: String?
    let linkGroupID: String?
    let lastError: String?

    enum CodingKeys: String, CodingKey {
        case id = "file_copy_id"
        case volumeID = "volume_id"
        case volume
        case role
        case path
        case status
        case isOnline = "is_online"
        case physicalDeviceID = "physical_device_id"
        case storageObjectKey = "storage_object_key"
        case linkGroupID = "link_group_id"
        case lastError = "last_error"
    }
}

struct DuplicateGroupModel: Codable, Identifiable, Hashable {
    let fullHash: String
    let fileSize: UInt64
    let knownCopies: Int
    let onlineCopies: Int
    let offlineCopies: Int
    let missingCopies: Int
    let pathCount: Int
    let storageObjectCount: Int
    let logicalVolumeCount: Int
    let physicalDeviceCount: Int
    let theoreticalReclaimableBytes: UInt64
    let copies: [CopyViewModel]

    var id: String { fullHash }

    enum CodingKeys: String, CodingKey {
        case fullHash = "full_hash"
        case fileSize = "file_size"
        case knownCopies = "known_copies"
        case onlineCopies = "online_copies"
        case offlineCopies = "offline_copies"
        case missingCopies = "missing_copies"
        case pathCount = "path_count"
        case storageObjectCount = "storage_object_count"
        case logicalVolumeCount = "logical_volume_count"
        case physicalDeviceCount = "physical_device_count"
        case theoreticalReclaimableBytes = "theoretical_reclaimable_bytes"
        case copies
    }
}

struct DuplicateGroupsPage: Codable {
    let groups: [DuplicateGroupModel]
    let nextAfterContentID: Int?

    enum CodingKeys: String, CodingKey {
        case groups
        case nextAfterContentID = "next_after_content_id"
    }
}

struct LookupResultModel: Codable {
    let path: String
    let fileSize: UInt64
    let fullHash: String?
    let hashState: String
    let exact: Bool
    let cacheState: String
    let metadataMatchesIndex: Bool
    let requiresRehash: Bool
    let hasAppearedBefore: Bool
    let knownCopies: Int
    let onlineCopies: Int
    let offlineCopies: Int
    let copies: [CopyViewModel]
    let firstSeenAt: String?
    let lastSeenAt: String?
    let warnings: [String]

    enum CodingKeys: String, CodingKey {
        case path
        case fileSize = "file_size"
        case fullHash = "full_hash"
        case hashState = "hash_state"
        case exact
        case cacheState = "cache_state"
        case metadataMatchesIndex = "metadata_matches_index"
        case requiresRehash = "requires_rehash"
        case hasAppearedBefore = "has_appeared_before"
        case knownCopies = "known_copies"
        case onlineCopies = "online_copies"
        case offlineCopies = "offline_copies"
        case copies
        case firstSeenAt = "first_seen_at"
        case lastSeenAt = "last_seen_at"
        case warnings
    }
}

struct CleanupPlanModel: Codable {
    let generatedAt: String
    let targetVolumeID: Int
    let keepVolumeID: Int
    let verificationMode: String
    let warnings: [String]
    let items: [CleanupPlanItemModel]

    enum CodingKeys: String, CodingKey {
        case generatedAt = "generated_at"
        case targetVolumeID = "target_volume_id"
        case keepVolumeID = "keep_volume_id"
        case verificationMode = "verification_mode"
        case warnings
        case items
    }
}

struct CleanupPlanItemModel: Codable, Identifiable {
    let fullHash: String
    let fileSize: UInt64
    let candidateDelete: CopyViewModel
    let status: String
    let verificationMode: String
    let remainingPhysicalDevices: Int
    let blockedReasons: [String]
    let riskNotice: String

    var id: String { "\(fullHash):\(candidateDelete.id)" }

    enum CodingKeys: String, CodingKey {
        case fullHash = "full_hash"
        case fileSize = "file_size"
        case candidateDelete = "candidate_delete"
        case status
        case verificationMode = "verification_mode"
        case remainingPhysicalDevices = "remaining_physical_devices"
        case blockedReasons = "blocked_reasons"
        case riskNotice = "risk_notice"
    }
}

struct JSONLTaskEvent: Codable, Identifiable, Hashable {
    let protocolVersion: Int
    let type: String
    let taskID: String
    let timestamp: String
    let operation: String
    let status: String?
    let currentPath: String?
    let filesSeen: UInt64?
    let filesReused: UInt64?
    let filesSampled: UInt64?
    let filesFullHashed: UInt64?
    let bytesRead: UInt64?
    let error: String?

    var id: String { "\(taskID):\(timestamp):\(type)" }

    enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version"
        case type
        case taskID = "task_id"
        case timestamp
        case operation
        case status
        case currentPath = "current_path"
        case filesSeen = "files_seen"
        case filesReused = "files_reused"
        case filesSampled = "files_sampled"
        case filesFullHashed = "files_full_hashed"
        case bytesRead = "bytes_read"
        case error
    }
}

struct AppLogEntry: Identifiable, Hashable {
    enum Level: String, CaseIterable, Identifiable {
        case info
        case warning
        case error

        var id: String { rawValue }
        var label: String {
            switch self {
            case .info: "信息"
            case .warning: "警告"
            case .error: "错误"
            }
        }
    }

    let id = UUID()
    let timestamp = Date()
    let level: Level
    let operation: String
    let message: String
}
