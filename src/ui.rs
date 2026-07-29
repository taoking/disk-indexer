//! 仅供本机使用的浏览器 UI。它不监听局域网地址，也不包含删除操作。

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};
use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::Html,
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::config::Config;
use crate::db::Database;
use crate::duplicate::{DuplicateFilter, duplicate_groups};
use crate::model::VolumeRole;
use crate::report::{create_cleanup_plan, duplicate_report};
use crate::scanner::{ScanOptions, complete_hashes, scan};
use crate::volume::{MarkerPolicy, register_volume};

const DASHBOARD_HTML: &str = include_str!("../ui/index.html");

#[derive(Clone)]
struct UiState {
    config: Config,
}

type ApiResult = Result<Json<Value>, (StatusCode, Json<Value>)>;

#[derive(Debug, Deserialize)]
struct VolumeRequest {
    path: PathBuf,
    #[serde(default = "unknown_role")]
    role: String,
    #[serde(default = "default_true")]
    write_marker: bool,
}

#[derive(Debug, Deserialize)]
struct ScanRequest {
    path: PathBuf,
    #[serde(default)]
    full_hash: bool,
    #[serde(default)]
    metadata_only: bool,
    #[serde(default)]
    excludes: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CompleteHashRequest {
    volume_id: Option<i64>,
    #[serde(default)]
    all: bool,
}

#[derive(Debug, Deserialize)]
struct CleanupRequest {
    target_volume_id: i64,
    keep_volume_id: i64,
    #[serde(default = "default_remaining_copies")]
    min_remaining_copies: usize,
}

fn unknown_role() -> String {
    "unknown".to_owned()
}

const fn default_true() -> bool {
    true
}

const fn default_remaining_copies() -> usize {
    1
}

/// 启动 UI 并阻塞至进程终止。服务器严格绑定至本机回环地址。
pub fn run_local_ui(config: &Config, port: u16, open_browser: bool) -> Result<()> {
    Database::open(config).context("无法初始化 UI 所需的数据库")?;
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let url = format!("http://{address}");
    println!("Disk Indexer UI 已启动: {url}");
    println!("安全边界：仅监听 127.0.0.1，不会把文件路径或索引上传到网络。");
    if open_browser {
        if let Err(error) = webbrowser::open(&url) {
            eprintln!("未能自动打开浏览器，请手动访问 {url}: {error}");
        }
    }
    let state = UiState {
        config: config.clone(),
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("无法创建 UI 运行时")?;
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(address)
            .await
            .with_context(|| format!("无法绑定本机 UI 地址 {address}"))?;
        axum::serve(listener, app(state))
            .with_graceful_shutdown(async {
                if let Err(error) = tokio::signal::ctrl_c().await {
                    eprintln!("UI 无法监听 Ctrl+C: {error}");
                }
            })
            .await
            .context("本机 UI 服务异常终止")
    })
}

fn app(state: UiState) -> Router {
    Router::new()
        .route("/", get(page))
        .route("/api/overview", get(overview))
        .route("/api/volumes", post(add_volume))
        .route("/api/scan", post(scan_volume))
        .route("/api/hash/complete", post(complete_hash))
        .route("/api/cleanup-plan", post(cleanup_plan))
        .with_state(state)
}

async fn page() -> Html<&'static str> {
    Html(DASHBOARD_HTML)
}

async fn overview(State(state): State<UiState>) -> ApiResult {
    let mut database = open_database(&state).map_err(api_error)?;
    database.refresh_volume_online_states().map_err(api_error)?;
    let volumes = database.volumes().map_err(api_error)?;
    let groups = duplicate_groups(&database, DuplicateFilter::default()).map_err(api_error)?;
    let report = duplicate_report(&database, &groups);
    Ok(Json(json!({
        "database_path": database.path().display().to_string(),
        "schema_version": database.schema_version().map_err(api_error)?,
        "volumes": volumes.iter().map(volume_json).collect::<Vec<_>>(),
        "duplicate_report": report,
    })))
}

async fn add_volume(State(state): State<UiState>, Json(request): Json<VolumeRequest>) -> ApiResult {
    let role = request.role.parse::<VolumeRole>().map_err(api_message)?;
    let mut database = open_database(&state).map_err(api_error)?;
    let policy = if request.write_marker {
        MarkerPolicy::WriteIfPossible
    } else {
        MarkerPolicy::DoNotWrite
    };
    let registration =
        register_volume(&mut database, &request.path, role, policy).map_err(api_error)?;
    Ok(Json(json!({
        "volume": registration.volume.as_ref().map(volume_json),
        "marker_uid": registration.marker_uid,
        "writable": registration.writable,
        "used_fallback_identity": registration.used_fallback_identity,
        "identity_state": registration.identity_state.as_str(),
        "identity_conflict": registration.conflict.as_ref().map(|conflict| json!({
            "id": conflict.id,
            "state": conflict.state,
            "existing_volume_id": conflict.existing_volume_id,
            "candidate_mount_path": conflict.candidate_mount_path.to_string_lossy(),
        })),
    })))
}

