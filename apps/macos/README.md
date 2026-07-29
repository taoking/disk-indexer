# DiskIndexer macOS App

这是 macOS 14+、Apple Silicon 优先的 SwiftUI 原生界面。它不使用 WebView、浏览器、HTTP 或本地端口；所有业务操作通过 App Bundle `Contents/Resources/disk-indexer` 中的 Rust CLI 完成。

构建与运行：

```bash
scripts/build-macos-app.sh
open build/DerivedData/Build/Products/Debug/DiskIndexer.app
```

脚本先构建 Rust release 二进制，再构建 Xcode 项目并拷贝工具进入 App Bundle。Swift 端始终通过 `Bundle.main` 查找工具，命令用 `Process.arguments` 数组传递，且每次都显式传入数据库路径。
