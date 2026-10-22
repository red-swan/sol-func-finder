use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result};
use xxhash_rust::xxh3::Xxh3;

use crate::cache::FORMAT_VERSION;

/// Hash of every artifact's (relative path, mtime, size), the canonical out
/// dir, the tool version and the cache format version.
pub fn compute(out_dir: &Path, files: &[PathBuf]) -> Result<u64> {
    let mut h = Xxh3::new();
    let canon = out_dir
        .canonicalize()
        .with_context(|| format!("resolving {}", out_dir.display()))?;
    h.update(canon.to_string_lossy().as_bytes());
    h.update(env!("CARGO_PKG_VERSION").as_bytes());
    h.update(&FORMAT_VERSION.to_le_bytes());
    for file in files {
        let meta = std::fs::metadata(file).with_context(|| format!("stat {}", file.display()))?;
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos());
        let rel = file.strip_prefix(out_dir).unwrap_or(file);
        h.update(rel.to_string_lossy().as_bytes());
        h.update(&[0]);
        h.update(&mtime.to_le_bytes());
        h.update(&meta.len().to_le_bytes());
    }
    Ok(h.digest())
}
