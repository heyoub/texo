//! texo CLI entrypoint.

mod commands;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use texo_core::fixture::FIXTURE_OBSERVED_AT_MS;
use texo_mcp::run_stdio;

#[derive(Parser)]
#[command(name = "texo", about = "Context version control for claims")]
struct Cli {
    /// Workspace root (defaults to current directory).
    #[arg(long, global = true, default_value = ".")]
    root: PathBuf,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize `.texo` config and store directory.
    Init {
        #[arg(long, default_value = "demo")]
        workspace: String,
    },
    /// Ingest markdown sources.
    Ingest {
        path: PathBuf,
        #[arg(long)]
        workspace: Option<String>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        json: bool,
    },
    /// Replay and print claims.
    Claims {
        #[arg(long)]
        subject: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Mark one claim superseded by another.
    Supersede {
        old: String,
        new: String,
        #[arg(long)]
        reason: String,
        #[arg(long, default_value = "human")]
        decided_by: String,
        #[arg(long)]
        json: bool,
    },
    /// Check markdown for stale claims.
    CheckStaleness {
        path: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Generate agent context JSON.
    AgentContext {
        #[arg(long)]
        subject: Option<String>,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Compile human-readable artifacts.
    Compile {
        #[arg(long, default_value = "public")]
        out: PathBuf,
    },
    /// Report possible conflicts.
    Conflicts {
        #[arg(long)]
        json: bool,
        #[arg(long)]
        commit: bool,
    },
    /// Verify replay consistency.
    Verify {
        #[arg(long)]
        json: bool,
    },
    /// Run MCP stdio server.
    Mcp,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Init { workspace } => commands::init::run(&cli.root, &workspace),
        Commands::Ingest {
            path,
            workspace,
            dry_run,
            json,
        } => commands::ingest::run(&cli.root, &path, workspace.as_deref(), dry_run, json),
        Commands::Claims { subject, json } => {
            commands::claims::run(&cli.root, subject.as_deref(), json)
        }
        Commands::Supersede {
            old,
            new,
            reason,
            decided_by,
            json,
        } => commands::supersede::run(&cli.root, &old, &new, &reason, &decided_by, json),
        Commands::CheckStaleness { path, json } => {
            commands::check_staleness::run(&cli.root, &path, json)
        }
        Commands::AgentContext { subject, out, json } => {
            commands::agent_context::run(&cli.root, subject.as_deref(), out.as_deref(), json)
        }
        Commands::Compile { out } => commands::compile::run(&cli.root, &out),
        Commands::Conflicts { json, commit } => commands::conflicts::run(&cli.root, json, commit),
        Commands::Verify { json } => commands::verify::run(&cli.root, json),
        Commands::Mcp => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(run_stdio(cli.root))?;
            Ok(())
        }
    }
}

/// Observation timestamp for writes (fixed in tests via env override pattern).
pub fn observed_at_ms() -> u64 {
    FIXTURE_OBSERVED_AT_MS
}
