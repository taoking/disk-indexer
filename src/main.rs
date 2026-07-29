use anyhow::Result;
use clap::Parser;

fn main() -> Result<()> {
    disk_indexer::cli::init_logging();
    disk_indexer::cli::run(disk_indexer::cli::Cli::parse())
}
