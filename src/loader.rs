use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value;
use serde_json::value::RawValue;
use walkdir::WalkDir;

pub struct Source {
    pub path: String,
    pub ast: Value,
}

pub struct Loaded {
    pub sources: Vec<Source>,
    /// Source paths whose ASTs were found, with whether the file exists on disk.
    pub seen: Vec<(String, bool)>,
    pub artifacts_with_ast: usize,
    pub artifacts_total: usize,
}

#[derive(Deserialize)]
struct Artifact {
    ast: Option<Value>,
}

/// Source files excluded from the graph entirely.
pub fn is_excluded(path: &str) -> bool {
    path.starts_with("test/")
        || path.starts_with("script/")
        || path.ends_with(".t.sol")
        || path.ends_with(".s.sol")
}

/// Every artifact JSON under `out_dir`, sorted, skipping `build-info/`.
pub fn artifact_files(out_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let walker = WalkDir::new(out_dir)
        .min_depth(1)
        .into_iter()
        .filter_entry(|e| !(e.file_type().is_dir() && e.file_name() == "build-info"));
    for entry in walker {
        let entry = entry.with_context(|| format!("walking {}", out_dir.display()))?;
        if entry.file_type().is_file() && entry.path().extension().is_some_and(|x| x == "json") {
            files.push(entry.into_path());
        }
    }
    files.sort();
    Ok(files)
}

/// Cheap view of an artifact: only where its AST lives, without building a tree.
#[derive(Deserialize)]
struct Probe<'a> {
    #[serde(borrow)]
    ast: Option<&'a RawValue>,
}

enum Probed {
    /// Unreadable, unparsable or no `ast` field.
    NoAst,
    Ast(String),
}

fn ast_path(raw: &str) -> Option<String> {
    // solc emits `absolutePath` first; fall back to a full parse if it ever does not.
    if let Some(rest) = raw.strip_prefix("{\"absolutePath\":\"")
        && let Some(end) = rest.find('"')
    {
        return Some(rest[..end].to_string());
    }
    let v: Value = serde_json::from_str(raw).ok()?;
    v.get("absolutePath")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn probe(file: &Path) -> Probed {
    let Ok(text) = std::fs::read_to_string(file) else {
        return Probed::NoAst;
    };
    match serde_json::from_str::<Probe>(&text) {
        Ok(Probe { ast: Some(raw) }) => ast_path(raw.get()).map_or(Probed::NoAst, Probed::Ast),
        _ => Probed::NoAst,
    }
}

fn parse_ast(file: &Path) -> Result<Value> {
    let bytes = std::fs::read(file).with_context(|| format!("reading {}", file.display()))?;
    let artifact: Artifact =
        serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", file.display()))?;
    artifact
        .ast
        .with_context(|| format!("{} has no ast", file.display()))
}

/// `f` over `items` on all cores, results in input order.
fn par_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let next = AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let mut all: Vec<(usize, R)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut local = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(item) = items.get(i) else { break };
                        local.push((i, f(item)));
                    }
                    local
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("worker panicked"))
            .collect()
    });
    all.sort_by_key(|(i, _)| *i);
    all.into_iter().map(|(_, r)| r).collect()
}

pub fn load(files: &[PathBuf]) -> Result<Loaded> {
    let mut loaded = Loaded {
        sources: Vec::new(),
        seen: Vec::new(),
        artifacts_with_ast: 0,
        artifacts_total: files.len(),
    };

    // Every contract in a file carries the file's whole AST: find each source's
    // first artifact cheaply, then fully parse only those.
    let mut done: HashSet<String> = HashSet::new();
    let mut unique: Vec<(&PathBuf, String)> = Vec::new();
    for (file, probed) in files.iter().zip(par_map(files, |f| probe(f))) {
        let Probed::Ast(path) = probed else {
            continue;
        };
        loaded.artifacts_with_ast += 1;
        if !done.insert(path.clone()) || is_excluded(&path) {
            continue;
        }
        let exists = Path::new(&path).exists();
        loaded.seen.push((path.clone(), exists));
        if !exists {
            continue;
        }
        unique.push((file, path));
    }

    for (parsed, (_, path)) in par_map(&unique, |(file, _)| parse_ast(file))
        .into_iter()
        .zip(&unique)
    {
        loaded.sources.push(Source {
            path: path.clone(),
            ast: parsed?,
        });
    }
    loaded.sources.sort_by(|a, b| a.path.cmp(&b.path));
    loaded.seen.sort();
    Ok(loaded)
}