async fn scan_volume(State(state): State<UiState>, Json(request): Json<ScanRequest>) -> ApiResult {
    if request.full_hash && request.metadata_only {
        return Err(api_message("完整哈希与仅元数据模式不能同时启用".to_owned()));
    }
    let canonical_root = request
        .path
        .canonicalize()
        .with_context(|| format!("无法访问扫描目录 {}", request.path.display()))
        .map_err(api_error)?;
    let mut database = open_database(&state).map_err(api_error)?;
    let role = database
        .find_volume_for_path(&canonical_root)
        .map_err(api_error)?
        .filter(|volume| volume.mount_path == canonical_root)
        .map_or(VolumeRole::Unknown, |volume| volume.role);
    let registration = register_volume(
        &mut database,
        &canonical_root,
        role,
        MarkerPolicy::WriteIfPossible,
    )
    .map_err(api_error)?;
    let volume = registration
        .volume
        .context("检测到 possible_clone；请先审核卷身份冲突")
        .map_err(api_error)?;
    let summary = scan(
        &mut database,
        &state.config,
        &volume,
        &ScanOptions {
            full_hash: request.full_hash,
            metadata_only: request.metadata_only,
            excludes: request.excludes,
            ..ScanOptions::default()
        },
    )
    .map_err(api_error)?;
    Ok(Json(json!(summary)))
}

async fn complete_hash(
    State(state): State<UiState>,
    Json(request): Json<CompleteHashRequest>,
) -> ApiResult {
    if !request.all && request.volume_id.is_none() {
        return Err(api_message("请选择一个卷或启用全部卷".to_owned()));
    }
    if request.all && request.volume_id.is_some() {
        return Err(api_message("不能同时选择单卷和全部卷".to_owned()));
    }
    let mut database = open_database(&state).map_err(api_error)?;
    database.refresh_volume_online_states().map_err(api_error)?;
    let stats =
        complete_hashes(&mut database, &state.config, request.volume_id).map_err(api_error)?;
    Ok(Json(json!({
        "sampled": stats.sampled,
        "full_hashed": stats.full_hashed,
        "errors": stats.errors,
        "bytes_read": stats.bytes_read,
    })))
}

async fn cleanup_plan(
    State(state): State<UiState>,
    Json(request): Json<CleanupRequest>,
) -> ApiResult {
    let mut database = open_database(&state).map_err(api_error)?;
    database.refresh_volume_online_states().map_err(api_error)?;
    let plan = create_cleanup_plan(
        &database,
        request.target_volume_id,
        request.keep_volume_id,
        request.min_remaining_copies,
    )
    .map_err(api_error)?;
    Ok(Json(json!(plan)))
}

fn open_database(state: &UiState) -> Result<Database> {
    Database::open(&state.config)
}

fn volume_json(volume: &crate::model::Volume) -> Value {
    json!({
        "id": volume.id,
        "volume_uid": volume.volume_uid,
        "marker_uid": volume.marker_uid,
        "volume_name": volume.volume_name,
        "filesystem": volume.filesystem,
        "mount_path": volume.mount_path.to_string_lossy(),
        "role": volume.role.as_str(),
        "is_online": volume.is_online,
        "first_seen_at": volume.first_seen_at,
        "last_seen_at": volume.last_seen_at,
    })
}

fn api_error(error: anyhow::Error) -> (StatusCode, Json<Value>) {
    api_message(error.to_string())
}

fn api_message(message: String) -> (StatusCode, Json<Value>) {
    (StatusCode::BAD_REQUEST, Json(json!({"error": message})))
}

#[cfg(test)]
mod tests {
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    use super::{DASHBOARD_HTML, UiState, app};
    use crate::config::Config;

    #[test]
    fn bundled_page_explains_local_only_boundary() {
        assert!(DASHBOARD_HTML.contains("127.0.0.1"));
        assert!(DASHBOARD_HTML.contains("不会删除"));
        assert!(DASHBOARD_HTML.contains("/api/overview"));
    }

    #[tokio::test]
    async fn dashboard_and_overview_routes_respond() {
        let temp = tempfile::tempdir().expect("temporary ui db");
        let config = Config::new(Some(temp.path().join("index.db"))).expect("config");
        let application = app(UiState { config });
        let page = application
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("page response");
        assert_eq!(page.status(), axum::http::StatusCode::OK);
        let overview = application
            .oneshot(
                Request::builder()
                    .uri("/api/overview")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("overview response");
        assert_eq!(overview.status(), axum::http::StatusCode::OK);
    }
}
