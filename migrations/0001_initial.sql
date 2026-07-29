CREATE TABLE IF NOT EXISTS volumes (
    id INTEGER PRIMARY KEY,
    volume_uid TEXT NOT NULL UNIQUE,
    marker_uid TEXT,
    volume_name TEXT NOT NULL,
    filesystem TEXT,
    mount_path BLOB NOT NULL,
    mount_path_display TEXT NOT NULL,
    system_volume_uuid TEXT,
    device_serial TEXT,
    partition_uuid TEXT,
    total_size INTEGER,
    role TEXT NOT NULL DEFAULT 'unknown',
    is_online INTEGER NOT NULL DEFAULT 1,
    first_seen_at TEXT NOT NULL,
    last_seen_at TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS contents (
    id INTEGER PRIMARY KEY,
    file_size INTEGER NOT NULL,
    sample_hash TEXT,
    sample_algorithm TEXT,
    full_hash TEXT,
    hash_algorithm TEXT NOT NULL DEFAULT 'blake3',
    hash_state TEXT NOT NULL DEFAULT 'none',
    first_seen_at TEXT NOT NULL,
    last_seen_at TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(full_hash, file_size)
);

CREATE TABLE IF NOT EXISTS scan_runs (
    id INTEGER PRIMARY KEY,
    volume_id INTEGER NOT NULL REFERENCES volumes(id),
    root_relative_path BLOB NOT NULL,
    mode TEXT NOT NULL,
    status TEXT NOT NULL,
    started_at TEXT NOT NULL,
    finished_at TEXT,
    discovered_count INTEGER NOT NULL DEFAULT 0,
    metadata_reused_count INTEGER NOT NULL DEFAULT 0,
    sampled_count INTEGER NOT NULL DEFAULT 0,
    full_hashed_count INTEGER NOT NULL DEFAULT 0,
    skipped_count INTEGER NOT NULL DEFAULT 0,
    missing_count INTEGER NOT NULL DEFAULT 0,
    error_count INTEGER NOT NULL DEFAULT 0,
    bytes_read INTEGER NOT NULL DEFAULT 0,
    checkpoint TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS file_copies (
    id INTEGER PRIMARY KEY,
    volume_id INTEGER NOT NULL REFERENCES volumes(id),
    relative_path BLOB NOT NULL,
    relative_path_display TEXT NOT NULL,
    filename BLOB NOT NULL,
    filename_display TEXT NOT NULL,
    file_size INTEGER NOT NULL,
    modified_at_ns INTEGER,
    created_at_ns INTEGER,
    inode INTEGER,
    device_id INTEGER,
    content_id INTEGER REFERENCES contents(id),
    sample_hash TEXT,
    sample_algorithm TEXT,
    full_hash TEXT,
    hash_algorithm TEXT NOT NULL DEFAULT 'blake3',
    hash_state TEXT NOT NULL DEFAULT 'none',
    status TEXT NOT NULL DEFAULT 'present',
    last_error TEXT,
    first_seen_at TEXT NOT NULL,
    last_seen_at TEXT NOT NULL,
    last_verified_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(volume_id, relative_path)
);

CREATE TABLE IF NOT EXISTS file_events (
    id INTEGER PRIMARY KEY,
    file_copy_id INTEGER NOT NULL REFERENCES file_copies(id),
    scan_run_id INTEGER REFERENCES scan_runs(id),
    event_type TEXT NOT NULL,
    old_value_json TEXT,
    new_value_json TEXT,
    occurred_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_contents_file_size ON contents(file_size);
CREATE INDEX IF NOT EXISTS idx_contents_full_hash_size ON contents(full_hash, file_size);
CREATE INDEX IF NOT EXISTS idx_file_copies_volume_path ON file_copies(volume_id, relative_path);
CREATE INDEX IF NOT EXISTS idx_file_copies_content ON file_copies(content_id);
CREATE INDEX IF NOT EXISTS idx_file_copies_status ON file_copies(status);
CREATE INDEX IF NOT EXISTS idx_scan_runs_volume_started ON scan_runs(volume_id, started_at);
CREATE INDEX IF NOT EXISTS idx_file_copies_size_status ON file_copies(file_size, status);
