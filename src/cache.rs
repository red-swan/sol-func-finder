use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::graph::Graph;

/// Bump whenever `Graph` or the resolution logic changes shape.
pub const FORMAT_VERSION: u32 = 5;
const MAGIC: [u8; 8] = *b"SOLFFGR\0";

#[derive(Serialize, Deserialize)]
struct Header {
    magic: [u8; 8],
    format: u32,
    fingerprint: u64,
    /// Source files the build saw and whether each existed on disk.
    sources: Vec<(String, bool)>,
}

fn dir() -> PathBuf {
    PathBuf::from(".solff")
}

fn file() -> PathBuf {
    dir().join("graph.bin")
}

/// The cached graph, if present, undamaged, and still valid for `fingerprint`.
pub fn load(fingerprint: u64) -> Option<Graph> {
    let bytes = std::fs::read(file()).ok()?;
    let (header, rest) = postcard::take_from_bytes::<Header>(&bytes).ok()?;
    if header.magic != MAGIC || header.format != FORMAT_VERSION || header.fingerprint != fingerprint
    {
        return None;
    }
    // A source deleted (or restored) without touching any artifact changes the result.
    if header
        .sources
        .iter()
        .any(|(p, existed)| std::path::Path::new(p).exists() != *existed)
    {
        return None;
    }
    postcard::from_bytes::<Graph>(rest).ok()
}

pub fn store(fingerprint: u64, sources: &[(String, bool)], graph: &Graph) -> Result<()> {
    let header = Header {
        magic: MAGIC,
        format: FORMAT_VERSION,
        fingerprint,
        sources: sources.to_vec(),
    };
    let mut bytes = postcard::to_stdvec(&header).context("encoding cache header")?;
    bytes.extend(postcard::to_stdvec(graph).context("encoding graph")?);

    std::fs::create_dir_all(dir()).context("creating .solff")?;
    let ignore = dir().join(".gitignore");
    if !ignore.exists() {
        std::fs::write(&ignore, "*\n").context("writing .solff/.gitignore")?;
    }
    let tmp = dir().join(format!("graph.bin.tmp.{}", std::process::id()));
    std::fs::write(&tmp, &bytes).context("writing cache")?;
    std::fs::rename(&tmp, file()).context("replacing cache")?;
    Ok(())
}
