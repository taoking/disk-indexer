use std::env;
use std::path::PathBuf;

use anyhow::{Context, Result};
use directories::ProjectDirs;

/// 运行时配置。CLI 参数优先于 `DISK_INDEXER_DB` 环境变量。
#[derive(Debug, Clone)]
pub struct Config {
    pub database_path: PathBuf,
    pub sample_bytes: usize,
    pub read_buffer_bytes: usize,
    /// 单次写入数据库的扫描元数据数量。
    pub batch_size: usize,
    /// 所有大查询使用的 keyset 分页大小。
    pub query_page_size: usize,
}

impl Config {
    pub fn new(database_override: Option<PathBuf>) -> Result<Self> {
        let database_path = database_override
            .or_else(|| env::var_os("DISK_INDEXER_DB").map(PathBuf::from))
            .unwrap_or(default_database_path()?);
        Ok(Self {
            database_path,
            sample_bytes: 256 * 1024,
            read_buffer_bytes: 1024 * 1024,
            batch_size: 500,
            query_page_size: 1_000,
        })
    }
}

pub fn default_database_path() -> Result<PathBuf> {
    let project = ProjectDirs::from("org", "DiskIndexer", "DiskIndexer")
        .context("无法确定当前平台的应用数据目录")?;
    Ok(project.data_dir().join("index.db"))
}
