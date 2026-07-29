//! 简单的本地哈希吞吐检查工具；不修改索引数据库。

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use clap::Parser;
use disk_indexer::hashing::full_hash;

#[derive(Debug, Parser)]
#[command(name = "disk-indexer-benchmark", about = "测量单文件流式 BLAKE3 吞吐")]
struct Args {
    path: PathBuf,
    #[arg(long, default_value_t = 1024 * 1024)]
    buffer_bytes: usize,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let started = Instant::now();
    let result = full_hash(&args.path, args.buffer_bytes)?;
    let elapsed = started.elapsed();
    let mib_per_second = if elapsed.is_zero() {
        0.0
    } else {
        result.bytes_read as f64 / (1024.0 * 1024.0) / elapsed.as_secs_f64()
    };
    println!(
        "{} bytes in {:.3}s ({mib_per_second:.1} MiB/s), hash {}",
        result.bytes_read,
        elapsed.as_secs_f64(),
        result.hex
    );
    std::fs::metadata(&args.path)
        .with_context(|| format!("基准完成后无法读取 {}", args.path.display()))?;
    Ok(())
}
