# 数据库迁移与升级

数据库使用 SQLite WAL，并在 `schema_migrations` 中逐个记录已应用迁移。升级会在打开数据库时顺序执行，已应用版本不会重复执行。升级前建议停止 App 与 CLI 任务，并备份 `index.db`、`index.db-wal` 与 `index.db-shm`（如存在）。

| 版本 | 迁移 | 目的 |
| --- | --- | --- |
| 0001 | `initial` | 初始卷、扫描、文件副本、内容和清理计划数据模型。 |
| 0002 | `volume_identity_safety` | `physical_devices`、卷身份状态、冲突与审计；回填旧卷的设备分组。 |
| 0003 | `hash_report_safety` | `storage_object_key`、硬链接分组和更严格的哈希/清理报告数据。 |
| 0004 | `task_protocol_and_paging` | `task_runs` 与大查询 keyset 分页支持。 |

升级步骤：

```bash
# 先备份数据库文件，再由新版本打开并自动升级
disk-indexer --db /安全位置/index.db init
disk-indexer --db /安全位置/index.db stats --json
```

然后检查 schema 版本、卷数与文件数，并随机执行 `lookup --full-hash`、`duplicates --json --page` 和只读 `cleanup plan`。迁移不修改已扫描的原始文件，也不会自动解决 marker 冲突；存在冲突时应使用 `volume conflicts` 审核。

降级不受支持：旧二进制不保证理解新表和列。若需要回退，应关闭所有任务后恢复升级前完整的 SQLite 备份，而不是删除迁移记录。
