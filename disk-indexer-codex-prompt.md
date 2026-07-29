# 多硬盘文件索引与重复备份清理工具：Codex 开发 Prompt

## 角色

你是一名资深 Rust、SQLite 和 macOS 文件系统工程师。请在当前仓库中设计并实现一个本地运行的多硬盘文件索引与重复备份识别工具。

项目暂定名：`disk-indexer`

本项目用于个人数据整理。用户拥有多个移动硬盘、机械硬盘和 SSD，长期进行了大量重复备份。工具需要建立一个长期可复用的本地数据库，对不同硬盘中的文件进行扫描、索引和内容指纹计算，并在后续扫描时判断文件是否曾经出现过、当前存在多少份副本、分别位于哪些硬盘和路径。

第一阶段只识别“文件字节内容完全相同”的重复文件，不处理相似照片、连拍照片、RAW 与 JPEG 关联、视频相似度或压缩包内部去重。

---

# 一、项目目标

实现一个可靠的本地 CLI 工具，具备以下核心能力：

1. 注册和识别多个硬盘或文件卷。
2. 扫描硬盘中的文件和目录。
3. 使用 SQLite 保存文件元数据、内容哈希、硬盘信息和扫描历史。
4. 支持断点续扫和增量扫描。
5. 判断一个文件是否曾经被扫描和收录。
6. 查找完全相同内容的所有文件副本。
7. 区分在线、离线、缺失、已删除和无法读取的副本。
8. 生成重复文件报告和建议清理清单。
9. 第一阶段只生成报告，不执行自动永久删除。
10. 为未来 SwiftUI/macOS GUI、隔离区删除和相似照片识别预留清晰接口。

核心概念不是“重复文件立即删除”，而是：

> 建立跨硬盘、可离线查询、基于内容哈希的文件资产索引，并识别超过备份策略所需数量的冗余副本。

---

# 二、默认技术方案

除非当前仓库已经存在明确技术栈，否则采用：

- 语言：Rust stable
- CLI：`clap`
- SQLite：`rusqlite`
- 哈希：`blake3`
- 遍历目录：优先标准库；如确有必要可使用 `walkdir`
- 时间处理：`chrono` 或 `time`
- 序列化：`serde`、`serde_json`
- 错误处理：
  - 应用层：`anyhow`
  - 核心库：可使用 `thiserror`
- 日志：`tracing`、`tracing-subscriber`
- 并发：
  - 第一版保持保守
  - 同一块机械硬盘默认低并发或单文件顺序读取
  - 不得为了追求 CPU 利用率造成大量随机读取
- 数据库默认路径：
  - macOS：`~/Library/Application Support/DiskIndexer/index.db`
  - 同时允许通过 CLI 参数或环境变量覆盖
- 数据库启用：
  - WAL
  - foreign_keys
  - busy_timeout
  - 批量事务写入

不要引入不必要的 Web 服务、云端组件、Electron、Docker、MySQL 或 PostgreSQL。

---

# 三、工程结构

将扫描核心与 CLI 解耦，建议结构如下：

```text
disk-indexer/
├── Cargo.toml
├── README.md
├── plan.md
├── docs/
│   ├── architecture.md
│   ├── database-schema.md
│   ├── scan-behavior.md
│   ├── safety-model.md
│   └── acceptance.md
├── migrations/
│   └── ...
├── src/
│   ├── main.rs
│   ├── cli/
│   ├── config/
│   ├── db/
│   ├── volume/
│   ├── scanner/
│   ├── hashing/
│   ├── duplicate/
│   ├── report/
│   └── model/
└── tests/
```

要求：

1. 扫描、哈希、数据库、报告生成分别封装。
2. CLI 层不得包含核心业务逻辑。
3. 核心逻辑未来可以被 SwiftUI GUI、其他 CLI 或测试直接调用。
4. 数据库迁移必须版本化，不能仅在启动时硬编码建表。
5. 所有文件路径在数据库中保存原始字节可恢复的表示；不得因为非 UTF-8 路径导致崩溃。
6. macOS 为第一目标平台，但核心模型不要无意义地锁死 macOS。

---

# 四、核心数据模型

至少实现以下实体。

## 1. volumes

