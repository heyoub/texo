//! Clap-driven CLI surface.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::{Parser, Subcommand};
use serde_json::{json, Value};

use crate::config::TexoRootConfig;
use crate::error::TexoError;
use crate::host::TexoHost;

/// CLI render helpers.
pub mod render;

#[derive(Parser)]
#[command(name = "texo", about = "Context version control for claims")]
struct Cli {
    /// Workspace root (defaults to current directory).
    #[arg(long, global = true, default_value = ".")]
    root: PathBuf,

    /// `BatPak` workspace scope id (defaults to config default).
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
    /// Semantic supersession + conflict pass (`OpenRouter`; needs `OPENROUTER_API_KEY`).
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
    Extract { path: PathBuf },
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

/// Run the CLI and return the requested process exit code.
///
/// # Errors
///
/// Returns [`TexoError`] when command execution fails.
pub fn run() -> Result<ExitCode, TexoError> {
    let cli = Cli::parse();
    dispatch(cli)
}

#[expect(
    clippy::too_many_lines,
    reason = "CLI dispatch table mirrors the command surface during rebuild"
)]
fn dispatch(cli: Cli) -> Result<ExitCode, TexoError> {
    match cli.command {
        Command::Init { workspace } => {
            let mut host = TexoHost::open(cli.root.clone(), workspace.clone(), observed_at_ms())?;
            let output =
                host.invoke_json("texo.workspace.init", &json!({ "workspace_id": workspace }))?;
            render::init(&cli.root, &output);
            Ok(ExitCode::SUCCESS)
        }
        Command::Ingest {
            path,
            dry_run,
            json,
        } => {
            let mut host = open_host(&cli.root, cli.workspace.as_deref())?;
            let output = host.invoke_json(
                "texo.ingest.run",
                &json!({"path": path, "dry_run": dry_run, "observed_at_ms": observed_at_ms()}),
            )?;
            if json {
                render::json(&output)?;
            } else {
                render::ingest(&output);
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Claims { subject, json } => {
            let mut host = open_host(&cli.root, cli.workspace.as_deref())?;
            let output = host.invoke_json("texo.claims.list", &json!({"subject": subject}))?;
            if json {
                let claims = output
                    .get("claims")
                    .cloned()
                    .unwrap_or(Value::Array(Vec::new()));
                render::json(&claims)?;
            } else {
                render::claims(&output);
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Supersede {
            old,
            new,
            reason,
            decided_by,
            json,
        } => {
            let mut host = open_host(&cli.root, cli.workspace.as_deref())?;
            let output = host.invoke_json(
                "texo.claim.supersede",
                &json!({
                    "old": old,
                    "new": new,
                    "reason": reason,
                    "decided_by": decided_by,
                    "observed_at_ms": observed_at_ms()
                }),
            )?;
            if json {
                render::json(&output)?;
            } else {
                render::supersede(&output);
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::CheckStaleness { path, json } => {
            let mut host = open_host(&cli.root, cli.workspace.as_deref())?;
            let output = host.invoke_json("texo.staleness.check", &json!({"path": path}))?;
            let has_findings = output
                .get("diagnostics")
                .and_then(Value::as_array)
                .is_some_and(|diagnostics| !diagnostics.is_empty());
            if json {
                render::json(&output)?;
            } else {
                render::staleness(&output);
            }
            Ok(if has_findings {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            })
        }
        Command::AgentContext { subject, out, json } => {
            let mut host = open_host(&cli.root, cli.workspace.as_deref())?;
            let output = host.invoke_json(
                "texo.context.agent",
                &json!({"subject": subject, "include_stale": true}),
            )?;
            let rendered = serde_json::to_string_pretty(&output)?;
            if let Some(path) = out {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&path, &rendered)?;
                if json {
                    render::json(&output)?;
                }
            } else {
                let _ = json;
                render::json(&output)?;
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Compile { out } => {
            let mut host = open_host(&cli.root, cli.workspace.as_deref())?;
            let out_for_input = out.clone();
            let output = host.invoke_json(
                "texo.compile.run",
                &json!({"out_dir": out_for_input, "observed_at_ms": observed_at_ms()}),
            )?;
            render::compile(&out, &output);
            Ok(ExitCode::SUCCESS)
        }
        Command::Conflicts { json, commit } => {
            let mut host = open_host(&cli.root, cli.workspace.as_deref())?;
            if commit {
                let output = host.invoke_json(
                    "texo.conflicts.commit",
                    &json!({"observed_at_ms": observed_at_ms()}),
                )?;
                if json {
                    render::json(&output)?;
                } else {
                    render::conflicts_committed(&output);
                }
            } else {
                let output = host.invoke_json("texo.conflicts.list", &json!({}))?;
                if json {
                    render::json(&output)?;
                } else {
                    render::conflicts(&output);
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Verify { json } => {
            let mut host = open_host(&cli.root, cli.workspace.as_deref())?;
            let output = host.invoke_json("texo.verify.run", &json!({}))?;
            let failed = ["projection_ok", "journal_ok", "transitions_ok"]
                .iter()
                .any(|key| output.get(*key).and_then(Value::as_bool) == Some(false));
            if json {
                render::json(&output)?;
            } else if failed {
                return Err(TexoError::Verify {
                    failures: output
                        .get("errors")
                        .and_then(Value::as_array)
                        .map(|errors| {
                            errors
                                .iter()
                                .filter_map(Value::as_str)
                                .map(str::to_owned)
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default(),
                });
            } else {
                render::verify(&output);
            }
            Ok(if failed {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            })
        }
        Command::Host {
            cmd: HostCmd::Fingerprint,
        } => {
            let mut host = open_host(&cli.root, cli.workspace.as_deref())?;
            let output = host.invoke_json("texo.host.fingerprint", &json!({}))?;
            render::json(&output)?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Relate { json } => {
            let _ = json;
            Ok(unimplemented_command("relate"))
        }
        Command::Mcp => Ok(unimplemented_command("mcp")),
        Command::Serve => Ok(unimplemented_command("serve")),
        Command::Extract { path } => {
            let _ = path;
            Ok(unimplemented_command("extract"))
        }
        Command::Session { cmd } => match cmd {
            SessionCmd::Export { session_id } => {
                let _ = session_id;
                Ok(unimplemented_command("session export"))
            }
        },
    }
}

fn open_host(root: &Path, workspace: Option<&str>) -> Result<TexoHost, TexoError> {
    let workspace = resolve_workspace(root, workspace)?;
    TexoHost::open(root.to_path_buf(), workspace, observed_at_ms())
}

fn resolve_workspace(root: &Path, workspace: Option<&str>) -> Result<String, TexoError> {
    if let Some(workspace) = workspace {
        return Ok(workspace.to_string());
    }
    let config_path = root.join(".texo").join("config.toml");
    if config_path.exists() {
        return TexoRootConfig::load(&config_path)
            .map_err(|error| TexoError::Config {
                detail: error.to_string(),
                source: Some(Box::new(error)),
            })?
            .resolve(None)
            .map(|config| config.workspace_id)
            .map_err(|error| TexoError::Config {
                detail: error.to_string(),
                source: Some(Box::new(error)),
            });
    }
    Ok("demo".to_string())
}

/// Observation timestamp in milliseconds since the Unix epoch.
#[must_use]
pub fn observed_at_ms() -> u64 {
    if let Ok(raw) = std::env::var("TEXO_OBSERVED_AT_MS") {
        if let Ok(parsed) = raw.trim().parse::<u64>() {
            return parsed;
        }
    }
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    u64::try_from(millis).unwrap_or(u64::MAX)
}

#[expect(clippy::print_stderr, reason = "CLI output contract")]
fn unimplemented_command(name: &str) -> ExitCode {
    eprintln!("texo: '{name}' is not wired yet (rebuild in progress)");
    ExitCode::FAILURE
}
