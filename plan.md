# Disk Indexer 计划

## 当前阶段

Phase 9.1（回归基线）已完成；下一步是 Phase 9.2（身份安全修复）。本轮目标是先解决 P0 文件身份和缓存安全问题，再以 SwiftUI 原生 App 完全替代浏览器 UI。

## 已完成

- 完成 Phase 1：单 crate、配置、SQLite WAL、版本化迁移和 `init`。
- 完成 Phase 2：卷角色、marker、回退身份、冲突拒绝和卷查询。
- 完成 Phase 3：元数据扫描、忽略规则、状态与离线保护。
- 完成 Phase 4：版本化抽样、流式完整 BLAKE3、增量复用、`hash complete` 和 `lookup`。
- 完成 Phase 5：文本、JSON、CSV 重复报告。
- 完成 Phase 6：只读清理计划及阻止条件。
- 完成核心文档、集成测试和哈希吞吐基准入口。
- 通过最终质量门：`cargo fmt --check`、严格 clippy 和全部测试。
- 完成三卷 CLI 验收：3 副本重复组、精确 lookup、候选/blocked 清理计划、离线保留和增量复用均已验证。
- 完成本机 UI、展示页、UI API 单测和详细使用说明。
- Phase 9.1：已从 `origin/main` 快进核验，阅读现有代码/文档，运行 Rust 基线质量门并确认当前 GitHub Actions 为绿色。

## 待完成

- Phase 9.2：physical devices、marker 克隆冲突、人工 relink/resolve 和审计事件。
- Phase 9.3：stale lookup、状态过滤、硬链接、清理计划验证。
- Phase 9.4：分页、JSONL 任务协议和任务取消。
- Phase 10.1：完全移除浏览器 UI、HTTP 服务和端口监听。
- Phase 10.2：SwiftUI App Shell、内置 Rust CLI、概览/硬盘/设置页。
- Phase 10.3：扫描任务、重复文件、查询、清理计划和日志页。
- Phase 11：原生 App 测试、CI、Release App Bundle 与最终验收。

## 技术决策

- 数据库迁移保存在 `migrations/`，由 `schema_migrations` 逐个记录版本。
- 所有数据库路径字段以 BLOB 保存 Unix 原始字节，并保存仅用于展示的 lossy 文本；非 Unix 平台使用可逆 UTF-8/系统字符串回退。
- 卷优先使用根目录 `.disk-indexer-volume-id` 中的 UUID；无法写入时使用设备号、文件系统和容量的保守回退标识。身份冲突只报告，绝不合并记录。
- 完整 BLAKE3 且文件大小一致才构成重复组；抽样哈希只能缩小候选集合。
- CLI 不含删除或移动文件代码路径；清理功能只输出 JSON 计划。
- UI 用 `axum` 提供静态展示页及操作 API，但严格绑定 `127.0.0.1`；每个请求使用短生命周期 SQLite 连接，避免跨异步任务共享连接。
- 本轮原生 App 通过 App Bundle 内置 `disk-indexer` 子进程和稳定 JSON/JSONL 协议调用 Rust，不引入 Rust/Swift FFI 或网络协议。

## 已知风险

- 在只读卷且系统未提供稳定卷 UUID/序列号时，回退标识仍会纳入路径避免误合并，跨重挂载稳定性有限。
- 扫描 `--resume` 通过已持久化元数据和幂等增量扫描恢复，不保存脆弱的目录遍历栈。
- 本机 UI 没有认证机制，因此绝不可通过端口转发或反向代理暴露到局域网/公网。
- Phase 9.1 基线：`cargo fmt --check`、`cargo clippy --all-targets --all-features -- -D warnings`、`cargo test` 已于 2026-07-29 通过（9 单元测试、3 集成测试）；当前尚无 Swift/Xcode 项目，因此 Swift build/test 门在本阶段不适用。Xcode 26.6 和 Swift 6.3.3 已验证可用。

## 发布记录

- 公开仓库：`https://github.com/taoking/disk-indexer`
- 默认分支：`main`
- 首次提交：`d3e63a4 发布本地磁盘索引工具与 UI`
- 已加入 macOS GitHub Actions 质量门：格式化、Clippy 和测试。

## GUI 后续计划

`disk_indexer` 库公开数据库、扫描和报告服务；未来 SwiftUI 可直接调用 JSON/库层接口，而不复用 CLI 输出解析。
