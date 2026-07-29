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

struct VolumeConflict: Codable {
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