记录逻辑硬盘或文件卷。

建议字段：

```text
id
volume_uid
marker_uid
volume_name
filesystem
mount_path
system_volume_uuid
device_serial
partition_uuid
total_size
role
is_online
first_seen_at
last_seen_at
created_at
updated_at
```

说明：

- `volume_uid` 是系统内部稳定主标识。
- 如果卷根目录可写，支持创建隐藏标记文件：
  - `.disk-indexer-volume-id`
- 标记文件内容为随机 UUID。
- 如果卷只读，则回退到系统卷 UUID、设备序列号、分区 UUID、容量和文件系统的组合识别。
- 不允许只依赖 `/Volumes/卷名`。
- 发现身份冲突时不得自动合并，必须报告冲突。

`role` 第一版至少支持：

```text
primary
local_backup
offsite_backup
temporary
legacy_backup
unknown
```

第一版可以只保存角色，暂不实现复杂自动策略。

## 2. contents

记录文件内容对象。

建议字段：

```text
id
file_size
sample_hash
full_hash
hash_algorithm
hash_state
first_seen_at
last_seen_at
created_at
updated_at
```

约束：

- `full_hash + file_size` 建立唯一约束。
- `full_hash` 未计算时允许为空。
- `hash_state` 至少包括：
  - none
  - sampled
  - full
  - failed

## 3. file_copies

记录某个内容在某块硬盘上的具体副本。

建议字段：

```text
id
volume_id
relative_path
filename
file_size
modified_at_ns
created_at_ns
inode
device_id
content_id
sample_hash
full_hash
status
last_error
first_seen_at
last_seen_at
last_verified_at
created_at
updated_at
```

`status` 至少包括：

```text
present
missing
offline
deleted
unreadable
changed
quarantined
```

注意：

- 硬盘离线时，不得将该硬盘中的所有文件直接标记为 missing。
- 只有对应硬盘在线并完成一次成功扫描后，未出现的旧记录才能进入 missing。
- 原路径出现但大小、时间或 inode 已变化时，应视为文件发生变化，重新计算内容身份。
- 不要覆盖历史内容关系而丢失审计信息。

## 4. scan_runs

记录每次扫描。

建议字段：

```text
id
volume_id
root_relative_path
mode
status
started_at
finished_at
discovered_count
metadata_reused_count
sampled_count
full_hashed_count
skipped_count
missing_count
error_count
bytes_read
checkpoint
created_at
updated_at
```

`status` 至少包括：

```text
pending
running
paused
completed
completed_with_errors
cancelled
failed
interrupted
```

## 5. file_events

保存关键历史事件。

建议字段：

```text
id
file_copy_id
scan_run_id
event_type
old_value_json
new_value_json
occurred_at
```

事件示例：

```text
discovered
metadata_changed
content_changed
marked_missing
restored
marked_deleted
marked_quarantined
verify_failed
```

---

# 五、扫描与哈希策略

实现分层扫描，避免第一次就无差别读取所有硬盘全部内容。

## 阶段 A：元数据扫描

遍历文件并记录：

- 相对路径
- 文件名
- 文件大小
- 修改时间，尽量保留纳秒
- 创建时间，系统支持时记录
- inode 或平台等效 File ID
- device ID
- 所属 volume

默认跳过：

- `.Spotlight-V100`
- `.Trashes`
- `.fseventsd`
- 系统临时目录
- 工具自己的隔离区
- 工具自己的标记和数据库文件
- 用户通过配置指定的忽略规则

符号链接默认不跟随，防止循环；需要在文档和 CLI 输出中明确。

## 阶段 B：增量复用

同一卷再次扫描时，如果以下条件一致：

- 相对路径
- 文件大小
- 修改时间纳秒值
- inode 或 File ID

则复用已有哈希，不重新读取文件。

如果文件系统不可靠地支持 inode 或纳秒时间，允许采用更保守策略，但必须在代码和文档中说明。

## 阶段 C：快速指纹

对可能重复的候选文件计算抽样哈希。

推荐抽样：

- 文件头部固定区间
- 文件中部固定区间
- 文件尾部固定区间
- 小文件直接完整读取
- 抽样算法也使用 BLAKE3

