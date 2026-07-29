# 详细使用说明

## 1. 安装与构建

需要 Rust stable（项目的最低 Rust 版本由 `Cargo.toml` 声明）。从源码构建：

```bash
git clone https://github.com/taoking/disk-indexer.git
cd disk-indexer
cargo build --release
./target/release/disk-indexer --help
```

也可以直接在仓库目录使用 `cargo run -- <command>`。默认数据库是 macOS 的 `~/Library/Application Support/DiskIndexer/index.db`。使用 `--db /安全位置/index.db` 或 `DISK_INDEXER_DB` 可将数据库放到指定位置；建议不要把数据库放在正被扫描的移动硬盘根目录。

## 2. 建议的首次工作流

1. 初始化数据库：`disk-indexer init`。
2. 将每个真实硬盘/卷单独注册，并设置角色。
3. 逐卷执行普通扫描。初始阶段只读取同大小候选，减少不必要 I/O。
4. 查看重复组，先核实在线、离线和缺失状态。
5. 对希望精确快速查询的卷执行 `hash complete`。
6. 仅生成清理计划，人工复核后再使用自己的文件管理工具处理文件。

```bash
disk-indexer init
disk-indexer volume add /Volumes/MainPhotos --role primary
disk-indexer volume add /Volumes/OldBackup --role legacy_backup
disk-indexer scan /Volumes/MainPhotos
disk-indexer scan /Volumes/OldBackup
disk-indexer duplicates
disk-indexer hash complete --all
```

卷角色可选 `primary`、`local_backup`、`offsite_backup`、`temporary`、`legacy_backup` 和 `unknown`。可写卷默认创建 `.disk-indexer-volume-id`，确保重挂载后仍能识别同一卷。只读卷使用保守系统身份回退；无法得到稳定身份时不会冒险合并记录。

同一个 marker 不是同一块物理盘的充分证明。若命令输出 `possible_clone`，候选路径没有被写入或合并到历史卷，扫描也会停止。先查看冲突：

```bash
disk-indexer volume conflicts
disk-indexer volume conflicts --json
```

确认它是独立克隆盘后，保留为新卷（不会修改原卷记录）：

```bash
disk-indexer volume resolve --conflict 12 --as-new-volume --role local_backup
```

确认它只是同一块盘的新挂载路径时，可以请求重连；CLI 会再次比较稳定 UUID/设备身份，不一致或缺失时会拒绝：

```bash
disk-indexer volume relink --volume 3 --path /Volumes/Photos
```

## 3. CLI 参考

```bash
# 卷
disk-indexer volume list
disk-indexer volume show 1
disk-indexer volume add /Volumes/Photos --role primary --no-write-marker
disk-indexer volume conflicts --json
disk-indexer volume resolve --conflict 12 --as-new-volume
disk-indexer volume relink --volume 3 --path /Volumes/Photos

# 扫描
disk-indexer scan /Volumes/Photos --exclude '*.tmp'
disk-indexer scan /Volumes/Photos --metadata-only
disk-indexer scan /Volumes/Photos --full-hash
disk-indexer scan list
disk-indexer scan show 12 --json

# 查询、报告和验证
disk-indexer lookup /Volumes/NewDisk/DCIM/IMG_0001.RAW --full-hash --json
disk-indexer duplicates --min-copies 3 --online-only
disk-indexer duplicates --csv duplicates.csv
disk-indexer duplicates --json --page --after-content-id 0 --limit 50
disk-indexer stats --json
disk-indexer verify --volume 1 --full-hash

# 清理计划（只写 JSON，不处理文件）
disk-indexer cleanup plan --target-volume 2 --keep-volume 1 \
  --min-remaining-copies 2 --output cleanup-plan.json
```

`--json` 适用于脚本或未来 GUI；进度和警告不混入 JSON 标准输出。`duplicates` 的 CSV 将每个副本展开为一行。`lookup --full-hash` 会对路径当前的文件执行精确完整哈希；未传该参数时，抽样匹配只能作为候选提示。

`duplicates --json --page` 使用 `next_after_content_id` 游标逐页返回重复组，适合原生 App 和大报告浏览；它不与 `--csv` 一起使用，以免产生不完整导出。`stats --json` 返回只读概览统计（schema、卷、文件、完整哈希、可信重复组和理论空间），不会返回文件路径。

长任务供原生 App 或脚本消费时使用 `--jsonl-progress`。stdout 每行都是一个 JSON 事件，绝不混入普通提示文本；诊断日志仍写入 stderr。每个事件包含 `protocol_version`、`type`、`task_id`、`timestamp` 和 `operation`。可用操作：

