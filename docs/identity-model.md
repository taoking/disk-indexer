# 卷身份、存储对象与重复判定

## 卷不是 marker

卷根的 `.disk-indexer-volume-id` 只能说明“可能是同一个逻辑卷”。注册还会比较系统卷 UUID、分区 UUID、media UUID、设备序列号、文件系统、容量、当前路径和历史物理设备。如果 marker 相同但稳定信息冲突，或缺少稳定信息且路径不同，结果为 `possible_clone`：不覆盖原 `volumes` 记录、不自动合并，也不会开始扫描。

`physical_devices` 为可获得稳定身份的设备提供分组基础，`volumes.identity_state` 使用 `verified`、`fallback`、`possible_clone`、`conflict` 或 `manual_link`。人工操作只能通过审计命令完成：

```bash
disk-indexer volume conflicts --json
disk-indexer volume resolve --conflict 12 --as-new-volume
disk-indexer volume relink --volume 3 --path /Volumes/NewMount
```

`relink` 需要稳定设备身份匹配；`as-new-volume` 生成新的内部 UID。两者都会保留历史副本并记录 `volume_events`。

## 路径不是独立副本

`file_copies` 中的 `storage_object_key` 由物理设备、device ID 和 inode 组成。同一 inode 的硬链接会产生多个路径，但只代表一个独立存储对象；`link_group_id` 用于展示关联关系。重复报告同时显示：

- `path_count`：目录路径数量；
- `storage_object_count`：实际可独立释放空间的对象数；
- `logical_volume_count`：逻辑卷数；
- `physical_device_count`：物理设备分组数。

理论可释放空间只以在线、`present` 且独立的存储对象计算。离线卷历史显示为 `offline_unverified`，不能当作当前可验证副本。`missing`、`changed`、`unreadable` 等异常状态不成为冗余候选。

## 哈希与清理

只有“文件大小相同 + 完整 BLAKE3 相同”才是内容重复。抽样哈希只用于缩小候选。`lookup` 复用索引完整哈希前，会比较大小、纳秒 mtime、inode 和 device ID；任一不匹配是 `cache_stale`，需要 `--full-hash` 才能给出精确结论。

清理计划从不执行文件操作。默认 `candidate_unverified`；开启元数据或完整哈希验证且所有候选/保留副本通过后，才是 `verified_candidate`。硬链接、物理设备数量不足、保留副本不足或验证失败一律为 `blocked`。
