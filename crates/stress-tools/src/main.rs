//! Devo stress tooling: synthetic session JSONL corpora + RPC latency probes.
//!
//! Generate a large `DEVO_HOME`-shaped tree (roots under `sessions/`, children
//! under `session-artifacts/<root>/sub-xxxxxxxx/`), then validate lines with the
//! same v2 parser production uses, and optionally bench list/resume/items RPCs.

mod bench;
mod generate;
mod validate;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "devo-stress", about = "Session/subagent stress corpus tools")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Write many valid v2 rollout JSONL files (roots + nested subagents).
    Generate(generate::GenerateArgs),
    /// Parse every generated JSONL line with the production v2 reader.
    Validate {
        /// Corpus root (contains `sessions/` and optional `session-artifacts/`).
        #[arg(long)]
        corpus: PathBuf,
    },
    /// Spawn `devo server --transport stdio` against a corpus and time hot RPCs.
    Bench(bench::BenchArgs),
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Generate(args) => generate::run(args),
        Command::Validate { corpus } => validate::run(&corpus),
        Command::Bench(args) => bench::run(args),
    }
}
