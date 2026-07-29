# App Bundle 资源

构建脚本会把 Rust release 可执行文件复制为：

```text
DiskIndexer.app/Contents/Resources/disk-indexer
```

该文件不提交到源码仓库。Swift 端只通过 `Bundle.main` 定位它，不依赖用户的 `PATH`。
