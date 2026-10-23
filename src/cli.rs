use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{bail, Result};
use clap::Parser;

use crate::graph::Graph;
use crate::{cache, fingerprint, index, loader, query, render, resolve};

/// Print every call chain from an entry point down to the given function(s),
/// using the AST in Foundry's build artifacts. Run from the project root.
///
/// Prints the function, then indented below it everything that calls it, and
/// so on up to the entry points, which are marked `<-`.
///
/// By default only chains of certain calls are shown. Calls through an
/// interface or base type are guesses (every in-scope contract that could be
/// the callee); a `~` line calls the line above it through such a guess, and
/// `--soft` shows the chains that contain one.
///
/// Limits: .call/.delegatecall/.staticcall, function pointers and proxies are
/// not resolved, so chains through them are missing. Operator overloads and
/// state-variable initializers are not followed. test/ and script/ files
/// (and *.t.sol, *.s.sol) are excluded.
#[derive(Parser)]
#[command(name = "solff", version)]
struct Args {
    /// Function(s): `name`, `Contract.name`, `name(type,...)` or `0xselector`.
    #[arg(required_unless_present = "dump_graph")]
    names: Vec<String>,

    /// Foundry artifacts directory.
    #[arg(long, default_value = "out")]
    out_dir: PathBuf,

    /// Neither read nor write the .solff cache.
    #[arg(long)]
    no_cache: bool,

    /// Show the other kind of chain: only those containing at least one `~`
    /// (guessed) call. The default shows only chains of certain calls.
    #[arg(long)]
    soft: bool,

    /// Treat the arguments as contract names and list every function each
    /// has (declared and inherited), instead of searching for callers.
    #[arg(long)]
    list: bool,

    #[arg(long, hide = true)]
    dump_graph: bool,
}

pub fn run() -> ExitCode {
    match try_run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn build(out_dir: &Path, files: &[PathBuf]) -> Result<(Graph, Vec<(String, bool)>)> {
    let loaded = loader::load(files)?;
    if loaded.artifacts_with_ast == 0 {
        bail!("no ASTs found under {}", out_dir.display());
    }
    let ix = index::build(&loaded.sources);
    Ok((resolve::build_graph(&ix, Vec::new()), loaded.seen))
}

fn load_graph(args: &Args) -> Result<Graph> {
    let files = loader::artifact_files(&args.out_dir)?;
    if files.is_empty() {
        bail!("no artifacts found under {}", args.out_dir.display());
    }
    let fp = fingerprint::compute(&args.out_dir, &files)?;
    if !args.no_cache
        && let Some(graph) = cache::load(fp)
    {
        return Ok(graph);
    }
    let (graph, seen) = build(&args.out_dir, &files)?;
    if !args.no_cache
        && let Err(e) = cache::store(fp, &seen, &graph)
    {
        eprintln!("warning: could not write cache: {e:#}");
    }
    Ok(graph)
}

fn try_run() -> Result<ExitCode> {
    let args = Args::parse();
    let graph = load_graph(&args)?;
    for line in &graph.diag {
        eprintln!("{line}");
    }

    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());

    if args.dump_graph {
        for (i, node) in graph.nodes.iter().enumerate() {
            writeln!(out, "{}{}", graph.label(i as u32), if node.entry { " [entry]" } else { "" })?;
            for e in &graph.adj[i] {
                writeln!(out, "    -> {}{}", if e.approx { "~" } else { "" }, graph.label(e.to))?;
            }
        }
        out.flush()?;
        return Ok(ExitCode::SUCCESS);
    }

    if args.list {
        let mut code = ExitCode::SUCCESS;
        for (i, name) in args.names.iter().enumerate() {
            let mut nodes = query::contract_functions(&graph, name);
            if nodes.is_empty() {
                out.flush()?;
                eprintln!("no contract matching `{name}`");
                code = ExitCode::FAILURE;
                continue;
            }
            if args.names.len() > 1 {
                if i > 0 {
                    writeln!(out)?;
                }
                writeln!(out, "# {name}")?;
            }
            // Overridden functions leave the base node behind; keep the most derived.
            nodes.sort_by_key(|&n| (graph.nodes[n as usize].sig.clone(), graph.inherited_ctx(n).is_some()));
            nodes.dedup_by_key(|&mut n| {
                let node = &graph.nodes[n as usize];
                (node.sig.clone(), node.kind)
            });
            nodes.sort_by_key(|&n| graph.name(n));
            for n in nodes {
                let full = graph.name(n);
                let decl = &graph.contracts[graph.nodes[n as usize].decl as usize].display;
                let line = full.strip_prefix(decl.as_str()).and_then(|f| f.strip_prefix('.')).unwrap_or(&full);
                match graph.inherited_ctx(n) {
                    Some(_) => writeln!(out, "{line} (from {decl})")?,
                    None => writeln!(out, "{line}")?,
                }
            }
        }
        out.flush()?;
        return Ok(code);
    }

    let mode = if args.soft { render::Mode::Soft } else { render::Mode::Hard };
    let mut code = ExitCode::SUCCESS;
    for (i, name) in args.names.iter().enumerate() {
        let targets = query::find_targets(&graph, name);
        if targets.is_empty() {
            out.flush()?;
            eprintln!("no function matching `{name}`");
            code = ExitCode::FAILURE;
            continue;
        }
        if args.names.len() > 1 {
            if i > 0 {
                writeln!(out)?;
            }
            writeln!(out, "# {name}")?;
        }
        if !render::render(&graph, &targets, mode, &mut out)? {
            out.flush()?;
            eprintln!("no entry point reaches `{name}`{}", if args.soft { " through a `~` call" } else { "" });
            code = ExitCode::FAILURE;
        }
    }
    out.flush()?;
    Ok(code)
}