抽样长度和阈值写入配置，并记录算法版本，避免以后修改算法后误复用旧指纹。

快速指纹只用于筛选候选，不能作为最终删除或完全重复结论。

## 阶段 D：完整哈希

以下情况计算完整 BLAKE3：

1. 同文件大小存在多个候选。
2. 抽样指纹相同。
3. 用户执行全量完整索引命令。
4. 删除计划生成前缺少可信完整哈希。
5. 用户主动查询并要求确认。

完全重复判断必须至少满足：

```text
file_size 相同
且
完整 BLAKE3 相同
```

未来执行删除前还要再次验证，但本阶段不实现永久删除。

---

# 六、增量扫描与中断恢复

必须支持：

1. 扫描中按 Ctrl+C 优雅中断。
2. 已完成的批次数据保留。
3. `scan_runs` 正确标记为 interrupted 或 cancelled。
4. 后续允许继续同一扫描任务。
5. 不因异常退出损坏数据库。
6. 每批文件使用事务提交。
7. checkpoint 中不要保存不可维护的大型状态；优先依赖数据库中的已扫描记录。
8. 扫描日志能够显示：
   - 当前路径
   - 已发现文件数
   - 已读取字节
   - 当前速率
   - 错误数
   - 元数据复用数量
   - 完整哈希数量

不要给出不可靠的精确剩余时间；如能合理估算，可标记为估算值。

---

# 七、CLI 设计

实现清晰、可脚本化的命令。

## 初始化

```bash
disk-indexer init
```

行为：

- 创建应用数据目录。
- 创建或升级 SQLite 数据库。
- 输出数据库路径和 schema 版本。

## 注册硬盘

```bash
disk-indexer volume add /Volumes/Photos
```

可选参数：

```bash
--role primary
--write-marker
--no-write-marker
```

输出：

- 卷名称
- 挂载路径
- 系统卷 UUID
- 设备标识
- marker UID
- 最终内部 volume UID
- 是否可写
- 是否发现身份冲突

## 查看硬盘

```bash
disk-indexer volume list
disk-indexer volume show <volume-id>
```

## 扫描

```bash
disk-indexer scan /Volumes/Photos
```

可选参数：

```text
--full-hash
--metadata-only
--resume
--exclude <pattern>
--max-readers <n>
--json
```

默认：

- 自动识别卷。
- 先做元数据扫描。
- 对重复候选进行抽样和必要的完整哈希。
- 不对所有唯一大小文件强制完整哈希。

## 补齐全量哈希

```bash
disk-indexer hash complete --volume <volume-id>
disk-indexer hash complete --all
```

此命令用于逐步把数据库升级为“任意新文件都能快速判断是否曾经存在”的完整索引。

## 查询文件

```bash
disk-indexer lookup "/Volumes/NewDisk/path/file.ext"
```

输出至少包括：

```text
文件大小
抽样指纹状态
完整哈希状态
数据库中是否曾出现
当前已知副本数量
在线副本数量
离线副本数量
所有已知硬盘和相对路径
首次发现时间
最后发现时间
```

支持：

```bash
--json
--full-hash
```

## 查看重复组

```bash
disk-indexer duplicates
```

过滤参数：

```text
--volume <volume-id>
--min-size <bytes>
--min-copies <n>
--online-only
--include-missing
--json
--csv <path>
```

重复组按可释放空间、文件大小或副本数量排序。

“可释放空间”第一版可以按“每组保留 1 份”进行理论计算，但输出必须注明这只是理论值，不等同于安全删除建议。

## 查看扫描历史

```bash
disk-indexer scan list
disk-indexer scan show <scan-id>
```

## 验证数据库记录

```bash
disk-indexer verify --volume <volume-id>
disk-indexer verify --file-copy <id>
```

验证行为：

- 检查文件是否存在。
- 检查大小和元数据。
- 可选重新计算完整哈希。
- 不修改或删除原文件。

## 生成清理计划

```bash
disk-indexer cleanup plan
```

参数：

```text
--target-volume <volume-id>
--keep-volume <volume-id>
--min-remaining-copies <n>
--output cleanup-plan.json
```

第一版只生成计划，不移动、不删除文件。

清理计划每一项必须包含：

