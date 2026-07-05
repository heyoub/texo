//! texo CLI entrypoint.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

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
    command: Command,
}

#[derive(Subcommand)]
enum Command {
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
    /// Semantic supersession + conflict pass (OpenRouter; needs OPENROUTER_API_KEY).
    Relate {
        #[arg(long)]
        json: bool,
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
    /// Run the memory-agent HTTP server.
    Serve,
    /// Extract claims from one path.
    Extract { path: std::path::PathBuf },
    /// Session utilities.
    Session {
        #[command(subcommand)]
        cmd: SessionCmd,
    },
    /// Host utilities.
    Host {
        #[command(subcommand)]
        cmd: HostCmd,
    },
}

#[derive(Subcommand)]
enum SessionCmd {
    /// Export a session transcript.
    Export { session_id: String },
}

#[derive(Subcommand)]
enum HostCmd {
    /// Print the host fingerprint.
    Fingerprint,
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "texo=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    if let Err(error) = batpak::event::validate_event_payload_registry() {
        eprintln!("{error}");
        return ExitCode::FAILURE;
    }

    let cli = Cli::parse();
    let _ = (&cli.root, &cli.workspace);
    match cli.command {
        Command::Init { workspace } => {
            let _ = workspace;
            unimplemented_command("init")
        }
        Command::Ingest {
            path,
            dry_run,
            json,
        } => {
            let _ = (path, dry_run, json);
            unimplemented_command("ingest")
        }
        Command::Claims { subject, json } => {
            let _ = (subject, json);
            unimplemented_command("claims")
        }
        Command::Supersede {
            old,
            new,
            reason,
            decided_by,
            json,
        } => {
            let _ = (old, new, reason, decided_by, json);
            unimplemented_command("supersede")
        }
        Command::CheckStaleness { path, json } => {
            let _ = (path, json);
            unimplemented_command("check-staleness")
        }
        Command::AgentContext { subject, out, json } => {
            let _ = (subject, out, json);
            unimplemented_command("agent-context")
        }
        Command::Compile { out } => {
            let _ = out;
            unimplemented_command("compile")
        }
        Command::Relate { json } => {
            let _ = json;
            unimplemented_command("relate")
        }
        Command::Conflicts { json, commit } => {
            let _ = (json, commit);
            unimplemented_command("conflicts")
        }
        Command::Verify { json } => {
            let _ = json;
            unimplemented_command("verify")
        }
        Command::Mcp => unimplemented_command("mcp"),
        Command::Serve => unimplemented_command("serve"),
        Command::Extract { path } => {
            let _ = path;
            unimplemented_command("extract")
        }
        Command::Session { cmd } => match cmd {
            SessionCmd::Export { session_id } => {
                let _ = session_id;
                unimplemented_command("session export")
            }
        },
        Command::Host { cmd } => match cmd {
            HostCmd::Fingerprint => unimplemented_command("host fingerprint"),
        },
    }
}

fn unimplemented_command(name: &str) -> ExitCode {
    eprintln!("texo: '{name}' is not wired yet (rebuild in progress)");
    ExitCode::FAILURE
}
