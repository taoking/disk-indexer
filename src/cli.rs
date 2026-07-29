//! CLI 适配层：只处理参数、输出与退出状态，不承载索引业务逻辑。

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use crate::config::Config;
use crate::db::Database;
use crate::duplicate::{DuplicateFilter, duplicate_groups, lookup, verify_record};
use crate::model::VolumeRole;
use crate::protocol::JsonlTask;
use crate::report::{
    CleanupVerificationOptions, create_cleanup_plan_with_verification, duplicate_report,
    render_cleanup_plan_text, render_duplicates_text, render_lookup_text, write_cleanup_plan,
    write_csv,
};
use crate::scanner::{
    ScanOptions, complete_hashes, complete_hashes_with_cancel, interrupt_flag, scan,
};
use crate::ui::run_local_ui;
use crate::volume::{MarkerPolicy, register_volume, relink_volume, resolve_conflict_as_new_volume};

#[derive(Debug, Parser)]
#[command(
    name = "disk-indexer",
    version,
    about = "本地多硬盘内容索引与重复备份识别工具"
)]
pub struct Cli {
    /// 覆盖数据库路径（也可设置 DISK_INDEXER_DB）
    #[arg(long, global = true, env = "DISK_INDEXER_DB")]
    pub db: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// 创建应用数据目录并初始化或升级数据库
    Init,
    /// 注册、查看逻辑卷
    Volume {
        #[command(subcommand)]
        command: VolumeCommand,
    },
    /// 扫描卷，或查看扫描历史
    Scan(ScanCommand),
    /// 为已有文件补齐可信完整哈希
    Hash {
        #[command(subcommand)]
        command: HashCommand,
    },
    /// 查询文件曾否出现及其副本
    Lookup(LookupArgs),
    /// 输出完整哈希确认的重复内容组
    Duplicates(DuplicatesArgs),
    /// 只读验证数据库中的文件记录
    Verify(VerifyArgs),
    /// 查看可供原生 App 恢复展示的任务历史
    Tasks(TasksArgs),
    /// 生成只供人工审核的清理候选计划，不会删除文件
    Cleanup {
        #[command(subcommand)]
        command: CleanupCommand,
    },
    /// 启动仅本机可访问的浏览器操作界面
    Ui(UiArgs),
}

#[derive(Debug, Subcommand)]
enum VolumeCommand {
    Add(VolumeAddArgs),
    List {
        #[arg(long)]
        json: bool,
    },
    Show {
        volume_id: i64,
        #[arg(long)]
        json: bool,
    },
    /// 列出尚未人工确认的 marker / 设备身份冲突
    Conflicts {
        #[arg(long)]
        json: bool,
    },
    /// 解决身份冲突；本轮只能明确保留为新卷，绝不自动合并
    Resolve(VolumeResolveArgs),
    /// 仅当稳定设备身份一致时，手工重连已有卷到新挂载路径
    Relink(VolumeRelinkArgs),
}

