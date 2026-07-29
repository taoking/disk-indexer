# Disk Indexer

`disk-indexer` 是一个本地运行的多硬盘文件资产索引工具。它把文件元数据、抽样指纹和可信完整 BLAKE3 哈希保存在 SQLite 中，以便在硬盘离线时仍能查询历史副本，并识别字节完全相同的文件。项目提供 CLI 与仅本机可访问的浏览器 UI。

它的目标是帮助人工整理长期重复备份，不是自动删除工具。

## 当前能力

- 注册卷并优先写入隐藏 UUID 标记；不以卷名作为身份。
- 扫描普通文件、保存原始路径字节、默认不跟随符号链接。
- 复用大小、纳秒修改时间、inode 和设备号均不变的已有哈希。
- 用 BLAKE3 抽样缩小候选，再用完整 BLAKE3 + 文件大小确认重复。
- 查询文件、报告重复组、导出 JSON/CSV、验证记录。
- 生成只读清理候选计划；没有删除、移动或废纸篓命令。

## 安装和构建

需要 Rust stable：

```bash
cargo build --release
./target/release/disk-indexer --help
./target/release/disk-indexer-benchmark /Volumes/Photos/large-file.raw
```

默认数据库位置为 macOS 的 `~/Library/Application Support/DiskIndexer/index.db`。可通过 `--db /path/index.db` 或 `DISK_INDEXER_DB` 覆盖。

## 常用命令

```bash
disk-indexer init
disk-indexer volume add /Volumes/Photos --role primary
disk-indexer volume list
disk-indexer scan /Volumes/Photos
disk-indexer hash complete --all
disk-indexer duplicates --csv duplicates.csv
disk-indexer lookup /Volumes/NewDisk/IMG_0001.RAW --full-hash
disk-indexer verify --volume 1 --full-hash
disk-indexer cleanup plan --target-volume 2 --keep-volume 1 \
  --min-remaining-copies 2 --output cleanup-plan.json
disk-indexer ui
```

首次扫描建议先逐卷运行普通 `scan`，让工具只对同大小候选读取内容；需要“任意新文件都可快速精确比对”时，再分批运行 `hash complete --volume <id>`。

## 本机 UI 与展示页

运行：

```bash
disk-indexer ui
# 使用其他端口，或不自动启动浏览器
disk-indexer ui --port 48153 --no-open
```

页面默认打开 `http://127.0.0.1:48152`，可展示卷在线状态、可信重复组和理论可释放空间，并可注册卷、启动扫描、补齐完整哈希及预览清理计划。它**只绑定本机回环地址**，不会上传文件路径、哈希或数据库；页面和 API 都没有删除或移动文件的能力。按 Ctrl+C 可关闭服务。

详尽的首次使用、命令参数、UI 操作顺序、恢复与故障排查见 [docs/usage.md](docs/usage.md)。展示页面源文件是 [ui/index.html](ui/index.html)。

## 安全限制

- 只有“文件大小相同且完整 BLAKE3 相同”才会进入重复组。
- 快速指纹仅用于筛选，不能成为清理结论。
- 卷离线只会显示为 `offline`，不会批量标记为 `missing`。
- 扫描出错或中断时，不会据此把未遍历文件标记为缺失。
- 内容重复不等于副本多余；清理时仍须考虑不同物理介质、恢复需求和异地副本。
- 工具从不修改被扫描的文件；卷注册默认会在可写卷根创建 `.disk-indexer-volume-id`，可用 `--no-write-marker` 禁用。

## 当前未实现

永久删除、移入废纸篓、隔离区执行、相似照片/视频识别、EXIF 分析、RAW/JPEG 关联、压缩包内部去重、云端/NAS/远程 Web 服务和实时 FSEvents 监听均不在本阶段范围内。当前 UI 仅是本机回环地址上的操作界面。

更多设计与验收说明见 [docs](docs/architecture.md)。
