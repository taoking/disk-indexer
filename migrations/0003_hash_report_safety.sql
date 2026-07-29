ALTER TABLE file_copies ADD COLUMN storage_object_key TEXT;
ALTER TABLE file_copies ADD COLUMN link_group_id TEXT;

CREATE INDEX IF NOT EXISTS idx_file_copies_storage_object ON file_copies(storage_object_key);
CREATE INDEX IF NOT EXISTS idx_file_copies_hash_status ON file_copies(full_hash, file_size, status);
