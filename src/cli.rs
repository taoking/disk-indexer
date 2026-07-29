//! CLI 适配层：只处理参数、输出与退出状态，不承载索引业务逻辑。

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use crate::config::Config;
use crate::db::Database;
use crate::duplicate::{DuplicateFilter, duplicate_groups, lookup, verify_record};
use crate::model::VolumeRole;
use crate::report::{
    create_cleanup_plan, duplicate_report, render_cleanup_plan_text, render_duplicates_text,
    render_lookup_text, write_cleanup_plan, write_csv,
};
use crate::scanner::{ScanOptions, complete_hashes, scan};
use crate::ui::run_local_ui;
use crate::volume::{MarkerPolicy, register_volume};

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
    #[arg(long)]
    json: bool,
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
        #[arg(long)]
        json: bool,
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
        Command::Cleanup { command } => match command {
            CleanupCommand::Plan(args) => {
                database.refresh_volume_online_states()?;
                let plan = create_cleanup_plan(
                    &database,
                    args.target_volume,
                    args.keep_volume,
                    args.min_remaining_copies,
                )?;
                write_cleanup_plan(&args.output, &plan)?;
                print!("{}", render_cleanup_plan_text(&plan));
                println!("计划文件: {}", args.output.display());
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
                    "volume": volume_json(&registration.volume),
                    "marker_uid": registration.marker_uid,
                    "writable": registration.writable,
                    "used_fallback_identity": registration.used_fallback_identity,
                    "identity_conflict": false,
                }))?;
            } else {
                println!("卷 ID: {}", registration.volume.id);
                println!("卷名称: {}", registration.volume.volume_name);
                println!("挂载路径: {}", registration.volume.mount_path.display());
                println!(
                    "系统卷 UUID: {}",
                    registration
                        .volume
                        .system_volume_uuid
                        .as_deref()
                        .unwrap_or("不可用")
                );
                println!(
                    "设备标识: {}",
                    registration
                        .volume
                        .filesystem
                        .as_deref()
                        .unwrap_or("不可用")
                );
                println!(
                    "marker UID: {}",
                    registration
                        .marker_uid
                        .as_deref()
                        .unwrap_or("未写入，使用保守回退身份")
                );
                println!("最终 volume UID: {}", registration.volume.volume_uid);
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
    }
    Ok(())
}

fn run_scan(database: &mut Database, config: &Config, command: ScanCommand) -> Result<()> {
    if let Some(action) = command.action {
        if command.root.is_some() {
            bail!("scan 历史子命令不能同时指定扫描路径");
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
    let summary = scan(
        database,
        config,
        &registration.volume,
        &ScanOptions {
            full_hash: command.full_hash,
            metadata_only: command.metadata_only,
            resume: command.resume,
            excludes: command.excludes,
            max_readers: command.max_readers,
            cancel_flag: None,
        },
    )?;
    if command.json {
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
        HashCommand::Complete { volume, all, json } => {
            if !all && volume.is_none() {
                bail!("hash complete 必须指定 --volume <id> 或 --all");
            }
            database.refresh_volume_online_states()?;
            let stats = complete_hashes(database, config, volume)?;
            if json {
                print_json(&serde_json::json!({
                    "sampled": stats.sampled,
                    "full_hashed": stats.full_hashed,
                    "errors": stats.errors,
                    "bytes_read": stats.bytes_read,
                }))?;
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
    let records = if let Some(id) = args.file_copy {
        vec![
            database
                .file_record_by_id(id)?
                .context("未找到文件副本记录")?,
        ]
    } else if let Some(volume_id) = args.volume {
        database.files_for_volume(volume_id)?
    } else {
        bail!("verify 必须指定 --volume <id> 或 --file-copy <id>");
    };
    let mut failures = 0usize;
    for record in records {
        match verify_record(database, config, &record, args.full_hash) {
            Ok(()) => println!(
                "通过: {}:{}",
                record.volume_name,
                record.relative_path.display()
            ),
            Err(error) => {
                failures = failures.saturating_add(1);
                eprintln!(
                    "失败: {}:{}: {error:#}",
                    record.volume_name,
                    record.relative_path.display()
                );
            }
        }
    }
    if failures > 0 {
        bail!("验证失败：{failures} 个文件副本未通过");
    }
    Ok(())
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
        "role": volume.role.as_str(),
        "is_online": volume.is_online,
        "first_seen_at": volume.first_seen_at,
        "last_seen_at": volume.last_seen_at,
    })
}

fn print_json<T: serde::Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
