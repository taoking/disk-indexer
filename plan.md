# Disk Indexer 计划

## 当前阶段

Phase 12.10（CI、App 构建运行与 GitHub Actions 验收）已完成；最新代码提交 `f3c2f23` 的 GitHub Actions 两个 job 均已实际成功。本轮未扩展删除、隔离、相似媒体或网络功能；原生 App 图标已完成构建和 CI 验收。

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
- Phase 9.2：新增 `0002_volume_identity_safety`；引入 `physical_devices`、`volumes.physical_device_id`、`identity_state`、冲突/卷审计表和旧历史回填。相同 marker 在稳定身份冲突或无法证明一致时返回 `possible_clone`，即使原卷离线也绝不覆盖；新增 `volume conflicts`、`volume resolve --as-new-volume` 和受稳定身份约束的 `volume relink`。自动化覆盖离线克隆、稳定重挂载和旧数据库迁移。
- Phase 9.3：新增 `0003_hash_report_safety`。`lookup` 先比较大小、mtime 纳秒、inode、device ID；缓存过期时返回 `cache_stale` 而不是精确命中，`--full-hash` 才重新计算。扫描写入 storage object / hard-link 分组键；重复报告分开统计路径、独立存储对象、逻辑卷和物理设备。清理计划默认 `candidate_unverified`，可用元数据/完整哈希严格验证得到 `verified_candidate`，硬链接、状态异常、存储对象或物理设备数量不足一律 `blocked`。自动化覆盖 stale、硬链接、状态过滤和清理验证。
- Phase 9.4：新增 `0004_task_protocol_and_paging` 和 `task_runs`。哈希补齐、卷验证、候选哈希和重复内容查询改用 `id > after_id` keyset 分页，默认查询页 1000、写入批 500；新增 `--jsonl-progress`、任务历史和 SIGINT 安全停止。JSONL stdout 只写机器事件，stderr 保留诊断。自动化覆盖 10 万条模拟文件的分页无遗漏和 JSONL 纯事件输出。
- Phase 10.1：完全删除 `disk-indexer ui`、`src/ui.rs`、静态页面、Axum/Tokio/WebBrowser/Tower 依赖与所有端口文档；CLI 集成测试断言 `ui` 子命令不可用，依赖树不再包含这些组件。
- Phase 10.2：新增 Apple Silicon 优先、macOS 14+ 的 SwiftUI Xcode 项目和独立单元测试；实现概览、硬盘注册、设置页，以及通过 `Bundle.main` 定位内置 Rust CLI 的 `Process.arguments` 调用边界。构建脚本会将 release `disk-indexer` 复制到 `DiskIndexer.app/Contents/Resources/`；同步读取 stdout/stderr 防止大输出管道阻塞。未引入 PATH、shell、HTTP、浏览器或端口。
- Phase 10.3：新增扫描任务、重复文件、文件查询、清理计划和日志原生页；长任务由 `TaskProcessController` 使用 JSONL 实时展示，stdout/stderr 分离，取消先 SIGINT、超时才 terminate，关闭窗口时要求选择安全取消或保留窗口。重复组新增 Rust 内容 ID 游标 API（每页 50）而非一次传给 Swift；概览新增只读统计接口。文件查询明确标示 `cache_stale`，清理页只能生成/导出 JSON 计划，绝无删除操作。Swift 单测覆盖 JSONL 分段解码和取消状态。
- Phase 11：新增 macOS App GitHub Actions job（Swift 测试、Bundle 构建与内置 Rust binary 检查）、完整原生 App/迁移/身份安全文档和验收步骤；`scripts/build-macos-app.sh` 已实际生成 `DiskIndexer.app`，主程序与内置 CLI 均为 arm64 Mach-O，后者可执行且版本为 0.1.0。已用 `open` 启动 App 并确认进程仍运行；该进程无 TCP 监听，CLI/依赖树也不含旧 Web UI 组件。为兼容 GitHub `macos-14` 的 Xcode 15.4，项目使用 Swift 5 语言模式（仍使用 Swift Concurrency API），而不是要求仅本机已有的 Swift 6。
- Phase 12.1：已核验 `main` 与 `origin/main` 同步；`cargo fmt --check`、严格 Clippy、全部 Rust 测试（7 单元、12 集成）、Swift 测试（3）和 `scripts/build-macos-app.sh` 均通过。隔离数据库 schema 为 v4；最新 GitHub Actions `30438122820`（`c1e9bd4`）的 `quality` 与 `macos-app` job 均成功。构建仅有 AppIntents 未使用的 Xcode 警告，不影响产物。
- Phase 12.2：新增 `0005_physical_device_identity_v2`。物理设备现保存整盘标识、整盘 Media UUID、硬件序列号、身份状态/来源和最近验证时间；macOS 注册会从卷的 `ParentWholeDisk` 再查询整盘 `diskutil -plist`，不再将 `disk4s2` 一类 `DeviceIdentifier` 当作序列号。只有硬件序列号或整盘 Media UUID 的 `verified` 身份可进入物理设备安全计数；整盘标识仅为 `inferred`，旧数据和无法确认的设备为 `unknown`，矛盾证据为 `conflict`。重复报告/CSV 显式输出 verified 与 unknown/unverified 数量，清理阈值只计 verified。旧 schema 的卷/分区 UUID 回填不再自动升级为 verified。Rust 质量门（7 单元、14 集成）与 Swift 3 项测试均通过。
- Phase 12.3：严格清理计划会先刷新卷在线状态，并对候选删除副本和每个剩余独立 storage object 的代表路径执行元数据/可选完整 BLAKE3 验证；本任务缓存同一 `file_copy_id`，不会重复完整哈希。所有阈值只基于验证成功的代表副本重新计算，且物理设备仍只计 `verified` 整盘身份。计划 JSON 新增验证协议版本、完成/阻止/验证计数、取消预留字段、剩余候选/成功/失败副本、验证后各类计数、失败原因和物理身份警告。必要副本的失败会阻止计划；候选自身的 stale 则标明 `stale`。新增破坏未重扫的第三副本回归覆盖；Rust 质量门（7 单元、15 集成）与 Swift 3 项测试均通过。
- Phase 12.4：新增 Rust `TaskProgress`，扫描、完整哈希、验证和清理计划的每条 JSONL `progress` 事件统一包含 files/groups/bytes/current path 字段；`JsonlTask` 以“100 个文件、300ms 或组阶段”节流，扫描在 JSONL 模式以最多 100 个已提交文件批次刷新。完整哈希/验证/清理计划接入实时回调，stdout 保持纯 JSONL。Swift 模型同步字段，明确拒绝非 v1 协议，任务事件最多保留 500 条；任务页按操作展示相应字段且缺失值显示“—”。新增 205 文件完整哈希实时事件测试与 Swift 协议/容量测试。Rust 质量门（7 单元、16 集成）与 Swift 5 项测试均通过。
- Phase 12.5：新增 `TaskRunGuard`，所有 JSONL 长任务由 guard 创建并统一结束；异常提前返回时 Drop 会把记录保守标为 `abandoned`。数据库每次打开都会把遗留 `running` 任务改为 `abandoned` 并写明恢复原因，已结束任务不受影响。扫描、完整哈希、验证和清理计划都使用同一个 SIGINT 标志；验证中断以 `interrupted` 正常结束。严格清理计划支持取消、标记未完成项、且 CLI 取消时绝不写最终输出。计划 JSON 先写同目录临时文件、`sync_all` 后原子 rename；失败会删除临时文件。Swift 任务控制器能识别 Rust `interrupted` 终态。新增旧运行任务恢复和 guard Drop 回归测试；Rust 质量门（7 单元、18 集成）与 Swift 5 项测试均通过。
- Phase 12.6：Swift 增加 `TaskOperation` 与 `PendingTaskContext(localID, operation, outputURL, remoteTaskID)`；`TaskProcessController.start()` 返回本地 UUID，收到同一任务的 `task_started` 后才绑定 Rust task ID。仅当上下文为 cleanup、远端 ID 与当前终态事件匹配且 Rust 成功完成时读取导出的 JSON；完成、失败、取消、中断和启动失败都会清理上下文，普通任务不会触发 cleanup 文件读取。新增上下文绑定单测；Rust 质量门（7 单元、18 集成）与 Swift 6 项测试均通过。
- Phase 12.7：已有数据库存在未应用迁移时，升级前使用 SQLite `VACUUM INTO` 在原数据库同目录创建带 UTC 时间戳的 `.before-migration-*.sqlite` 一致性备份；迁移前后执行 `PRAGMA quick_check` 和 `foreign_key_check`。迁移事务失败的信息会给出备份恢复路径。新增旧 schema 升级备份回归测试；Rust 质量门（7 单元、18 集成）与 Swift 6 项测试均通过。
- Phase 12.8：`duplicates --csv` 改用内容 ID keyset 分页直接写入 CSV，新增 `duplicates --jsonl` 逐组输出 JSON Lines；两者不再先汇总完整重复报告。回归测试验证流式 CSV 与 JSONL 输出；Rust 质量门（7 单元、18 集成）与 Swift 6 项测试均通过。
- Phase 12.9：数据库路径仅会在设置页验证成功后写入 `UserDefaults`；启动时读取非空持久值，否则使用默认 Application Support 路径。新增隔离 UserDefaults suite 的持久化测试；Swift 测试增至 7 项。
- Phase 12.10：CI `quality` job 增加原生边界守卫，检查 Rust 核心与 Swift App 代码不得重新引入 Web/HTTP/端口监听依赖；文档补充 verified 整盘身份、流式 JSONL/CSV 和迁移备份。已实际运行边界检查、构建 `DiskIndexer.app` 并用 `open` 启动。推送后必须以最新提交的 GitHub Actions 两个 job 成功作为最终验收。
- 原生 App 图标：新增深蓝—青绿的硬盘索引标志，使用完整 macOS `AppIcon.appiconset`（16px 至 1024px）并接入 Xcode Asset Catalog；不改变任何扫描、清理或网络边界。提交 `f3c2f23` 的 GitHub Actions 运行 `30449757276` 中 `quality` 与 `macos-app` 均已实际成功。

