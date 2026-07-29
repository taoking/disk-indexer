CREATE TABLE IF NOT EXISTS physical_devices (
    id INTEGER PRIMARY KEY,
    stable_uid TEXT NOT NULL UNIQUE,
    media_uuid TEXT,
    device_serial TEXT,
    model TEXT,
    transport TEXT,
    total_size INTEGER,
    first_seen_at TEXT NOT NULL,
    last_seen_at TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

ALTER TABLE volumes ADD COLUMN physical_device_id INTEGER REFERENCES physical_devices(id);
ALTER TABLE volumes ADD COLUMN identity_state TEXT NOT NULL DEFAULT 'fallback';

CREATE TABLE IF NOT EXISTS volume_identity_conflicts (
    id INTEGER PRIMARY KEY,
    existing_volume_id INTEGER NOT NULL REFERENCES volumes(id),
    candidate_marker_uid TEXT,
    candidate_mount_path BLOB NOT NULL,
    candidate_mount_path_display TEXT NOT NULL,
    candidate_filesystem TEXT,
    candidate_system_volume_uuid TEXT,
    candidate_partition_uuid TEXT,
    candidate_media_uuid TEXT,
    candidate_device_serial TEXT,
    candidate_total_size INTEGER,
    candidate_physical_device_id INTEGER REFERENCES physical_devices(id),
    state TEXT NOT NULL DEFAULT 'open',
    resolution TEXT,
    resolved_volume_id INTEGER REFERENCES volumes(id),
    detected_at TEXT NOT NULL,
    resolved_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS volume_events (
    id INTEGER PRIMARY KEY,
    volume_id INTEGER REFERENCES volumes(id),
    conflict_id INTEGER REFERENCES volume_identity_conflicts(id),
    event_type TEXT NOT NULL,
    old_value_json TEXT,
    new_value_json TEXT,
    occurred_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_volumes_marker_uid ON volumes(marker_uid);
CREATE INDEX IF NOT EXISTS idx_volumes_physical_device ON volumes(physical_device_id);
CREATE INDEX IF NOT EXISTS idx_volume_identity_conflicts_state ON volume_identity_conflicts(state, detected_at);
CREATE INDEX IF NOT EXISTS idx_volume_events_volume_occurred ON volume_events(volume_id, occurred_at);
