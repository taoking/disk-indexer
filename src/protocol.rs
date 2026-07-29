//! SwiftUI 子进程桥接使用的稳定 JSON Lines 任务协议。

use std::io::{self, Write};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::db::Database;
use crate::util::now;

pub const TASK_PROTOCOL_VERSION: u32 = 1;

/// 所有长任务共用的 JSONL 进度负载。字段始终序列化，避免消费者把“缺失”误判为零。
#[derive(Debug, Clone, Default, Serialize)]
pub struct TaskProgress {
    pub files_seen: u64,
    pub files_processed: u64,
    pub files_reused: u64,
    pub files_skipped: u64,
    pub files_sampled: u64,
    pub files_full_hashed: u64,
    pub files_verified: u64,
    pub files_failed: u64,
    pub groups_seen: u64,
    pub groups_processed: u64,
    pub bytes_read: u64,
    pub current_path: Option<String>,
    pub current_group_hash: Option<String>,
}

#[derive(Debug)]
struct ProgressEmissionState {
    last_emitted_at: Instant,
    last_files_processed: u64,
    last_groups_processed: u64,
}

#[derive(Debug)]
pub struct JsonlTask {
    database_id: i64,
    task_uid: String,
    operation: String,
    progress_state: Mutex<ProgressEmissionState>,
}

impl JsonlTask {
    pub fn start(database: &mut Database, operation: &str) -> Result<Self> {
        let task_uid = Uuid::new_v4().to_string();
        let database_id = database.create_task_run(&task_uid, operation)?;
        let task = Self {
            database_id,
            task_uid,
            operation: operation.to_owned(),
            progress_state: Mutex::new(ProgressEmissionState {
                last_emitted_at: Instant::now(),
                last_files_processed: 0,
                last_groups_processed: 0,
            }),
        };
        task.emit("task_started", json!({"status": "running"}))?;
        Ok(task)
    }

    #[must_use]
    pub fn task_uid(&self) -> &str {
        &self.task_uid
    }

    pub fn progress(&self, progress: &TaskProgress) {
        self.emit_progress(progress, false);
    }

    pub fn progress_force(&self, progress: &TaskProgress) {
        self.emit_progress(progress, true);
    }

    fn emit_progress(&self, progress: &TaskProgress, force: bool) {
        let mut state = match self.progress_state.lock() {
            Ok(state) => state,
            Err(_) => return,
        };
        let now = Instant::now();
        let file_delta = progress
            .files_processed
            .saturating_sub(state.last_files_processed);
        let group_delta = progress
            .groups_processed
            .saturating_sub(state.last_groups_processed);
        if !force
            && file_delta < 100
            && group_delta == 0
            && now.duration_since(state.last_emitted_at) < Duration::from_millis(300)
        {
            return;
        }
        if self
            .emit(
                "progress",
                serde_json::to_value(progress).unwrap_or_else(|_| json!({})),
            )
            .is_ok()
        {
            state.last_emitted_at = now;
            state.last_files_processed = progress.files_processed;
            state.last_groups_processed = progress.groups_processed;
        }
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
