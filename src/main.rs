mod ast;
mod cache;
mod cli;
mod fingerprint;
mod graph;
mod index;
mod loader;
mod query;
mod render;
mod resolve;

fn main() -> std::process::ExitCode {
    cli::run()
}
