//! texo CLI entrypoint.

mod commands;

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use texo_mcp::run_stdio;

#[derive(Parser)]
#[command(name = "texo", about = "Context version control for claims")]
struct Cli {
    /// Workspace root (defaults to current directory).
    #[arg(long, global = true, default_value = ".")]
    root: PathBuf,

    /// BatPak workspace scope id (defaults to config default).
    #[arg(long, global = true)]
    workspace: Option<String>,

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
    let workspace = cli.workspace.as_deref();
    match cli.command {
        Commands::Init { workspace: init_ws } => commands::init::run(&cli.root, &init_ws),
        Commands::Ingest {
            path,
            dry_run,
            json,
        } => commands::ingest::run(&cli.root, &path, workspace, dry_run, json),
        Commands::Claims { subject, json } => {
            commands::claims::run(&cli.root, workspace, subject.as_deref(), json)
        }
        Commands::Supersede {
            old,
            new,
            reason,
            decided_by,
            json,
        } => commands::supersede::run(&cli.root, workspace, &old, &new, &reason, &decided_by, json),
        Commands::CheckStaleness { path, json } => {
            commands::check_staleness::run(&cli.root, workspace, &path, json)
        }
        Commands::AgentContext { subject, out, json } => commands::agent_context::run(
            &cli.root,
            workspace,
            subject.as_deref(),
            out.as_deref(),
            json,
        ),
        Commands::Compile { out } => commands::compile::run(&cli.root, workspace, &out),
        Commands::Conflicts { json, commit } => {
            commands::conflicts::run(&cli.root, workspace, json, commit)
        }
        Commands::Verify { json } => commands::verify::run(&cli.root, workspace, json),
        Commands::Mcp => {
            let rt = tokio::runtime::Runtime::new()
                .context("failed to create tokio runtime for MCP server")?;
            rt.block_on(run_stdio(cli.root, workspace.map(str::to_string)))
                .context("MCP stdio server failed")?;
            Ok(())
        }
    }
}

/// Observation timestamp (in milliseconds since the Unix epoch) for writes.
///
/// Returns real wall-clock time by default. If the `TEXO_OBSERVED_AT_MS`
/// environment variable is set and parses as a `u64`, that value is returned
/// instead. This override exists so golden/integration tests can pin a
/// deterministic timestamp (e.g. the fixture constant).
pub fn observed_at_ms() -> u64 {
    if let Ok(raw) = std::env::var("TEXO_OBSERVED_AT_MS") {
        if let Ok(parsed) = raw.trim().parse::<u64>() {
            return parsed;
        }
    }
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    u64::try_from(millis).unwrap_or(u64::MAX)
}
