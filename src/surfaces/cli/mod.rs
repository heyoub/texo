//! Clap-driven CLI surface.

use std::io::Read;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::{Parser, Subcommand};
use serde_json::{json, Value};

use crate::config::TexoRootConfig;
use crate::error::TexoError;
use crate::host::TexoHost;
use crate::install::ClientTarget;

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
        /// Abort before all appends if any source fails planning.
        #[arg(long)]
        strict: bool,
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
        /// Refuse context while semantic settlement remains incomplete.
        #[arg(long)]
        strict_settlement: bool,
    },
    /// Compile human-readable artifacts.
    Compile {
        #[arg(long, default_value = "public")]
        out: PathBuf,
        /// Refuse compilation while semantic settlement remains incomplete.
        #[arg(long)]
        strict_settlement: bool,
    },
    /// Semantic supersession + conflict pass (needs `TEXO_LLM_API_KEY`).
    Relate {
        #[arg(long)]
        json: bool,
        /// Refuse derived authority while any required pair is unresolved.
        #[arg(long)]
        strict: bool,
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
    /// Report deterministic workspace counters and artifact sizes.
    Stats {
        #[arg(long)]
        json: bool,
    },
    /// Run MCP stdio server.
    Mcp,
    /// Run the memory-agent HTTP server.
    Serve {
        /// Listen address.
        #[arg(long)]
        addr: Option<String>,
        /// Workspace root.
        #[arg(long)]
        root: Option<PathBuf>,
        /// Workspace id.
        #[arg(long)]
        workspace: Option<String>,
    },
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
    /// Discover the typed operation surface.
    Ops {
        #[command(subcommand)]
        cmd: OpsCmd,
    },
    /// Install Texo's agent-facing workspace adapters.
    Install {
        /// Client adapters to manage; auto-detects when omitted.
        #[arg(long, value_enum)]
        client: Vec<ClientTarget>,
        /// Preview the exact file changes without writing.
        #[arg(long)]
        dry_run: bool,
        /// Emit the stable machine-readable report.
        #[arg(long)]
        json: bool,
    },
    /// Remove only Texo-managed workspace adapters and guidance.
    Uninstall {
        /// Preview the exact file changes without writing.
        #[arg(long)]
        dry_run: bool,
        /// Emit the stable machine-readable report.
        #[arg(long)]
        json: bool,
    },
    /// Run a fixed read-only agent hook.
    Hook {
        #[command(subcommand)]
        cmd: HookCmd,
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

#[derive(Subcommand)]
enum OpsCmd {
    /// List registered operations and agent exposure.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Describe one registered operation.
    Describe {
        name: String,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum HookCmd {
    /// Return bounded context for the start of an agent session.
    SessionStart {
        #[arg(long)]
        json: bool,
    },
    /// Check changed workspace paths supplied as a bounded JSON stdin envelope.
    FilesChanged {
        #[arg(long)]
        json: bool,
    },
    /// Verify the journal and projection before a commit.
    PreCommit {
        #[arg(long)]
        json: bool,
    },
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
            let mut host =
                TexoHost::open_for_init(cli.root.clone(), workspace.clone(), observed_at_ms())?;
            let output =
                host.invoke_json("texo.workspace.init", &json!({ "workspace_id": workspace }))?;
            render::init(&cli.root, &output);
            Ok(ExitCode::SUCCESS)
        }
        Command::Ingest {
            path,
            dry_run,
            strict,
            json,
        } => {
            let mut host = open_host(&cli.root, cli.workspace.as_deref())?;
            let output = host.invoke_json(
                "texo.ingest.run",
                &json!({
                    "path": path,
                    "dry_run": dry_run,
                    "strict": strict,
                    "observed_at_ms": observed_at_ms()
                }),
            )?;
            if json {
                render::json(&output)?;
            } else {
                render::ingest(&output);
            }
            Ok(
                if output.get("outcome").and_then(Value::as_str) == Some("partial") {
                    ExitCode::from(2)
                } else {
                    ExitCode::SUCCESS
                },
            )
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
        Command::AgentContext {
            subject,
            out,
            json,
            strict_settlement,
        } => {
            let mut host = open_host(&cli.root, cli.workspace.as_deref())?;
            let output = host.invoke_json(
                "texo.context.agent",
                &json!({
                    "subject": subject,
                    "include_stale": true,
                    "strict_settlement": strict_settlement
                }),
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
        Command::Compile {
            out,
            strict_settlement,
        } => {
            let mut host = open_host(&cli.root, cli.workspace.as_deref())?;
            let out_for_input = out.clone();
            let output = host.invoke_json(
                "texo.compile.run",
                &json!({
                    "out_dir": out_for_input,
                    "observed_at_ms": observed_at_ms(),
                    "strict_settlement": strict_settlement
                }),
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
        Command::Stats { json: _ } => {
            let mut host = open_host(&cli.root, cli.workspace.as_deref())?;
            let output = host.invoke_json("texo.stats.read", &json!({}))?;
            render::json(&output)?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Host {
            cmd: HostCmd::Fingerprint,
        } => {
            let mut host = open_host(&cli.root, cli.workspace.as_deref())?;
            let output = host.invoke_json("texo.host.fingerprint", &json!({}))?;
            render::json(&output)?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Relate { json, strict } => {
            let mut host = open_host(&cli.root, cli.workspace.as_deref())?;
            let output = host.invoke_json(
                "texo.relate.run",
                &json!({"observed_at_ms": observed_at_ms(), "strict": strict}),
            )?;
            if json {
                render::json(&output)?;
            } else {
                render::relate(&output);
            }
            Ok(
                if output.get("outcome").and_then(Value::as_str) == Some("partial") {
                    ExitCode::from(2)
                } else {
                    ExitCode::SUCCESS
                },
            )
        }
        Command::Mcp => {
            crate::surfaces::mcp_stdio::run(&cli.root, cli.workspace.as_deref())?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Serve {
            addr,
            root,
            workspace,
        } => serve(addr, root, workspace, &cli.root, cli.workspace.as_deref()),
        Command::Extract { path } => Ok(extract(&path)),
        Command::Session { cmd } => match cmd {
            SessionCmd::Export { session_id } => {
                let mut host = open_host(&cli.root, cli.workspace.as_deref())?;
                let output =
                    host.invoke_json("texo.session.export", &json!({"session_id": session_id}))?;
                render::session_markdown(
                    output
                        .get("markdown")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                );
                Ok(ExitCode::SUCCESS)
            }
        },
        Command::Ops { cmd } => {
            let inventory = crate::agent_catalog::operation_inventory();
            match cmd {
                OpsCmd::List { json } => {
                    if json {
                        render::json(&inventory)?;
                    } else {
                        render::operations(&inventory);
                    }
                }
                OpsCmd::Describe { name, json } => {
                    let operation = inventory["operations"]
                        .as_array()
                        .and_then(|operations| {
                            operations
                                .iter()
                                .find(|operation| operation["name"] == name)
                        })
                        .cloned()
                        .ok_or_else(|| TexoError::OpInput {
                            op: "texo ops describe".to_string(),
                            detail: format!("unknown operation `{name}`"),
                        })?;
                    if json {
                        render::json(&operation)?;
                    } else {
                        render::operations(&json!({"operations": [operation]}));
                    }
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Install {
            client,
            dry_run,
            json,
        } => {
            let workspace = cli.workspace.as_deref().unwrap_or("demo");
            let report = crate::install::install(&cli.root, workspace, &client, dry_run)?;
            let output = serde_json::to_value(report)?;
            if json {
                render::json(&output)?;
            } else {
                render::installation(&output);
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Uninstall { dry_run, json } => {
            let report = crate::install::uninstall(&cli.root, dry_run)?;
            let output = serde_json::to_value(report)?;
            if json {
                render::json(&output)?;
            } else {
                render::installation(&output);
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Hook { cmd } => {
            let mut host = open_host(&cli.root, cli.workspace.as_deref())?;
            let (event, data, json) = match cmd {
                HookCmd::SessionStart { json } => (
                    "session_start",
                    host.invoke_json(
                        "texo.context.agent",
                        &json!({
                            "subject": null,
                            "include_stale": true,
                            "strict_settlement": false
                        }),
                    )?,
                    json,
                ),
                HookCmd::FilesChanged { json } => {
                    let mut bytes = Vec::new();
                    std::io::stdin()
                        .take((crate::hooks::MAX_INPUT_BYTES + 1) as u64)
                        .read_to_end(&mut bytes)?;
                    let input = crate::hooks::parse_files_changed(&bytes)?;
                    let mut reports = Vec::with_capacity(input.paths.len());
                    for path in input.paths {
                        reports.push(
                            host.invoke_json("texo.staleness.check", &json!({"path": path}))?,
                        );
                    }
                    ("files_changed", json!({"reports": reports}), json)
                }
                HookCmd::PreCommit { json } => (
                    "pre_commit",
                    host.invoke_json("texo.verify.run", &json!({}))?,
                    json,
                ),
            };
            let output = json!({
                "schema": "texo.hook-result.v1",
                "event": event,
                "advisory": true,
                "data": data
            });
            let _ = json;
            render::json(&output)?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn serve(
    addr: Option<String>,
    root: Option<PathBuf>,
    workspace: Option<String>,
    global_root: &Path,
    global_workspace: Option<&str>,
) -> Result<ExitCode, TexoError> {
    let addr = addr
        .or_else(|| std::env::var("TEXO_AGENT_ADDR").ok())
        .unwrap_or_else(|| "127.0.0.1:8787".to_string());
    let root = root
        .or_else(|| {
            if global_root == Path::new(".") {
                None
            } else {
                Some(global_root.to_path_buf())
            }
        })
        .or_else(|| std::env::var("TEXO_AGENT_ROOT").ok().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."));
    let workspace = workspace
        .or_else(|| global_workspace.map(str::to_string))
        .or_else(|| std::env::var("TEXO_AGENT_WORKSPACE").ok())
        .unwrap_or_else(|| "memory".to_string());
    let decision = crate::surfaces::bootstrap::resolve_bootstrap_from_env(&root)?;
    if let Some(warning) = &decision.warning {
        render::serve_warning(warning);
    }
    crate::surfaces::bootstrap::ensure_workspace(&root, &workspace, &decision)?;
    let listener = TcpListener::bind(&addr).map_err(|error| TexoError::Surface {
        which: crate::error::SurfaceKind::Http,
        detail: error.to_string(),
    })?;
    let local = listener.local_addr().map_err(|error| TexoError::Surface {
        which: crate::error::SurfaceKind::Http,
        detail: error.to_string(),
    })?;
    render::serve_listening(local);
    let store = crate::host::open_workspace_store(&root, &workspace)?;
    let gateway = crate::config::TexoRootConfig::load(&root.join(".texo/config.toml"))
        .ok()
        .and_then(|config| config.gateway);
    let chat_role = crate::gateway::resolve_role(
        crate::gateway::ModelRole::Chat,
        &crate::gateway::RoleOverrides::default(),
        gateway.as_ref(),
    );
    let state = crate::surfaces::http::routes::RouteState {
        root,
        workspace_id: workspace,
        store: Some(store),
        projection_cache: std::sync::Arc::new(std::sync::Mutex::new(None)),
        chat_enabled: crate::host::grants_model_capability(Some(chat_role.api_key)),
    };
    let config = crate::surfaces::http::server::ServerConfig::new(local.to_string(), state);
    let shutdown = crate::surfaces::http::server::ShutdownHandle::new();
    shutdown.register_termination_signals()?;
    let _stats = crate::surfaces::http::server::serve_listener(listener, config, &shutdown)?;
    Ok(ExitCode::SUCCESS)
}

fn extract(path: &Path) -> ExitCode {
    match extract_impl(path) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            render::extract_error(error.as_ref());
            ExitCode::FAILURE
        }
    }
}

#[cfg(feature = "openrouter")]
fn extract_impl(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Write as _;

    use crate::extract::cache::CachingProposer;
    use crate::extract::faithfulness::DEFAULT_GROUNDING_THRESHOLD_PPM;
    use crate::extract::llm::{run_extraction, write_ndjson};
    use crate::semantics::openrouter::OpenRouterProposer;

    const ENV_CACHE_DIR: &str = "TEXO_EXTRACT_CACHE";
    const DEFAULT_CACHE_DIR: &str = ".texo/extract-cache";

    let source = std::fs::read_to_string(path)
        .map_err(|error| format!("reading {}: {error}", path.display()))?;
    let cache_dir =
        PathBuf::from(std::env::var_os(ENV_CACHE_DIR).unwrap_or_else(|| DEFAULT_CACHE_DIR.into()));
    let gateway = crate::config::TexoRootConfig::load(Path::new(".texo/config.toml"))
        .ok()
        .and_then(|config| config.gateway);
    let proposer =
        CachingProposer::new(OpenRouterProposer::new(None, gateway.as_ref())?, cache_dir);
    let claims = run_extraction(&source, &proposer, DEFAULT_GROUNDING_THRESHOLD_PPM)?;

    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    write_ndjson(&claims, &mut lock)?;
    lock.flush()?;
    Ok(())
}

#[cfg(not(feature = "openrouter"))]
fn extract_impl(_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    Err("openrouter feature is disabled".into())
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
