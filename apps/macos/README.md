# DiskIndexer macOS App

这是 macOS 14+、Apple Silicon 优先的 SwiftUI 原生界面。它不使用 WebView、浏览器、HTTP 或本地端口；所有业务操作通过 App Bundle `Contents/Resources/disk-indexer` 中的 Rust CLI 完成。

构建与运行：

```bash
scripts/build-macos-app.sh
open build/DerivedData/Build/Products/Debug/DiskIndexer.app
```

脚本先构建 Rust release 二进制，再构建 Xcode 项目并拷贝工具进入 App Bundle。Swift 端始终通过 `Bundle.main` 查找工具，命令用 `Process.arguments` 数组传递，且每次都显式传入数据库路径。应用不依赖用户的 Rust 安装或 `PATH`。

首次打开后，选择“硬盘”注册目录或卷，再到“扫描任务”启动仅元数据、增量或完整哈希扫描。任务页会显示 JSONL 实时进度，可安全取消；历史任务来自 SQLite 的 `task_runs`。重复文件页按内容 ID 每次读取 50 组；文件查询会在索引过期时提示重新计算完整哈希；清理计划页只生成和导出 JSON，从不删除文件。

关闭运行中任务时，应用会要求选择继续等待或安全取消。安全取消先发送 SIGINT，5 秒后才调用 `terminate`，不会使用 `kill -9`。设置页只能选择已存在且由 Rust CLI 成功打开的数据库，避免切换时静默创建错误文件。

本地开发质量门：

```bash
xcodebuild \
  -project apps/macos/DiskIndexerApp.xcodeproj \
  -scheme DiskIndexerApp \
  -destination 'platform=macOS,arch=arm64' \
  CODE_SIGNING_ALLOWED=NO \
  test
```

当前构建脚本是 arm64。Universal 2 是未来扩展：需要分别构建 Rust 的 arm64/x86_64，再用 `lipo` 合并后才可拷进 Bundle。
