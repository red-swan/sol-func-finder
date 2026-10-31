use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Result, bail};
use clap::Parser;

use crate::graph::{FKind, Graph, Vis};
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
    /// has (declared and inherited) as `Declaring.function`, instead of
    /// searching for callers.
    #[arg(long)]
    list: bool,

    /// With `--list`, also list internal and private functions and modifiers
    /// (by default only public and external ones are listed).
    #[arg(long)]
    internal: bool,

    /// With `--list`, also list the getters the compiler generates for public
    /// state variables (hidden by default).
    #[arg(long)]
    getters: bool,

    /// With `--list`, put the source path in front of each line
    /// (`path/File.sol:Contract.function`). Lines never carry a path otherwise.
    #[arg(long)]
    paths: bool,

    /// Only print functions in these contracts or source files (repeatable).
    /// A name with a `/` or ending in `.sol` matches a source path by
    /// substring, anything else a contract name. Applies to the functions
    /// printed: `--list` lines, `--entries` lines, and the top line of each
    /// tree. The graph itself is unfiltered.
    #[arg(long, value_name = "CONTRACT|PATH")]
    include: Vec<String>,

    /// Leave out functions in these contracts or source files (repeatable).
    /// Same patterns as `--include`; wins over it.
    #[arg(long, value_name = "CONTRACT|PATH")]
    exclude: Vec<String>,

    /// Print only the entry points that reach the function, one
    /// `Contract.function` per line (the contract it is called on), without
    /// the calls in between.
    #[arg(long)]
    entries: bool,

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
    let mut diag = Vec::new();
    let missing = loaded.artifacts_total - loaded.artifacts_with_ast;
    if missing > 0 {
        diag.push(format!(
            "warning: {missing} of {} artifacts have no ast; contracts in them are missing from results. Rebuild with `forge build --ast`",
            loaded.artifacts_total
        ));
    }
    let ix = index::build(&loaded.sources);
    Ok((resolve::build_graph(&ix, diag), loaded.seen))
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
            writeln!(
                out,
                "{}{}",
                graph.label(i as u32),
                if node.entry { " [entry]" } else { "" }
            )?;
            for e in &graph.adj[i] {
                writeln!(
                    out,
                    "    -> {}{}",
                    if e.approx { "~" } else { "" },
                    graph.label(e.to)
                )?;
            }
        }
        out.flush()?;
        return Ok(ExitCode::SUCCESS);
    }

    let filter = query::Filter {
        include: args.include.clone(),
        exclude: args.exclude.clone(),
    };

    if args.list {
        let mut code = ExitCode::SUCCESS;
        for (i, name) in args.names.iter().enumerate() {
            let mut nodes = query::contract_functions(&graph, name);
            let had = !nodes.is_empty();
            if !had {
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
            nodes.sort_by_key(|&n| {
                (
                    graph.nodes[n as usize].sig.clone(),
                    graph.inherited_ctx(n).is_some(),
                )
            });
            nodes.dedup_by_key(|&mut n| {
                let node = &graph.nodes[n as usize];
                (node.sig.clone(), node.kind)
            });
            nodes.retain(|&n| {
                (args.getters || graph.nodes[n as usize].kind != FKind::Getter)
                    && (args.internal
                        || matches!(graph.nodes[n as usize].vis, Vis::Public | Vis::External))
                    && filter.keep(&graph, n)
            });
            let mut lines: Vec<String> = nodes
                .iter()
                .map(|&n| {
                    // `name` carries a `path:` prefix for duplicate contract names; rebuild it.
                    let full = graph.name(n);
                    let decl = &graph.contracts[graph.nodes[n as usize].decl as usize];
                    let rest = full.strip_prefix(decl.display.as_str()).unwrap_or(&full);
                    if args.paths {
                        format!("{}:{}{rest}", decl.path, decl.name)
                    } else {
                        format!("{}{rest}", decl.name)
                    }
                })
                .collect();
            lines.sort();
            for line in lines {
                writeln!(out, "{line}")?;
            }
        }
        out.flush()?;
        return Ok(code);
    }

    let mode = if args.soft {
        render::Mode::Soft
    } else {
        render::Mode::Hard
    };
    let mut code = ExitCode::SUCCESS;
    for (i, name) in args.names.iter().enumerate() {
        let mut targets = query::find_targets(&graph, name);
        let had = !targets.is_empty();
        if !args.entries {
            targets.retain(|&n| filter.keep(&graph, n));
        }
        if !had {
            out.flush()?;
            eprintln!("no function matching `{name}`");
            code = ExitCode::FAILURE;
            continue;
        }
        if targets.is_empty() {
            out.flush()?;
            eprintln!("`{name}` matches only functions removed by --include/--exclude");
            code = ExitCode::FAILURE;
            continue;
        }
        if args.names.len() > 1 {
            if i > 0 {
                writeln!(out)?;
            }
            writeln!(out, "# {name}")?;
        }
        let found = if args.entries {
            let entries = render::entries(&graph, &targets, mode, |n| filter.keep(&graph, n));
            for e in &entries {
                writeln!(out, "{e}")?;
            }
            !entries.is_empty()
        } else {
            render::render(&graph, &targets, mode, &mut out)?
        };
        if !found {
            out.flush()?;
            eprintln!(
                "no entry point reaches `{name}`{}",
                if args.soft { " through a `~` call" } else { "" }
            );
            code = ExitCode::FAILURE;
        }
    }
    out.flush()?;
    Ok(code)
}
