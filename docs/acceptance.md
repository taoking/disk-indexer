# 验收

基础质量门：

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

最终场景：

```bash
root="$(mktemp -d)"
mkdir "$root"/{volume-a,volume-b,volume-c}
printf same > "$root/volume-a/photo1.raw"
printf other > "$root/volume-a/photo2.raw"
printf same > "$root/volume-b/renamed-photo1.raw"
printf video > "$root/volume-b/unique-video.mp4"
printf same > "$root/volume-c/photo1-copy.raw"

disk-indexer --db "$root/index.db" init
disk-indexer --db "$root/index.db" volume add "$root/volume-a" --role primary
disk-indexer --db "$root/index.db" volume add "$root/volume-b" --role legacy_backup
disk-indexer --db "$root/index.db" volume add "$root/volume-c" --role local_backup
disk-indexer --db "$root/index.db" scan "$root/volume-a"
disk-indexer --db "$root/index.db" scan "$root/volume-b"
disk-indexer --db "$root/index.db" scan "$root/volume-c"
disk-indexer --db "$root/index.db" duplicates
disk-indexer --db "$root/index.db" lookup "$root/volume-b/renamed-photo1.raw"
disk-indexer --db "$root/index.db" cleanup plan --target-volume 2 --keep-volume 1 \
  --min-remaining-copies 2 --output "$root/cleanup-plan.json"
```

预期重复组有 3 个副本；同大小不同内容不在组内；第二次扫描 `volume-a` 的 `metadata_reused_count` 为 2；清理计划只写 JSON，绝不改动源文件。将 `volume-c` 改名以模拟拔盘后，`duplicates` 仍显示该副本为 `offline`，不是 `missing`。自动化版本位于 `tests/integration.rs`。

长任务协议验收：

```bash
disk-indexer --db "$root/index.db" scan "$root/volume-a" --metadata-only --jsonl-progress
```

stdout 每一行必须是合法 JSON，包含相同的 `task_id`，并以 `task_completed` 结束；不会启动浏览器或监听网络端口。

原生 App 验收：

```bash
xcodebuild \
  -project apps/macos/DiskIndexerApp.xcodeproj \
  -scheme DiskIndexerApp \
  -destination 'platform=macOS,arch=arm64' \
  CODE_SIGNING_ALLOWED=NO \
  test
scripts/build-macos-app.sh
test -x build/DerivedData/Build/Products/Debug/DiskIndexer.app/Contents/Resources/disk-indexer
open build/DerivedData/Build/Products/Debug/DiskIndexer.app
```

打开后确认侧边栏含概览、硬盘、扫描任务、重复文件、文件查询、清理计划、设置和日志；扫描能显示 JSONL 进度并可取消，清理页只有生成/导出计划，不含删除、废纸篓、隔离或一键清理按钮。