```text
content hash
file size
候选删除副本
建议保留副本
所有已知副本
在线状态
生成时间
生成时数据库版本
风险提示
```

如果保留副本不在线、不可读、缺少完整哈希或数量不足，则该项必须标记为 blocked，不能进入可执行状态。

---

# 八、重复文件报告

文本报告应便于人阅读，例如：

```text
Duplicate Group
Hash: <blake3>
Size: 52.8 MiB
Known copies: 4
Online copies: 3
Offline copies: 1

[KEEP CANDIDATE]
Volume: MainPhotos
Role: primary
Path: 2024/Xinjiang/DSC00123.ARW
Status: present

[REDUNDANT CANDIDATE]
Volume: OldBackup
Role: legacy_backup
Path: Backup-2025/Xinjiang/DSC00123.ARW
Status: present

[OFFLINE]
Volume: OffsiteBackup
Role: offsite_backup
Path: Photos/2024/Xinjiang/DSC00123.ARW
Status: offline
```

同时提供稳定 JSON 输出，便于未来 GUI 或脚本使用。

JSON 字段命名应版本化，至少包含：

```text
schema_version
generated_at
database_path
groups
warnings
```

---

# 九、安全约束

这是本项目最高优先级要求。

1. 第一阶段不得实现永久删除命令。
2. 不得因为两个文件文件名、大小或时间相同就判定完全重复。
3. 快速指纹不能用于最终清理结论。
4. 硬盘离线不能导致副本被误判 missing 或 deleted。
5. 单次扫描出错不能回滚此前所有成功扫描结果。
6. 文件读取失败必须记录错误，不得静默跳过。
7. 所有查询和报告必须区分：
   - 已存在
   - 历史存在
   - 当前离线
   - 当前缺失
   - 无法验证
8. 清理计划不得把数据库文件、卷标识文件、系统目录或隔离目录列为候选。
9. 对路径大小写敏感性、Unicode 规范化和非 UTF-8 路径进行测试。
10. 同一内容在同一硬盘不同路径出现，仍然是多个副本。
11. 同一物理硬盘的两个分区不能默认等价于两份独立安全备份。
12. 报告应显式提示：
    - 内容重复不等于副本多余。
    - 备份策略应考虑不同物理介质和异地副本。

---

# 十、错误处理

至少处理以下异常：

- 扫描时硬盘拔出
- 权限不足
- 文件在扫描过程中被删除
- 文件扫描过程中发生变化
- 读取 I/O 错误
- SQLite 被占用
- 数据库迁移失败
- 卷 UUID 重复或 marker 冲突
- 路径包含不可打印字符
- 文件大于 4 GiB
- 空文件
- 稀疏文件
- 符号链接循环
- 深层目录
- 大量小文件
- 同名硬盘
- 只读硬盘

对于扫描期间发生变化的文件：

1. 读取前记录元数据。
2. 哈希完成后再次读取关键元数据。
3. 如大小或修改时间变化，则本次哈希作废。
4. 文件标记为 changed，并等待后续重新扫描。

---

# 十一、性能要求

性能重点是减少不必要读取，而不是盲目增加线程。

要求：

1. SQLite 批量事务写入。
2. 建立必要索引，至少覆盖：
   - `contents(file_size)`
   - `contents(full_hash, file_size)`
   - `file_copies(volume_id, relative_path)`
   - `file_copies(content_id)`
   - `file_copies(status)`
   - `scan_runs(volume_id, started_at)`
3. 默认并发数保守。
4. 同一机械硬盘优先顺序读取。
5. 可配置读取缓冲区。
6. 大文件采用流式哈希，不把整个文件读入内存。
7. 内存使用应与文件大小基本无关。
8. 百万级文件索引时不得把所有文件元数据一次性加载到内存。
9. CLI 输出进度时避免高频刷新导致性能下降。
10. 提供简单 benchmark 或性能测试工具，验证：
    - 大文件哈希吞吐
    - 大量小文件扫描
    - SQLite 批量写入

---

# 十二、测试要求

必须包含自动化测试。

## 单元测试

覆盖：

