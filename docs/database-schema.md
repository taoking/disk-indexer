# 数据库模式

迁移文件位于 `migrations/`，`schema_migrations` 记录已应用版本。启动时逐个迁移，SQLite 启用 WAL、foreign keys 和 busy timeout。

- `volumes`：逻辑卷身份、marker、挂载路径、角色与在线状态。`volume_uid` 唯一。
- `contents`：内容对象；`(full_hash, file_size)` 唯一，空完整哈希允许存在。
- `file_copies`：每个卷上每条相对路径的副本；`(volume_id, relative_path)` 唯一。路径以 BLOB 保存原始字节，并另存展示文本。
- `scan_runs`：扫描计数、状态和轻量 checkpoint。
- `file_events`：发现、变化、恢复、缺失等审计事件。

索引覆盖内容大小/完整哈希、卷路径、内容关联、状态和扫描历史。`hash_state` 为 `none`、`sampled`、`full` 或 `failed`；副本状态为 `present`、`missing`、`offline`（展示态）、`deleted`、`unreadable`、`changed` 或 `quarantined`。当前实现不主动写入 `deleted` 或 `quarantined`，为后续安全工作流预留。
