# Disk Indexer

`disk-indexer` 是一个本地运行的多硬盘文件资产索引工具。它把文件元数据、抽样指纹和可信完整 BLAKE3 哈希保存在 SQLite 中，以便在硬盘离线时仍能查询历史副本，并识别字节完全相同的文件。项目提供 CLI，并为 macOS 原生 App 提供本地 JSON/JSONL 子进程协议。

它的目标是帮助人工整理长期重复备份，不是自动删除工具。

## 当前能力

- 注册卷并优先写入隐藏 UUID 标记；marker 只代表“可能是同一逻辑卷”，还会校验稳定设备身份。
- 扫描普通文件、保存原始路径字节、默认不跟随符号链接。
- 复用大小、纳秒修改时间、inode 和设备号均不变的已有哈希。
- 用 BLAKE3 抽样缩小候选，再用完整 BLAKE3 + 文件大小确认重复。
- 查询文件时校验元数据；缓存过期时不会把旧完整哈希当作精确结论。
- 重复报告区分路径、独立存储对象、逻辑卷和物理设备；硬链接不虚增可释放空间。
- 生成只读清理候选计划；没有删除、移动或废纸篓命令。
- 大查询使用 keyset 分页；扫描、补哈希、验证和清理计划可输出稳定 JSON Lines 任务事件。

## 安装和构建

需要 Rust stable：

```bash
cargo build --release
./target/release/disk-indexer --help
./target/release/disk-indexer-benchmark /Volumes/Photos/large-file.raw
```

### macOS 原生 App

macOS 14+（Apple Silicon 优先）可构建普通的 SwiftUI App，无需用户安装 Rust，也不使用浏览器、WebView、HTTP 或端口：

```bash
scripts/build-macos-app.sh
open build/DerivedData/Build/Products/Debug/DiskIndexer.app
```

构建脚本会把 `target/release/disk-indexer` 放入 `DiskIndexer.app/Contents/Resources/disk-indexer`。应用只通过 `Bundle.main` 和 `Process.arguments` 调用该文件，所有操作显式传入数据库路径。完整界面说明见 [apps/macos/README.md](apps/macos/README.md) 和 [docs/native-app-architecture.md](docs/native-app-architecture.md)。

默认数据库位置为 macOS 的 `~/Library/Application Support/DiskIndexer/index.db`。可通过 `--db /path/index.db` 或 `DISK_INDEXER_DB` 覆盖。

## 常用命令

```bash
disk-indexer init
disk-indexer volume add /Volumes/Photos --role primary
disk-indexer volume list
disk-indexer volume conflicts
disk-indexer scan /Volumes/Photos
disk-indexer hash complete --all
disk-indexer duplicates --csv duplicates.csv
disk-indexer stats --json
disk-indexer lookup /Volumes/NewDisk/IMG_0001.RAW --full-hash
disk-indexer verify --volume 1 --full-hash
disk-indexer cleanup plan --target-volume 2 --keep-volume 1 \
  --min-remaining-copies 2 --output cleanup-plan.json
```

### 卷身份冲突

若某块已注册硬盘离线后，另一块硬盘带着相同的 `.disk-indexer-volume-id` 出现，工具不会覆盖离线卷记录，也不会自动开始扫描候选盘。`volume add --json` 会返回 `possible_clone` 和冲突 ID；先审核 `disk-indexer volume conflicts`，再明确执行：

```bash
# 确认候选盘是独立克隆盘：保留成新的内部卷记录，不合并历史
disk-indexer volume resolve --conflict 12 --as-new-volume --role local_backup

# 确认同一块盘只是换了挂载路径：只有稳定设备身份一致时才允许重连
disk-indexer volume relink --volume 3 --path /Volumes/Photos
```

两条命令都会写入本地 SQLite 审计事件。没有稳定设备 UUID/序列号可验证时，重连会被拒绝；此时应保留冲突并人工核对硬盘。

首次扫描建议先逐卷运行普通 `scan`，让工具只对同大小候选读取内容；需要“任意新文件都可快速精确比对”时，再分批运行 `hash complete --volume <id>`。

详尽的首次使用、命令参数、JSONL 任务协议、恢复与故障排查见 [docs/usage.md](docs/usage.md)。macOS 原生 App 已通过内置 CLI 子进程调用这些协议，不启动浏览器或本地 HTTP 服务。

## 安全限制

- 只有“文件大小相同且完整 BLAKE3 相同”才会进入重复组。
- 快速指纹仅用于筛选，不能成为清理结论。
- `lookup` 的 `cache_stale` 表示当前元数据与索引不一致；必须使用 `--full-hash` 重新计算后才会给出精确结论。
- 默认清理计划是 `candidate_unverified`，只是人工审核材料。只有 `--verify-metadata` 或 `--verify-full-hash` 成功后才会标为 `verified_candidate`。
- 同一存储对象的多个硬链接只算一个对象；删除其中一个路径不会被建议为释放完整文件空间。
- 卷离线只会显示为 `offline`，不会批量标记为 `missing`。
- 扫描出错或中断时，不会据此把未遍历文件标记为缺失。
- 内容重复不等于副本多余；清理时仍须考虑不同物理介质、恢复需求和异地副本。
- 工具从不修改被扫描的文件；卷注册默认会在可写卷根创建 `.disk-indexer-volume-id`，可用 `--no-write-marker` 禁用。

## 当前未实现

永久删除、移入废纸篓、隔离区执行、相似照片/视频识别、EXIF 分析、RAW/JPEG 关联、压缩包内部去重、云端/NAS/远程服务和实时 FSEvents 监听均不在本阶段范围内。

更多设计与验收说明见 [docs](docs/architecture.md)。