- 抽样范围计算
- 空文件哈希
- 小文件直接完整读取
- 大文件抽样
- 相同内容不同文件名
- 相同大小不同内容
- 文件扫描中途变化
- 路径编码
- volume UID 生成
- marker 文件解析
- 状态转换
- 理论可释放空间计算

## 集成测试

使用临时目录模拟：

1. 首次扫描。
2. 二次扫描复用元数据。
3. 新增文件。
4. 文件移动。
5. 文件改名。
6. 文件内容变化。
7. 文件删除。
8. 同一内容多个路径。
9. 模拟硬盘离线。
10. 扫描中断与继续。
11. 数据库迁移。
12. JSON 和 CSV 报告。

## 安全测试

- 不允许快速哈希直接产生“可安全删除”结论。
- 离线卷不得批量变成 missing。
- 缺少在线保留副本时，清理计划必须 blocked。
- 哈希过程中变化的文件不得写入可信 full_hash。
- 只读操作不得修改原文件。

---

# 十三、文档要求

完成以下文档：

## README.md

包括：

- 项目定位
- 当前能力
- 安装和构建
- CLI 示例
- 数据库存放位置
- 首次扫描建议
- 安全限制
- 当前未实现功能

## plan.md

列出：

- 当前阶段
- 已完成任务
- 待完成任务
- 技术决策
- 已知风险
- 后续 GUI 计划

## docs/architecture.md

描述：

- 模块边界
- 扫描流程
- 数据流
- 状态流转
- 未来 GUI 接入方式

## docs/database-schema.md

描述：

- 表结构
- 索引
- 唯一约束
- 数据迁移策略
- 每个状态的语义

## docs/scan-behavior.md

描述：

- 初次扫描
- 增量扫描
- 元数据复用
- 抽样哈希
- 完整哈希
- 中断恢复
- 离线硬盘处理

## docs/safety-model.md

明确写出：

- 为什么不能按文件名判断
- 为什么快速指纹不能用于删除
- 为什么离线副本不能视为不存在
- 为什么重复不一定多余
- 第一阶段为什么不提供永久删除

## docs/acceptance.md

逐项列出本 Prompt 中的验收条件及执行命令。

---

# 十四、分阶段实施

不要一次性堆积全部代码。按以下阶段推进，每阶段都必须保持可编译、可测试。

## Phase 0：仓库检查和方案确认

1. 检查当前仓库结构和已有代码。
2. 创建或更新 `plan.md`。
3. 记录关键技术决策。
4. 不删除已有有效代码。
5. 输出准备实施的阶段列表。

## Phase 1：基础工程和数据库

实现：

- Rust workspace 或单 crate 结构
- CLI 入口
- 配置目录
- SQLite 初始化
- migrations
- volumes、contents、file_copies、scan_runs、file_events
- `disk-indexer init`
- 基础测试

验收：

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo run -- init
```

## Phase 2：卷识别

实现：

- `volume add`
- `volume list`
- `volume show`
- marker 文件
- 只读卷回退识别
- 冲突报告
- 卷角色

验收：

- 同一卷重新挂载后能够识别为同一 volume。
- 两个同名卷不会被混淆。
- marker 冲突不会自动合并。

## Phase 3：元数据扫描

实现：

- 文件遍历
- 忽略规则
- 符号链接策略
- scan_runs
- 批量事务
- Ctrl+C 中断
- 基础进度
- 文件状态更新
- 离线卷保护

验收：

- 扫描临时目录并正确写入数据库。
- 二次扫描能识别新增、变化和缺失。
- 未连接的卷不会被标记 missing。

## Phase 4：哈希与增量复用

实现：

- sample hash
- full BLAKE3
- 读取前后元数据校验
- 元数据未变时复用哈希
- `hash complete`
- `lookup`

验收：

- 相同内容不同名称被识别。
- 相同大小不同内容不会被误判。
- 修改中的文件不会写入可信 full_hash。
- 大文件流式处理。

## Phase 5：重复报告

实现：

- `duplicates`
- 文本输出
- JSON 输出
- CSV 输出
- 在线、离线、missing 分组
- 理论可释放空间

验收：

- 每组副本位置完整。
- 报告明确区分在线和离线。
- JSON 结构稳定并有 schema_version。

## Phase 6：清理计划

实现：

- `cleanup plan`
- keep candidate
- redundant candidate
- blocked reason
- 不执行文件操作
- 计划 JSON

验收：

- 没有在线可信保留副本时必须 blocked。
- 缺少完整哈希时必须 blocked 或先要求补齐哈希。
- 不存在任何永久删除代码路径。

## Phase 7：稳定性与文档

完成：

- 全部自动化测试
- 错误场景测试
- 性能基准
- 文档
- 示例数据生成脚本
- 最终验收报告

---

# 十五、最终验收场景

创建三个临时“硬盘”目录：

```text
volume-a/
volume-b/
volume-c/
```

准备数据：

```text
volume-a/photo1.raw
volume-a/photo2.raw
volume-b/renamed-photo1.raw
volume-b/unique-video.mp4
volume-c/photo1-copy.raw
```

其中：

- `photo1.raw`
- `renamed-photo1.raw`
- `photo1-copy.raw`

内容完全相同。

执行：

```bash
disk-indexer init
disk-indexer volume add volume-a --role primary
disk-indexer volume add volume-b --role legacy_backup
disk-indexer volume add volume-c --role local_backup