#[derive(Debug, Args)]
struct VolumeAddArgs {
    path: PathBuf,
    #[arg(long, default_value = "unknown")]
    role: VolumeRole,
    #[arg(long, conflicts_with = "no_write_marker")]
    write_marker: bool,
    #[arg(long, conflicts_with = "write_marker")]
    no_write_marker: bool,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct VolumeResolveArgs {
    #[arg(long)]
    conflict: i64,
    #[arg(long)]
    as_new_volume: bool,
    #[arg(long, default_value = "unknown")]
    role: VolumeRole,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct VolumeRelinkArgs {
    #[arg(long)]
    volume: i64,
    #[arg(long)]
    path: PathBuf,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct ScanCommand {
    #[command(subcommand)]
    action: Option<ScanAction>,
    /// 要扫描的卷根目录
    root: Option<PathBuf>,
    #[arg(long)]
    full_hash: bool,
    #[arg(long, conflicts_with = "full_hash")]
    metadata_only: bool,
    #[arg(long)]
    resume: bool,
    #[arg(long = "exclude")]
    excludes: Vec<String>,
    #[arg(long, default_value_t = 1)]
    max_readers: usize,
    #[arg(long, conflicts_with = "jsonl_progress")]
    json: bool,
    /// 使用稳定 JSON Lines 输出任务开始、持久化进度与结束事件
    #[arg(long, conflicts_with = "json")]
    jsonl_progress: bool,
}

#[derive(Debug, Subcommand)]
enum ScanAction {
    List {
        #[arg(long)]
        json: bool,
    },
    Show {
        scan_id: i64,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum HashCommand {
    Complete {
        #[arg(long, conflicts_with = "all")]
        volume: Option<i64>,
        #[arg(long)]
        all: bool,
        #[arg(long, conflicts_with = "jsonl_progress")]
        json: bool,
        #[arg(long, conflicts_with = "json")]
        jsonl_progress: bool,
    },
}

#[derive(Debug, Args)]
struct LookupArgs {
    path: PathBuf,
    #[arg(long)]
    full_hash: bool,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct DuplicatesArgs {
    #[arg(long)]
    volume: Option<i64>,
    #[arg(long, default_value_t = 0)]
    min_size: u64,
    #[arg(long, default_value_t = 2)]
    min_copies: usize,
    #[arg(long)]
    online_only: bool,
    #[arg(long)]
    include_missing: bool,
    #[arg(long)]
    json: bool,
    #[arg(long)]
    csv: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct VerifyArgs {
    #[arg(long, conflicts_with = "file_copy")]
    volume: Option<i64>,
    #[arg(long, conflicts_with = "volume")]
    file_copy: Option<i64>,
    #[arg(long)]
    full_hash: bool,
    #[arg(long)]
    jsonl_progress: bool,
}

#[derive(Debug, Args)]
struct TasksArgs {
    #[arg(long, default_value_t = 0)]
    after_id: i64,
    #[arg(long, default_value_t = 100)]
    limit: usize,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Subcommand)]
enum CleanupCommand {
    Plan(CleanupPlanArgs),
}

#[derive(Debug, Args)]
struct CleanupPlanArgs {
    #[arg(long)]
    target_volume: i64,
    #[arg(long)]
    keep_volume: i64,
    #[arg(long, default_value_t = 1)]
    min_remaining_copies: usize,
    #[arg(long, default_value_t = 1)]
    min_remaining_physical_devices: usize,
    #[arg(long)]
    verify_metadata: bool,
    #[arg(long)]
    verify_full_hash: bool,
    #[arg(long)]
    jsonl_progress: bool,
    #[arg(long)]
    output: PathBuf,
}

#[derive(Debug, Args)]
struct UiArgs {
    /// 本机监听端口，仅绑定 127.0.0.1
    #[arg(long, default_value_t = 48152)]
    port: u16,
    /// 不自动打开默认浏览器
    #[arg(long)]
    no_open: bool,
}

pub fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}

pub fn run(cli: Cli) -> Result<()> {
    let config = Config::new(cli.db)?;
    let mut database = Database::open(&config)?;
    match cli.command {
        Command::Init => {
            println!("数据库已初始化: {}", database.path().display());
            println!("schema 版本: {}", database.schema_version()?);
        }
        Command::Volume { command } => run_volume(&mut database, command)?,
        Command::Scan(command) => run_scan(&mut database, &config, command)?,
        Command::Hash { command } => run_hash(&mut database, &config, command)?,
        Command::Lookup(args) => {
            database.refresh_volume_online_states()?;
            let result = lookup(&database, &config, &args.path, args.full_hash)?;
            if args.json {
                print_json(&result)?;
            } else {
                print!("{}", render_lookup_text(&result));
            }
        }
        Command::Duplicates(args) => {
            database.refresh_volume_online_states()?;
            let groups = duplicate_groups(
                &database,
                DuplicateFilter {
                    min_size: args.min_size,
                    min_copies: args.min_copies,
                    online_only: args.online_only,
                    include_missing: args.include_missing,
                    volume_id: args.volume,
                },
            )?;
            if let Some(path) = args.csv {
                write_csv(&path, &groups)?;
                eprintln!("CSV 报告已写入: {}", path.display());
            }
            if args.json {
                print_json(&duplicate_report(&database, &groups))?;
            } else {
                print!("{}", render_duplicates_text(&groups));
            }
        }
        Command::Verify(args) => run_verify(&mut database, &config, args)?,
        Command::Tasks(args) => {
            let tasks = database.task_runs_page(args.after_id, args.limit)?;
            if args.json {
                print_json(&tasks)?;
            } else {
                for task in tasks {
                    println!(
                        "{}\t{}\t{}\t{}",
                        task["id"], task["operation"], task["status"], task["started_at"]
                    );
                }
            }
        }
        Command::Cleanup { command } => match command {
            CleanupCommand::Plan(args) => {
                database.refresh_volume_online_states()?;
                let task = if args.jsonl_progress {
                    Some(JsonlTask::start(&mut database, "cleanup_plan")?)
                } else {
                    None
                };
                let plan_result = create_cleanup_plan_with_verification(
                    &mut database,
                    &config,
                    args.target_volume,
                    args.keep_volume,
                    args.min_remaining_copies,
                    CleanupVerificationOptions {
                        min_remaining_physical_devices: args.min_remaining_physical_devices,
                        verify_metadata: args.verify_metadata || args.verify_full_hash,
                        verify_full_hash: args.verify_full_hash,
                    },
                );
                let plan = match plan_result {
                    Ok(plan) => plan,
                    Err(error) => {
                        if let Some(task) = &task {
                            task.fail(&mut database, &error)?;
                        }
                        return Err(error);
                    }
                };
                if let Err(error) = write_cleanup_plan(&args.output, &plan) {
                    if let Some(task) = &task {
                        task.fail(&mut database, &error)?;
                    }
                    return Err(error);
                }
                if let Some(task) = &task {
                    task.complete(
                        &mut database,
                        "completed",
                        &serde_json::json!({"plan": plan, "output": args.output}),
                    )?;
                } else {
                    print!("{}", render_cleanup_plan_text(&plan));
                    println!("计划文件: {}", args.output.display());
                }
            }
        },
        Command::Ui(args) => run_local_ui(&config, args.port, !args.no_open)?,
    }
    Ok(())
}

fn run_volume(database: &mut Database, command: VolumeCommand) -> Result<()> {
    match command {
        VolumeCommand::Add(args) => {
            let policy = if args.no_write_marker {
                MarkerPolicy::DoNotWrite
            } else {
                MarkerPolicy::WriteIfPossible
            };
            let registration = register_volume(database, &args.path, args.role, policy)?;
            if args.json {
                print_json(&serde_json::json!({
                    "status": registration.identity_state.as_str(),
                    "volume": registration.volume.as_ref().map(volume_json),
                    "marker_uid": registration.marker_uid,
                    "writable": registration.writable,
                    "used_fallback_identity": registration.used_fallback_identity,
                    "identity_conflict": registration.conflict.as_ref().map(volume_conflict_json),
                }))?;
            } else if let Some(conflict) = registration.conflict {
                println!("卷身份状态: possible_clone");
                println!("冲突 ID: {}", conflict.id);
                println!("历史卷 ID: {}", conflict.existing_volume_id);
                println!("候选路径: {}", conflict.candidate_mount_path.display());
                println!("未写入或覆盖任何历史卷；请先运行 `disk-indexer volume conflicts` 审核。");
            } else {
                let volume = registration
                    .volume
                    .context("卷注册未产生可安全使用的卷记录")?;
                println!("卷 ID: {}", volume.id);
                println!("卷名称: {}", volume.volume_name);
                println!("挂载路径: {}", volume.mount_path.display());
                println!(
                    "系统卷 UUID: {}",
                    volume.system_volume_uuid.as_deref().unwrap_or("不可用")
                );
                println!(
                    "设备标识: {}",
                    volume.filesystem.as_deref().unwrap_or("不可用")
                );
                println!(
                    "marker UID: {}",
                    registration
                        .marker_uid
                        .as_deref()
                        .unwrap_or("未写入，使用保守回退身份")
                );
                println!("最终 volume UID: {}", volume.volume_uid);
                println!("可写: {}", registration.writable);
                println!("身份冲突: 否");
            }
        }
        VolumeCommand::List { json } => {
            database.refresh_volume_online_states()?;
            let volumes = database.volumes()?;
            if json {
                let values = volumes.iter().map(volume_json).collect::<Vec<_>>();
                print_json(&values)?;
            } else if volumes.is_empty() {
                println!("尚未注册卷。");
            } else {
                for volume in volumes {
                    println!(
                        "{}\t{}\t{}\t{}\t{}",
                        volume.id,
                        if volume.is_online {
                            "online"
                        } else {
                            "offline"
                        },
                        volume.role.as_str(),
                        volume.volume_name,
                        volume.mount_path.display()
                    );
                }
            }
        }
        VolumeCommand::Show { volume_id, json } => {
            database.refresh_volume_online_states()?;
            let volume = database.volume_by_id(volume_id)?;
            if json {
                print_json(&volume_json(&volume))?;
            } else {
                println!("卷 ID: {}", volume.id);
                println!("UID: {}", volume.volume_uid);
                println!("名称: {}", volume.volume_name);
                println!("挂载路径: {}", volume.mount_path.display());
                println!("角色: {}", volume.role.as_str());
                println!(
                    "状态: {}",
                    if volume.is_online {
                        "online"
                    } else {
                        "offline"
                    }
                );
                println!("首次发现: {}", volume.first_seen_at);
                println!("最后发现: {}", volume.last_seen_at);
            }
        }
        VolumeCommand::Conflicts { json } => {
            let conflicts = database.open_volume_conflicts()?;
            if json {
                let values = conflicts
                    .iter()
                    .map(volume_conflict_json)
                    .collect::<Vec<_>>();
                print_json(&values)?;
            } else if conflicts.is_empty() {
                println!("没有待处理的卷身份冲突。");
            } else {
                for conflict in conflicts {
                    println!(
                        "{}\topen\t历史卷 {}\t候选路径 {}\t检测于 {}",
                        conflict.id,
                        conflict.existing_volume_id,
                        conflict.candidate_mount_path.display(),
                        conflict.detected_at
                    );
                }
            }
        }
        VolumeCommand::Resolve(args) => {
            if !args.as_new_volume {
                bail!("为避免误合并，volume resolve 当前必须显式指定 --as-new-volume");
            }
            let volume = resolve_conflict_as_new_volume(database, args.conflict, args.role)?;
            if args.json {
                print_json(&serde_json::json!({
                    "status": "resolved_as_new_volume",
                    "volume": volume_json(&volume),
                    "conflict_id": args.conflict,
                }))?;
            } else {
                println!("已将冲突 {} 保留为独立卷 ID {}。", args.conflict, volume.id);
            }
        }
        VolumeCommand::Relink(args) => {
            let volume = relink_volume(database, args.volume, &args.path)?;
            if args.json {
                print_json(&volume_json(&volume))?;
            } else {
                println!(
                    "卷 {} 已重连到 {}。",
                    volume.id,
                    volume.mount_path.display()
                );
            }
        }
    }
    Ok(())
}

fn run_scan(database: &mut Database, config: &Config, command: ScanCommand) -> Result<()> {
    if let Some(action) = command.action {
        if command.root.is_some() {
            bail!("scan 历史子命令不能同时指定扫描路径");
        }
        if command.jsonl_progress {
            bail!("scan 历史查询不支持 --jsonl-progress");
        }
        return match action {
            ScanAction::List { json } => {
                let runs = database.scan_runs()?;
                if json {
                    print_json(&runs)?;
                } else {
                    for run in runs {
                        println!(
                            "{}\t{}\t{}\t{}\t{} files\t{}",
                            run["id"],
                            run["volume"],
                            run["status"],
                            run["started_at"],
                            run["discovered_count"],
                            run["mode"]
                        );
                    }
                }
                Ok(())
            }
            ScanAction::Show { scan_id, json } => {
                let run = database.scan_run(scan_id)?.context("未找到扫描记录")?;
                if json {
                    print_json(&run)?;
                } else {
                    println!("{run}");
                }
                Ok(())
            }
        };
    }
    let root = command
        .root
        .context("请提供卷根目录，例如 disk-indexer scan /Volumes/Photos")?;
    let canonical_root = root
        .canonicalize()
        .with_context(|| format!("无法访问扫描根目录 {}", root.display()))?;
    let existing_role = database
        .find_volume_for_path(&canonical_root)?
        .filter(|volume| volume.mount_path == canonical_root)
        .map(|volume| volume.role)
        .unwrap_or(VolumeRole::Unknown);
    let registration = register_volume(
        database,
        &canonical_root,
        existing_role,
        MarkerPolicy::WriteIfPossible,
    )?;
    let volume = registration.volume.context(
        "扫描已停止：检测到 possible_clone；请先使用 volume conflicts 审核并 resolve 或 relink",
    )?;
    let task = if command.jsonl_progress {
        Some(Arc::new(JsonlTask::start(database, "scan")?))
    } else {
        None
    };
    let progress_callback = task.as_ref().map(|task| {
        let task = Arc::clone(task);
        Arc::new(
            move |summary: &crate::model::ScanSummary, current_path: Option<&str>| {
                task.progress(serde_json::json!({
                    "files_seen": summary.discovered_count,
                    "files_reused": summary.metadata_reused_count,
                    "files_sampled": summary.sampled_count,
                    "files_full_hashed": summary.full_hashed_count,
                    "bytes_read": summary.bytes_read,
                    "current_path": current_path,
                }));
            },
        ) as crate::scanner::ScanProgressCallback
    });
    let summary_result = scan(
        database,
        config,
        &volume,
        &ScanOptions {
            full_hash: command.full_hash,
            metadata_only: command.metadata_only,
            resume: command.resume,
            excludes: command.excludes,
            max_readers: command.max_readers,
            cancel_flag: None,
            progress_callback,
        },
    );
    let summary = match summary_result {
        Ok(summary) => summary,
        Err(error) => {
            if let Some(task) = &task {
                task.fail(database, &error)?;
            }
            return Err(error);
        }
    };
    if let Some(task) = &task {
        task.complete(database, &summary.status, &summary)?;
    } else if command.json {
        print_json(&summary)?;
    } else {
        println!(
            "扫描 {}：{}，发现 {}，元数据复用 {}，抽样 {}，完整哈希 {}，缺失 {}，错误 {}，读取 {} 字节",
            summary.scan_id,
            summary.status,
            summary.discovered_count,
            summary.metadata_reused_count,
            summary.sampled_count,
            summary.full_hashed_count,
            summary.missing_count,
            summary.error_count,
            summary.bytes_read
        );
    }
    Ok(())
}

fn run_hash(database: &mut Database, config: &Config, command: HashCommand) -> Result<()> {
    match command {
        HashCommand::Complete {
            volume,
            all,
            json,
            jsonl_progress,
        } => {
            if !all && volume.is_none() {
                bail!("hash complete 必须指定 --volume <id> 或 --all");
            }
            database.refresh_volume_online_states()?;
            let task = if jsonl_progress {
                Some(JsonlTask::start(database, "hash_complete")?)
            } else {
                None
            };
            let cancel_flag = if task.is_some() {
                let flag = interrupt_flag()?;
                flag.store(false, std::sync::atomic::Ordering::SeqCst);
                Some(flag)
            } else {
                None
            };
            let stats = match if let Some(cancel_flag) = cancel_flag.as_deref() {
                complete_hashes_with_cancel(database, config, volume, Some(cancel_flag))
            } else {
                complete_hashes(database, config, volume)
            } {
                Ok(stats) => stats,
                Err(error) => {
                    if let Some(task) = &task {
                        task.fail(database, &error)?;
                    }
                    return Err(error);
                }
            };
            let summary = serde_json::json!({
                "sampled": stats.sampled,
                "full_hashed": stats.full_hashed,
                "errors": stats.errors,
                "bytes_read": stats.bytes_read,
                "interrupted": stats.interrupted,
            });
            if let Some(task) = &task {
                task.progress(summary.clone());
                task.complete(
                    database,
                    if stats.interrupted {
                        "interrupted"
                    } else {
                        "completed"
                    },
                    &summary,
                )?;
            } else if json {
                print_json(&summary)?;
            } else {
                println!(
                    "完整哈希完成：{} 个文件，读取 {} 字节，错误 {}",
                    stats.full_hashed, stats.bytes_read, stats.errors
                );
            }
        }
    }
    Ok(())
}

fn run_verify(database: &mut Database, config: &Config, args: VerifyArgs) -> Result<()> {
    if args.file_copy.is_none() && args.volume.is_none() {
        bail!("verify 必须指定 --volume <id> 或 --file-copy <id>");
    }
    let task = if args.jsonl_progress {
        Some(JsonlTask::start(database, "verify")?)
    } else {
        None
    };
    let cancel_flag = if task.is_some() {
        let flag = interrupt_flag()?;
        flag.store(false, std::sync::atomic::Ordering::SeqCst);
        Some(flag)
    } else {
        None
    };
    let mut failures = 0usize;
    let mut verified = 0usize;
    let mut interrupted = false;
    if let Some(id) = args.file_copy {
        let record = database
            .file_record_by_id(id)?
            .context("未找到文件副本记录")?;
        verify_one(
            database,
            config,
            &record,
            args.full_hash,
            task.as_ref(),
            &mut verified,
            &mut failures,
        );
    } else if let Some(volume_id) = args.volume {
        let mut after_id = 0;
        loop {
            if cancel_flag
                .as_ref()
                .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::SeqCst))
            {
                interrupted = true;
                break;
            }
            let records =
                database.files_for_volume_page(volume_id, after_id, config.query_page_size)?;
            let Some(last) = records.last() else {
                break;
            };
            after_id = last.id;
            for record in records {
                if cancel_flag
                    .as_ref()
                    .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::SeqCst))
                {
                    interrupted = true;
                    break;
                }
                verify_one(
                    database,
                    config,
                    &record,
                    args.full_hash,
                    task.as_ref(),
                    &mut verified,
                    &mut failures,
                );
            }
            if interrupted {
                break;
            }
        }
    }
    let summary =
        serde_json::json!({"verified": verified, "failures": failures, "interrupted": interrupted});
    if let Some(task) = &task {
        task.complete(
            database,
            if interrupted {
                "interrupted"
            } else if failures == 0 {
                "completed"
            } else {
                "completed_with_errors"
            },
            &summary,
        )?;
    }
    if failures > 0 {
        bail!("验证失败：{failures} 个文件副本未通过");
    }
    Ok(())
}

fn verify_one(
    database: &mut Database,
    config: &Config,
    record: &crate::model::FileRecord,
    full_hash: bool,
    task: Option<&JsonlTask>,
    verified: &mut usize,
    failures: &mut usize,
) {
    match verify_record(database, config, record, full_hash) {
        Ok(()) => {
            *verified = verified.saturating_add(1);
            if task.is_none() {
                println!(
                    "通过: {}:{}",
                    record.volume_name,
                    record.relative_path.display()
                );
            }
        }
        Err(error) => {
            *failures = failures.saturating_add(1);
            if task.is_none() {
                eprintln!(
                    "失败: {}:{}: {error:#}",
                    record.volume_name,
                    record.relative_path.display()
                );
            }
        }
    }
    if let Some(task) = task {
        task.progress(serde_json::json!({"verified": *verified, "failures": *failures}));
    }
}

fn volume_json(volume: &crate::model::Volume) -> serde_json::Value {
    serde_json::json!({
        "id": volume.id,
        "volume_uid": volume.volume_uid,
        "marker_uid": volume.marker_uid,
        "volume_name": volume.volume_name,
        "filesystem": volume.filesystem,
        "mount_path": volume.mount_path.to_string_lossy(),
        "system_volume_uuid": volume.system_volume_uuid,
        "device_serial": volume.device_serial,
        "partition_uuid": volume.partition_uuid,
        "total_size": volume.total_size,
        "physical_device_id": volume.physical_device_id,
        "identity_state": volume.identity_state.as_str(),
        "role": volume.role.as_str(),
        "is_online": volume.is_online,
        "first_seen_at": volume.first_seen_at,
        "last_seen_at": volume.last_seen_at,
    })
}

fn volume_conflict_json(conflict: &crate::model::VolumeIdentityConflict) -> serde_json::Value {
    serde_json::json!({
        "id": conflict.id,
        "state": conflict.state,
        "existing_volume_id": conflict.existing_volume_id,
        "existing_volume_uid": conflict.existing_volume_uid,
        "candidate_marker_uid": conflict.candidate_marker_uid,
        "candidate_mount_path": conflict.candidate_mount_path.to_string_lossy(),
        "candidate_filesystem": conflict.candidate_filesystem,
        "candidate_system_volume_uuid": conflict.candidate_system_volume_uuid,
        "candidate_partition_uuid": conflict.candidate_partition_uuid,
        "candidate_media_uuid": conflict.candidate_media_uuid,
        "candidate_device_serial": conflict.candidate_device_serial,
        "candidate_total_size": conflict.candidate_total_size,
        "candidate_physical_device_id": conflict.candidate_physical_device_id,
        "resolution": conflict.resolution,
        "resolved_volume_id": conflict.resolved_volume_id,
        "detected_at": conflict.detected_at,
        "resolved_at": conflict.resolved_at,
    })
}

fn print_json<T: serde::Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