## 待完成

- 当前没有未完成的 Phase 12 项目或原生 App 图标验收项。


## 技术决策

- 数据库迁移保存在 `migrations/`，由 `schema_migrations` 逐个记录版本。
- 所有数据库路径字段以 BLOB 保存 Unix 原始字节，并保存仅用于展示的 lossy 文本；非 Unix 平台使用可逆 UTF-8/系统字符串回退。
- marker 只能证明“可能是同一逻辑卷”。注册时会比较卷/分区/media UUID、设备身份、容量、文件系统和历史物理设备；稳定身份不一致或缺失且路径不同均创建可审计的 `possible_clone`，绝不合并记录。既有卷在升级时会回填物理设备分组。
- 路径不是独立存储的证明。`storage_object_key` 由物理设备、device ID 与 inode 组成；报告理论释放空间按在线独立存储对象计算。索引哈希复用前必须验证元数据，清理计划默认不代表已重新核验文件。
- 长任务使用版本化 JSONL，并把最终状态写入 `task_runs`；默认分页不使用 OFFSET。当前重复报告仍为兼容 CLI 汇总为内存中的 JSON/文本结果，原生 App 将在 Phase 10 使用分页 API 分段加载。
- 完整 BLAKE3 且文件大小一致才构成重复组；抽样哈希只能缩小候选集合。
- CLI 不含删除或移动文件代码路径；清理功能只输出 JSON 计划。
- 本轮原生 App 通过 App Bundle 内置 `disk-indexer` 子进程和稳定 JSON/JSONL 协议调用 Rust，不引入 Rust/Swift FFI 或网络协议。

