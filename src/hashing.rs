//! 流式 BLAKE3 与版本化抽样指纹。

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use anyhow::{Context, Result};

pub const FULL_HASH_ALGORITHM: &str = "blake3";
pub const SAMPLE_HASH_ALGORITHM: &str = "blake3-sample-v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HashResult {
    pub hex: String,
    pub bytes_read: u64,
}

/// 计算完整内容 BLAKE3；文件始终以固定缓冲区流式读取。
pub fn full_hash(path: &Path, buffer_bytes: usize) -> Result<HashResult> {
    let file = File::open(path)
        .with_context(|| format!("无法打开文件以计算完整哈希: {}", path.display()))?;
    let mut reader = BufReader::with_capacity(buffer_bytes.max(4096), file);
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0_u8; buffer_bytes.max(4096)];
    let mut bytes_read = 0_u64;
    loop {
        let read = reader
            .read(&mut buffer)
            .with_context(|| format!("读取文件失败: {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        bytes_read = bytes_read.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
    }
    Ok(HashResult {
        hex: hasher.finalize().to_hex().to_string(),
        bytes_read,
    })
}

/// 返回抽样区间 `(offset, length)`。小文件只有一个完整区间。
#[must_use]
pub fn sample_ranges(size: u64, sample_bytes: usize) -> Vec<(u64, usize)> {
    let amount = u64::try_from(sample_bytes).unwrap_or(u64::MAX);
    if size <= amount.saturating_mul(3) {
        return vec![(0, usize::try_from(size).unwrap_or(sample_bytes))];
    }
    let middle = size.saturating_sub(amount) / 2;
    vec![
        (0, sample_bytes),
        (middle, sample_bytes),
        (size.saturating_sub(amount), sample_bytes),
    ]
}

/// 计算仅用于候选筛选的抽样指纹，绝不作为内容相等结论。
pub fn sample_hash(path: &Path, sample_bytes: usize, buffer_bytes: usize) -> Result<HashResult> {
    let file = File::open(path)
        .with_context(|| format!("无法打开文件以计算抽样哈希: {}", path.display()))?;
    let size = file
        .metadata()
        .with_context(|| format!("无法读取文件大小: {}", path.display()))?
        .len();
    let mut reader = BufReader::with_capacity(buffer_bytes.max(4096), file);
    let mut hasher = blake3::Hasher::new();
    hasher.update(SAMPLE_HASH_ALGORITHM.as_bytes());
    hasher.update(&size.to_le_bytes());
    let mut bytes_read = 0_u64;
    let mut buffer = vec![0_u8; buffer_bytes.max(4096).min(sample_bytes.max(4096))];
    for (offset, length) in sample_ranges(size, sample_bytes) {
        hasher.update(&offset.to_le_bytes());
        hasher.update(&u64::try_from(length).unwrap_or(u64::MAX).to_le_bytes());
        reader
            .seek(SeekFrom::Start(offset))
            .with_context(|| format!("无法定位抽样区间（文件可能已变化）: {}", path.display()))?;
        let mut remaining = length;
        while remaining > 0 {
            let take = remaining.min(buffer.len());
            let read = reader
                .read(&mut buffer[..take])
                .with_context(|| format!("读取抽样区间失败: {}", path.display()))?;
            if read == 0 {
                anyhow::bail!("抽样区间提前结束（文件可能已变化）: {}", path.display());
            }
            hasher.update(&buffer[..read]);
            remaining -= read;
            bytes_read = bytes_read.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
        }
    }
    Ok(HashResult {
        hex: hasher.finalize().to_hex().to_string(),
        bytes_read,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{FULL_HASH_ALGORITHM, full_hash, sample_hash, sample_ranges};

    #[test]
    fn empty_file_has_normal_blake3_hash() {
        let dir = tempdir().expect("temp dir");
        let path = dir.path().join("empty");
        fs::write(&path, []).expect("write empty");
        let result = full_hash(&path, 4096).expect("hash");
        assert_eq!(FULL_HASH_ALGORITHM, "blake3");
        assert_eq!(result.hex, blake3::hash(&[]).to_hex().to_string());
    }

    #[test]
    fn small_file_uses_one_complete_range() {
        assert_eq!(sample_ranges(10, 8), vec![(0, 10)]);
    }

    #[test]
    fn large_file_uses_head_middle_tail() {
        assert_eq!(sample_ranges(100, 10), vec![(0, 10), (45, 10), (90, 10)]);
    }

    #[test]
    fn sample_is_deterministic() {
        let dir = tempdir().expect("temp dir");
        let path = dir.path().join("data");
        fs::write(&path, vec![42_u8; 1000]).expect("write data");
        assert_eq!(
            sample_hash(&path, 100, 1024).expect("first"),
            sample_hash(&path, 100, 1024).expect("second")
        );
    }
}
