//! SwiftUI 子进程桥接使用的稳定 JSON Lines 任务协议。

use std::io::{self, Write};

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::db::Database;
use crate::util::now;

pub const TASK_PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub struct JsonlTask {
    database_id: i64,
    task_uid: String,
    operation: String,
}

impl JsonlTask {
    pub fn start(database: &mut Database, operation: &str) -> Result<Self> {
        let task_uid = Uuid::new_v4().to_string();
        let database_id = database.create_task_run(&task_uid, operation)?;
        let task = Self {
            database_id,
            task_uid,
            operation: operation.to_owned(),
        };
        task.emit("task_started", json!({"status": "running"}))?;
        Ok(task)
    }

    #[must_use]
    pub fn task_uid(&self) -> &str {
        &self.task_uid
    }

    pub fn progress(&self, payload: Value) {
        let _ = self.emit("progress", payload);
    }

    pub fn complete<T: Serialize>(
        &self,
        database: &mut Database,
        status: &str,
        summary: &T,
    ) -> Result<()> {
        let summary = serde_json::to_value(summary)?;
        database.update_task_progress(self.database_id, &summary)?;
        database.finish_task_run(self.database_id, status, Some(&summary), None)?;
        self.emit(
            "task_completed",
            json!({"status": status, "summary": summary}),
        )
    }

    pub fn fail(&self, database: &mut Database, error: &anyhow::Error) -> Result<()> {
        database.finish_task_run(
            self.database_id,
            "failed",
            None,
            Some(&format!("{error:#}")),
        )?;
        self.emit(
            "task_completed",
            json!({"status": "failed", "error": format!("{error:#}")}),
        )
    }

    fn emit(&self, event_type: &str, payload: Value) -> Result<()> {
        let mut event = serde_json::Map::new();
        event.insert("protocol_version".to_owned(), json!(TASK_PROTOCOL_VERSION));
        event.insert("type".to_owned(), json!(event_type));
        event.insert("task_id".to_owned(), json!(self.task_uid));
        event.insert("timestamp".to_owned(), json!(now()));
        event.insert("operation".to_owned(), json!(self.operation));
        match payload {
            Value::Object(payload) => event.extend(payload),
            value => {
                event.insert("payload".to_owned(), value);
            }
        }
        let stdout = io::stdout();
        let mut output = stdout.lock();
        serde_json::to_writer(&mut output, &event).context("无法编码 JSONL 任务事件")?;
        output.write_all(b"\n").context("无法写入 JSONL 任务事件")?;
        output.flush().context("无法刷新 JSONL 任务事件")
    }
}