## 已知风险

- 在只读卷且系统未提供稳定卷 UUID/序列号时，回退标识仍会纳入路径避免误合并，跨重挂载稳定性有限。
- 扫描 `--resume` 通过已持久化元数据和幂等增量扫描恢复，不保存脆弱的目录遍历栈。
- Phase 9.1 基线：`cargo fmt --check`、`cargo clippy --all-targets --all-features -- -D warnings`、`cargo test` 已于 2026-07-29 通过（9 单元测试、3 集成测试）；当前尚无 Swift/Xcode 项目，因此 Swift build/test 门在本阶段不适用。Xcode 26.6 和 Swift 6.3.3 已验证可用。
- Phase 9.2 的 CLI 冲突返回是安全状态而不是异常；调用方必须检查 `volume` 是否为 null/缺失并引导用户先审核冲突。
- Phase 10.2：Swift 子进程读取与基础参数构造已有单测；尚未接入 JSONL 实时事件、取消控制和全部业务页面，这些在 Phase 10.3 完成。
- Phase 10.3：重复组游标以内容 ID 的稳定顺序读取；App 可对已加载页按空间、大小或副本数排序，但不会为了全局排序把全部报告读入内存。
- Phase 11：当前 Bundle 为未签名 Debug 构建，尚未做 Developer ID 签名、公证或 dmg/pkg 发布；Universal 2 只保留了构建扩展说明，尚未实际合并 x86_64 二进制。GitHub Actions 会在推送本提交后验证 macOS 环境。
- Phase 12.10：本地完整质量门、原生 App 构建/启动与最新 GitHub Actions 实际成功均已完成。新的图标资源尚未做 Developer ID 签名、公证或独立发布验证。

## 发布记录

- 公开仓库：`https://github.com/taoking/disk-indexer`
- 默认分支：`main`
- 首次提交：`d3e63a4 发布本地磁盘索引工具与 UI`
- 已加入 macOS GitHub Actions 质量门：格式化、Clippy 和测试。

## 原生 App 后续计划

SwiftUI App 将通过 App Bundle 内置 CLI 和 JSON/JSONL 协议调用 Rust 核心，不复用终端文本、不启动 HTTP 服务，也不直接读写 SQLite 业务表。