```bash
disk-indexer scan /Volumes/Photos --jsonl-progress
disk-indexer hash complete --volume 1 --jsonl-progress
disk-indexer verify --volume 1 --full-hash --jsonl-progress
disk-indexer cleanup plan --target-volume 2 --keep-volume 1 \
  --output cleanup-plan.json --jsonl-progress
disk-indexer tasks --json
```

扫描会在每个已提交数据库批次后输出进度。向子进程发送 SIGINT 时，扫描、完整哈希和卷验证会在安全检查点停止，任务记录状态为 `interrupted`；不要把强制终止作为常规取消方式。

如果 `lookup` 返回 `cache_stale`、`metadata_matches_index: false` 或 `requires_rehash: true`，文件的大小、修改时间、inode 或设备号已与索引不符。工具不会复用旧完整哈希，也不会显示精确命中；使用 `lookup <path> --full-hash` 重新计算当前文件。

## 4. 清理计划如何阅读

`cleanup plan` 默认产生的 `candidate_unverified` 不是可直接执行的删除指令。它只代表目标副本当前在线、指定保留卷有在线可信副本，且移除目标路径后在线独立存储对象数量满足阈值。需要在生成时重新校验文件，可使用：

```bash
disk-indexer cleanup plan --target-volume 2 --keep-volume 1 \
  --min-remaining-copies 2 --min-remaining-physical-devices 2 \
  --verify-metadata --verify-full-hash --output cleanup-plan.json
```

严格验证全部通过才会得到 `verified_candidate`；任一候选或保留副本失效、剩余独立存储对象不足、物理设备不足或目标是硬链接路径时均为 `blocked`。工具不会执行删除。

每次实际人工清理前，应重新扫描相关卷，并对计划中的文件执行 `verify --file-copy <id> --full-hash`。同一物理硬盘的不同分区不等于独立备份；内容相同不等于副本多余。

## 6. 中断、离线和恢复

- 扫描中按 Ctrl+C：已提交的元数据保留，扫描记录标记为 `interrupted`。
- 使用 `scan <path> --resume` 继续执行幂等增量扫描。
- 某卷未挂载时，其副本显示为 `offline`，不会被标成 `missing`。
- 只有卷在线且一次遍历无错误完成后，本次未见的旧路径才成为 `missing`。
- 读取失败和扫描期间变化的文件会记录错误或 `changed` 状态；其完整哈希不会被信任。

## 7. macOS 原生 App

在装有 Xcode 15.4 或更高兼容版本的 Apple Silicon Mac 上，从仓库根目录执行：

```bash
scripts/build-macos-app.sh
open build/DerivedData/Build/Products/Debug/DiskIndexer.app
```

App 的侧边栏提供概览、硬盘、扫描任务、重复文件、文件查询、清理计划、设置和日志。长任务将 JSONL 实时进度显示在界面中；取消先向内置 CLI 发送 SIGINT，5 秒未退出才 `terminate`。关闭窗口时，若任务仍运行，必须选择等待或安全取消，应用不会静默遗留子进程。

重复组每次读取 50 组，避免完整报告进入 Swift 内存；页面内排序只针对当前已加载的数据。文件查询遇到 `cache_stale` 会明确提示重新计算完整哈希。清理计划页只能生成/导出 JSON，绝不会删除、移动或隔离原始文件。

设置页只允许切换到已经存在、经 Rust CLI 验证可以打开的 SQLite 文件；有运行任务时禁止切换，以避免错误创建或切换数据库。更多结构与协议约束见 [native-app-architecture.md](native-app-architecture.md)。

## 8. 故障排查

| 现象 | 处理方式 |
| --- | --- |
| 数据库被占用 | 稍后重试；数据库已配置 WAL 和 busy timeout。 |
| 卷身份冲突 | 不要复制 `.disk-indexer-volume-id`；保留原卷标记并为副本生成新标记。 |
| 找不到重复 | 普通扫描可能尚未计算完整哈希；运行 `hash complete --volume <id>`。 |

## 9. 性能检查

大文件采用固定缓冲区流式 BLAKE3。可用以下命令测量当前磁盘的单文件吞吐：

```bash
disk-indexer-benchmark /Volumes/Photos/large-file.raw
```

默认扫描保持顺序、单读取器策略。不要为机械硬盘盲目提高并发；`--max-readers` 当前会被保守限制为单读取器。
