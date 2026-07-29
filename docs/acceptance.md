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

本机 UI 验收：

```bash
disk-indexer --db "$root/index.db" ui --no-open --port 48152
curl --fail http://127.0.0.1:48152/
curl --fail http://127.0.0.1:48152/api/overview
```

页面必须解释 `127.0.0.1` 安全边界，概览 API 必须返回 schema 版本和卷数据。UI 路由单测位于 `src/ui.rs`；按 Ctrl+C 应优雅停止服务。
