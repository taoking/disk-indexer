# macOS 原生应用架构

`DiskIndexer.app` 是 macOS 14+ 的 SwiftUI 应用，当前优先构建 `arm64`。它没有浏览器页面、WebView、HTTP 客户端、本地服务器或监听端口。

```text
SwiftUI 页面 / AppState
        │  Process.arguments（不经 shell）
        ▼
DiskIndexer.app/Contents/Resources/disk-indexer
        │  JSON 或 JSONL stdout；stderr 诊断
        ▼
Rust 核心 ── SQLite / BLAKE3 / 文件系统
```

## Bundle 与构建

`scripts/build-macos-app.sh` 先构建 `target/release/disk-indexer`，再构建 Xcode 的 Debug App，并复制到：

```text
DiskIndexer.app/Contents/Resources/disk-indexer
```

Swift 的 `RustCommandRunner` 只以 `Bundle.main.url(forResource:)` 定位该可执行文件。它始终把 `--db <path>` 放进独立的 `Process.arguments` 数组，绝不调用 shell、拼接命令字符串或依赖 `PATH`。当前脚本明确指定 `arm64`；未来 Universal 2 需要分别构建 `arm64` 和 `x86_64` Rust 二进制并在打包阶段用 `lipo` 合并，不能让运行时回退到用户 PATH。

## 数据与任务协议

- 短命令（卷、统计、分页重复组、查询）只从 stdout 读取一个 JSON 文档。
- 长命令（扫描、补完整哈希、验证、清理计划）使用 JSONL。`TaskProcessController` 按字节流分段复原行，并把结构化事件回送 MainActor。
- stderr 只作为诊断日志，不能污染 JSON/JSONL stdout。
- `task_runs` 持久化任务历史，因此 App 重开后仍可显示已完成、失败或中断任务。
- 重复组命令使用内容 ID 游标，每页 50 组；Swift 不自动读取完整报告。

取消长任务时先发送 `Process.interrupt()`（SIGINT）。五秒仍未退出才调用 `terminate()`；不会使用 `kill -9` 作为正常流程。窗口关闭检测到运行任务时会要求用户选择等待或安全取消，等待退出后才允许终止应用。

## 页面与安全边界

| 页面 | Rust 协议 | 关键限制 |
| --- | --- | --- |
| 硬盘 | `volume add/list/conflicts/resolve` | `possible_clone` 不会覆盖历史卷，只能明确保留为新卷。 |
| 扫描任务 | `scan`、`hash complete` JSONL | UI 不阻塞；支持进度、取消与任务历史。 |
| 重复文件 | `duplicates --json --page` | 明确显示路径、存储对象、卷、物理设备及离线状态。 |
| 文件查询 | `lookup --json [--full-hash]` | `cache_stale` 绝不显示精确命中。 |
| 清理计划 | `cleanup plan --jsonl-progress --output` | 只能生成/导出计划；没有删除、废纸篓、隔离或一键清理按钮。 |
| 设置与日志 | `stats --json` 与内存诊断 | 切换数据库前要求无运行任务且目标文件已存在、可打开。 |

App 不直接写 SQLite 业务表，避免 Swift 与 Rust 产生两套安全规则。未来若迁移到 UniFFI 或 C ABI，应保留相同的 Swift 页面模型、JSONL 任务语义、取消状态和安全限制；本轮不实现 FFI。
