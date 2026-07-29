-- 卷 UUID、分区 UUID 与挂载路径只能识别逻辑卷，不能证明物理介质独立。
-- 既有 physical_devices 的旧字段可能来自这些不可信来源，因此一律从 unknown 开始，
-- 等下一次实际注册时由整盘级 diskutil 信息重新确认。
ALTER TABLE physical_devices ADD COLUMN whole_disk_identifier TEXT;
ALTER TABLE physical_devices ADD COLUMN whole_disk_media_uuid TEXT;
ALTER TABLE physical_devices ADD COLUMN hardware_serial TEXT;
ALTER TABLE physical_devices ADD COLUMN identity_state TEXT NOT NULL DEFAULT 'unknown';
ALTER TABLE physical_devices ADD COLUMN identity_source TEXT NOT NULL DEFAULT 'legacy_volume_identity';
ALTER TABLE physical_devices ADD COLUMN last_verified_at TEXT;

CREATE INDEX IF NOT EXISTS idx_physical_devices_whole_disk_media_uuid
    ON physical_devices(whole_disk_media_uuid);
CREATE INDEX IF NOT EXISTS idx_physical_devices_hardware_serial
    ON physical_devices(hardware_serial);
CREATE INDEX IF NOT EXISTS idx_physical_devices_identity_state
    ON physical_devices(identity_state);
