# JSONL 任务协议

版本 1 用于 macOS 原生 App 与内置 `disk-indexer` 子进程之间的长任务通信，不使用 HTTP、端口或 shell。

所有事件通过 stdout 单行 JSON 输出，且必含：`protocol_version: 1`、`type`、`task_id`、`timestamp`、`operation`。stderr 只用于日志和诊断。

```json
{"protocol_version":1,"type":"task_started","task_id":"…","timestamp":"…","operation":"scan","status":"running"}
{"protocol_version":1,"type":"progress","task_id":"…","timestamp":"…","operation":"scan","files_seen":500,"bytes_read":0,"current_path":"…"}
{"protocol_version":1,"type":"task_completed","task_id":"…","timestamp":"…","operation":"scan","status":"completed","summary":{}}
```

`task_completed.status` 为 `completed`、`completed_with_errors`、`interrupted` 或 `failed`。每个任务也写入 SQLite 的 `task_runs`，可通过 `disk-indexer tasks --json` 进行分页读取。调用方取消时先发送 SIGINT，等待清理和状态持久化；只有进程无响应时才考虑终止。
