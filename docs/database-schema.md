# 数据库模式

迁移文件位于 `migrations/`，`schema_migrations` 记录已应用版本。启动时逐个迁移，SQLite 启用 WAL、foreign keys 和 busy timeout。

- `physical_devices`：从 media UUID、设备序列号、分区/卷 UUID 或保守回退标识导出的物理设备分组基础。
- `volumes`：逻辑卷身份、marker、挂载路径、角色、在线状态、`physical_device_id` 和 `identity_state`。`volume_uid` 唯一。
- `volume_identity_conflicts`：相同 marker 但无法安全证明为同一设备的候选注册；原卷离线时仍会保留该记录，绝不自动覆盖。
- `volume_events`：marker 冲突发现、历史卷回填、人工重连和人工解冲突的审计事件。
- `contents`：内容对象；`(full_hash, file_size)` 唯一，空完整哈希允许存在。
- `file_copies`：每个卷上每条相对路径的副本；`(volume_id, relative_path)` 唯一。路径以 BLOB 保存原始字节，并另存展示文本。
- `file_copies.storage_object_key` / `link_group_id`：由物理设备、device ID 与 inode 组成的可识别存储对象键；同一键的多个路径是硬链接，不作为独立备份或完整空间释放量计算。
- `scan_runs`：扫描计数、状态和轻量 checkpoint。
- `file_events`：发现、变化、恢复、缺失等审计事件。

索引覆盖内容大小/完整哈希、卷路径、内容关联、状态和扫描历史。`hash_state` 为 `none`、`sampled`、`full` 或 `failed`；副本状态为 `present`、`missing`、`offline`（展示态）、`deleted`、`unreadable`、`changed` 或 `quarantined`。当前实现不主动写入 `deleted` 或 `quarantined`，为后续安全工作流预留。
