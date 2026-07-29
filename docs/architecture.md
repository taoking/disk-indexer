# 架构

`src/cli.rs` 只处理 clap 参数和输出。macOS SwiftUI App 已消费版本化 JSON/JSONL，不解析终端文本；Rust 库仍保留为 CLI、测试和未来 FFI 的唯一业务核心。

```text
CLI / macOS 原生 App 子进程协议
   │
   ├── volume：卷身份和 marker
   ├── protocol：版本化 JSON / JSONL 任务事件
   ├── scanner：遍历、增量复用、中断状态
   ├── hashing：sample-v1 与完整 BLAKE3
   ├── db：SQLite 迁移与事务
   └── duplicate/report：查询、报告、只读清理计划
```

扫描先收集元数据。元数据没有变化时复用哈希；同大小候选再计算抽样指纹，抽样相同才计算完整哈希。哈希前后都检查关键元数据，变化的文件状态为 `changed`，不会写入可信完整哈希。

原生 App 通过 App Bundle 内的 `disk-indexer` 子进程、显式数据库路径和 JSON/JSONL 协议调用核心能力；不使用 HTTP、浏览器、WebView 或监听端口。`task_runs` 用于恢复展示任务历史，App 不直接读写 SQLite 业务表。

原生 App 的页面、Bundle、取消策略和未来 FFI 边界见 [native-app-architecture.md](native-app-architecture.md)；卷与存储对象的安全语义见 [identity-model.md](identity-model.md)。

状态流转为 `present → missing`（仅成功扫描在线卷后），`missing → present`（再次发现），以及 `present → changed/unreadable`（变化或读取失败）。卷离线由 `volumes.is_online` 表示，不会修改其中 `file_copies` 为 missing。
