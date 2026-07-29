# 架构

`src/cli.rs` 只处理 clap 参数和输出。未来 SwiftUI 可调用 `disk_indexer` 库中的 `db`、`scanner`、`duplicate` 和 `report` 模块，或消费版本化 JSON，不必解析终端文本。

```text
CLI / 本机浏览器 UI
   │
   ├── volume：卷身份和 marker
   ├── ui：127.0.0.1 路由、展示页与操作 API
   ├── scanner：遍历、增量复用、中断状态
   ├── hashing：sample-v1 与完整 BLAKE3
   ├── db：SQLite 迁移与事务
   └── duplicate/report：查询、报告、只读清理计划
```

扫描先收集元数据。元数据没有变化时复用哈希；同大小候选再计算抽样指纹，抽样相同才计算完整哈希。哈希前后都检查关键元数据，变化的文件状态为 `changed`，不会写入可信完整哈希。

`disk-indexer ui` 通过 `axum` 提供静态展示页以及卷注册、扫描、补哈希和清理计划 API。服务固定绑定 `127.0.0.1`；每个请求打开短生命周期数据库连接，避免将 SQLite 连接跨异步任务共享。UI 复用扫描、报告和清理计划服务，不包含文件删除 API。

状态流转为 `present → missing`（仅成功扫描在线卷后），`missing → present`（再次发现），以及 `present → changed/unreadable`（变化或读取失败）。卷离线由 `volumes.is_online` 表示，不会修改其中 `file_copies` 为 missing。