disk-indexer scan volume-a
disk-indexer scan volume-b
disk-indexer scan volume-c

disk-indexer duplicates
disk-indexer lookup volume-b/renamed-photo1.raw
disk-indexer cleanup plan \
  --target-volume <volume-b-id> \
  --keep-volume <volume-a-id> \
  --min-remaining-copies 2 \
  --output cleanup-plan.json
```

预期：

1. 三个不同文件名和路径被归为同一内容组。
2. 报告显示 3 个副本。
3. `lookup` 显示该内容以前已经出现。
4. 清理计划在保留数量不足时 blocked。
5. 参数满足后才生成候选项。
6. 工具没有删除或移动任何原始文件。
7. 拔掉或模拟 volume-c 离线后，数据库仍保留该副本历史，并显示 offline，而不是 missing。
8. 再次扫描 volume-a 时，未变化文件复用旧哈希。

---

# 十六、编码规范

1. 所有公共结构和关键函数写清晰注释。
2. 避免超大文件和超长函数。
3. 不使用 `unwrap()` 处理正常运行路径。
4. 错误信息包含路径、卷和操作上下文。
5. 日志不得泄露不必要的文件内容。
6. SQL 集中管理，避免散落重复。
7. 时间统一存 UTC，展示时按本地时区。
8. 文件大小内部使用字节整数，展示时格式化。
9. JSON 输出不得混入人类进度日志。
10. 所有 CLI 命令支持非零退出码表达失败。
11. 对数据库 schema、JSON schema 和 sample hash 算法进行版本化。
12. 不为尚未实现的删除功能保留危险的半成品命令。

---

# 十七、本轮明确不做

以下功能不属于本轮范围：

- 永久删除文件
- 自动移动到废纸篓
- 隔离区执行
- 相似图片识别
- 连拍筛选
- EXIF 语义分析
- RAW 与 JPEG 配对
- 视频感知哈希
- ZIP、RAR 内部扫描
- 云盘同步
- NAS 服务端
- Web 管理后台
- 用户系统
- 自动备份
- 文件内容修复
- 文件系统驱动
- 实时 FSEvents 监听

可以在文档中设计扩展点，但不要提前实现。

---

# 十八、执行要求

1. 先检查仓库，再更新 `plan.md`。
2. 按 Phase 逐步实现。
3. 每完成一个 Phase：
   - 运行格式化
   - 运行 clippy
   - 运行测试
   - 更新文档和 `plan.md`
4. 不要跳过失败测试。
5. 不要为了通过测试删除关键安全检查。
6. 遇到不确定行为时，优先选择：
   - 只读
   - 保守判断
   - 标记为无法验证
   - 不生成可清理建议
7. 如果当前仓库为空，从 Phase 1 开始完整搭建。
8. 如果当前仓库已有实现，先复用和重构，不要无理由推倒重写。
9. 最终输出：
   - 已完成阶段
   - 关键文件
   - 数据库 schema
   - CLI 示例
   - 测试结果
   - 尚未实现内容
   - 已知风险
   - 下一阶段建议

开始执行。
