//! Clap-driven CLI surface.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::TexoRootConfig;
use crate::error::TexoError;
use crate::host::TexoHost;
use crate::install::ClientTarget;
use clap::{Args, Parser, Subcommand};

/// CLI render helpers.
pub mod render;

mod dispatch;

#[derive(Parser)]
#[command(name = "texo", about = "Agent-ready claim memory in one local binary")]
struct Cli {
    /// Workspace root (defaults to current directory).
    #[arg(long, global = true, default_value = ".")]
    root: PathBuf,

    /// `BatPak` workspace scope id (defaults to config default).
    #[arg(long, global = true)]
    workspace: Option<String>,

    /// Physical journal id (defaults to the workspace primary journal).
    #[arg(long, global = true)]
    journal: Option<String>,

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
        /// Permit context before semantic settlement completes.
        #[arg(long)]
        allow_unsettled: bool,
    },
    /// Compile human-readable artifacts.
    Compile {
        #[arg(long, default_value = "public")]
        out: PathBuf,
        /// Permit artifact compilation before semantic settlement completes.
        #[arg(long)]
        allow_unsettled: bool,
    },
    /// Semantic supersession + conflict pass (needs `TEXO_LLM_API_KEY`).
    Relate {
        #[arg(long)]
        json: bool,
        /// Refuse derived authority while any required pair is unresolved.
        #[arg(long)]
        strict: bool,
        /// Hard ceiling for previously-unsettled candidate pairs in this pass.
        #[arg(long)]
        pair_budget: Option<usize>,
        /// Resume candidate enumeration after this durable cursor.
        #[arg(long = "pair-cursor")]
        candidate_cursor: Option<u64>,
        /// Evict and freshly judge one settled pair; first judgment remains authoritative.
        #[arg(long, num_args = 2, value_names = ["OLDER_CLAIM", "NEWER_CLAIM"])]
        rejudge_pair: Option<Vec<String>>,
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
    /// Freeze the current Git commit and worktree as evidence.
    Index {
        /// Optional workspace-local SCIP protobuf index; built-in analyzers
        /// cover sources absent from it.
        #[arg(long)]
        scip: Option<PathBuf>,
        /// Maximum source files captured.
        #[arg(long)]
        max_files: Option<usize>,
        /// Maximum bytes captured from one source.
        #[arg(long)]
        max_file_bytes: Option<u64>,
        /// Maximum total captured source bytes.
        #[arg(long)]
        max_total_bytes: Option<u64>,
        /// Emit the stable machine-readable report.
        #[arg(long)]
        json: bool,
    },
    /// Reconcile current semantic claims against the frozen code index.
    Reconcile {
        /// Maximum code candidates considered for each claim.
        #[arg(long)]
        max_per_claim: Option<usize>,
        /// Maximum paid proposal candidates for the complete run.
        #[arg(long)]
        max_candidates: Option<usize>,
        /// Minimum policy acceptance score in parts per million.
        #[arg(long)]
        min_score_ppm: Option<u32>,
        /// Emit the stable machine-readable report.
        #[arg(long)]
        json: bool,
    },
    /// Run MCP stdio server.
    Mcp,
    /// Run the memory-agent HTTP server.
    Serve(ServeOptions),
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
        /// Client adapters to remove; omitting removes the complete installation.
        #[arg(long, value_enum)]
        client: Vec<ClientTarget>,
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
    /// Diagnose config, store, model readiness, and agent integration.
    Doctor {
        /// Verify the full journal and projection chain.
        #[arg(long)]
        deep: bool,
        /// Reconcile only Texo-owned integration files.
        #[arg(long)]
        fix: bool,
        /// Emit the stable machine-readable report.
        #[arg(long)]
        json: bool,
    },
    /// Create or verify an evidence-backed workspace backup.
    Backup {
        #[command(subcommand)]
        cmd: BackupCmd,
    },
    /// Bootstrap or advance a configured scale-out read replica.
    Replica {
        #[command(subcommand)]
        cmd: ReplicaCmd,
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

#[derive(Subcommand)]
enum BackupCmd {
    /// Create a new immutable backup directory.
    Create {
        dest: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Verify a backup using only the destination's bytes.
    Verify {
        dest: PathBuf,
        /// Out-of-band manifest hash printed when the backup was created.
        #[arg(long)]
        expect_manifest_hash: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Restore a verified backup into the fresh `--root` workspace.
    Restore {
        source: PathBuf,
        /// Out-of-band manifest hash printed when the backup was created.
        #[arg(long)]
        expect_manifest_hash: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum ReplicaCmd {
    /// Materialize a fresh exact fork or imported read model.
    Bootstrap {
        /// Replica journal id declared in `.texo/config.toml`.
        replica: String,
        /// Emit the stable machine-readable report.
        #[arg(long)]
        json: bool,
    },
    /// Advance an imported read model once from its durable source cursor.
    Follow {
        /// Replica journal id declared in `.texo/config.toml`.
        replica: String,
        /// Emit the stable machine-readable report.
        #[arg(long)]
        json: bool,
        /// Continue following until SIGINT/SIGTERM.
        #[arg(long)]
        watch: bool,
        /// Idle polling interval for `--watch`.
        #[arg(long, default_value_t = 250, value_parser = clap::value_parser!(u64).range(50..=60_000))]
        interval_ms: u64,
    },
}

#[derive(Args)]
struct ServeOptions {
    /// Listen address.
    #[arg(long)]
    addr: Option<String>,
    /// Workspace root.
    #[arg(long)]
    root: Option<PathBuf>,
    /// Workspace id.
    #[arg(long)]
    workspace: Option<String>,
    /// Physical journal id.
    #[arg(long)]
    journal: Option<String>,
    /// Optional private-network canonical replica-source listener address.
    #[arg(long)]
    replica_addr: Option<String>,
    /// Environment variable containing the replica MAC secret.
    #[arg(long, default_value = "TEXO_REPLICA_TOKEN")]
    replica_token_env: String,
}

impl ServeOptions {
    #[must_use]
    fn with_default_journal(mut self, default_journal: Option<&str>) -> Self {
        if self.journal.is_none() {
            self.journal = default_journal.map(str::to_string);
        }
        self
    }
}

struct PreparedServe {
    listener: TcpListener,
    config: crate::surfaces::http::server::ServerConfig,
    shutdown: crate::surfaces::http::server::ShutdownHandle,
    replica_thread: Option<std::thread::JoinHandle<Result<netbat::TcpServeStats, TexoError>>>,
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

fn dispatch(cli: Cli) -> Result<ExitCode, TexoError> {
    let Cli {
        root,
        workspace,
        journal,
        command,
    } = cli;
    let context = DispatchContext {
        root,
        workspace,
        journal,
    };
    dispatch::route(&context, command)
}

struct DispatchContext {
    root: PathBuf,
    workspace: Option<String>,
    journal: Option<String>,
}

fn follow_replica_until_shutdown(
    root: &Path,
    workspace: Option<&str>,
    replica: &str,
    interval_ms: u64,
) -> Result<ExitCode, TexoError> {
    let shutdown = crate::surfaces::http::server::ShutdownHandle::new();
    shutdown.register_termination_signals()?;
    let mut first = true;
    while !shutdown.is_shutdown() {
        let report = crate::replication::follow_once(root, workspace, replica)?;
        let changed = matches!(
            &report,
            crate::replication::ReplicaReport::ImportedReadModel { imported, .. } if *imported > 0
        );
        if first || changed {
            render::json(&serde_json::to_value(&report)?)?;
            first = false;
        }
        if !shutdown.is_shutdown() {
            std::thread::park_timeout(std::time::Duration::from_millis(interval_ms));
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn serve(
    options: ServeOptions,
    global_root: &Path,
    global_workspace: Option<&str>,
) -> Result<ExitCode, TexoError> {
    let PreparedServe {
        listener,
        config,
        shutdown,
        replica_thread,
    } = prepare_serve(options, global_root, global_workspace)?;
    let replica_stats = run_serve(&listener, config, &shutdown, replica_thread)?;
    Ok(report_serve(replica_stats))
}

fn prepare_serve(
    options: ServeOptions,
    global_root: &Path,
    global_workspace: Option<&str>,
) -> Result<PreparedServe, TexoError> {
    let addr = options
        .addr
        .or_else(|| std::env::var("TEXO_AGENT_ADDR").ok())
        .unwrap_or_else(|| "127.0.0.1:8787".to_string());
    let root = options
        .root
        .or_else(|| {
            if global_root == Path::new(".") {
                None
            } else {
                Some(global_root.to_path_buf())
            }
        })
        .or_else(|| std::env::var("TEXO_AGENT_ROOT").ok().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."));
    let workspace = options
        .workspace
        .or_else(|| global_workspace.map(str::to_string))
        .or_else(|| std::env::var("TEXO_AGENT_WORKSPACE").ok())
        .unwrap_or_else(|| "memory".to_string());
    let decision = crate::surfaces::bootstrap::resolve_bootstrap_from_env(&root)?;
    if let Some(warning) = &decision.warning {
        render::serve_warning(warning)?;
    }
    crate::surfaces::bootstrap::ensure_workspace(&root, &workspace, &decision)?;
    let root_config = crate::config::TexoRootConfig::load(&root.join(".texo/config.toml"))
        .map_err(|error| TexoError::Config {
            detail: error.to_string(),
            source: Some(Box::new(error)),
        })?;
    let (_workspace_config, selected_journal) = root_config
        .resolve_journal(Some(&workspace), options.journal.as_deref())
        .map_err(|error| TexoError::Config {
            detail: error.to_string(),
            source: Some(Box::new(error)),
        })?;
    let _refresh =
        crate::replication::refresh_reader(&root, Some(&workspace), selected_journal.id.as_str())?;
    let socket =
        crate::compat::netbat::private_socket_addr(&addr).map_err(|error| TexoError::Config {
            detail: format!("replica listener {addr}: {error}"),
            source: None,
        })?;
    let listener = TcpListener::bind(socket).map_err(|error| TexoError::Surface {
        which: crate::error::SurfaceKind::Http,
        detail: error.to_string(),
    })?;
    let local = listener.local_addr().map_err(|error| TexoError::Surface {
        which: crate::error::SurfaceKind::Http,
        detail: error.to_string(),
    })?;
    render::serve_listening(local)?;
    let store =
        crate::host::open_workspace_journal_store(&root, &workspace, selected_journal.id.as_str())?;
    let gateway = root_config.gateway;
    let chat_role = crate::gateway::resolve_role(
        crate::gateway::ModelRole::Chat,
        &crate::gateway::RoleOverrides::default(),
        gateway.as_ref(),
    );
    let served_workspace = workspace.clone();
    let state = crate::surfaces::http::routes::RouteState {
        root,
        workspace_id: workspace,
        journal_id: selected_journal.id.to_string(),
        store: Some(store.clone()),
        projection_cache: std::sync::Arc::new(std::sync::Mutex::new(None)),
        chat_enabled: crate::host::grants_model_capability(Some(chat_role.api_key.as_str())),
    };
    let config = crate::surfaces::http::server::ServerConfig::new(local.to_string(), state);
    let shutdown = crate::surfaces::http::server::ShutdownHandle::new();
    shutdown.register_termination_signals()?;
    let replica_thread = start_replica_server(
        options.replica_addr,
        &options.replica_token_env,
        &selected_journal,
        &store,
        &served_workspace,
        &shutdown,
    )?;
    Ok(PreparedServe {
        listener,
        config,
        shutdown,
        replica_thread,
    })
}

fn run_serve(
    listener: &TcpListener,
    config: crate::surfaces::http::server::ServerConfig,
    shutdown: &crate::surfaces::http::server::ShutdownHandle,
    replica_thread: Option<std::thread::JoinHandle<Result<netbat::TcpServeStats, TexoError>>>,
) -> Result<Option<netbat::TcpServeStats>, TexoError> {
    let http_result = crate::surfaces::http::server::serve_listener(listener, config, shutdown);
    shutdown.shutdown();
    let replica_result = replica_thread
        .map(|thread| {
            thread.join().map_err(|_| TexoError::Surface {
                which: crate::error::SurfaceKind::Http,
                detail: "replica listener thread terminated without a result".to_string(),
            })?
        })
        .transpose()?;
    let _http_stats = http_result?;
    Ok(replica_result)
}

fn report_serve(replica_stats: Option<netbat::TcpServeStats>) -> ExitCode {
    if let Some(stats) = replica_stats {
        tracing::debug!(
            accepted = stats.accepted_connections,
            served = stats.served_requests,
            failed = stats.failed_requests,
            "replica listener stopped"
        );
    }
    ExitCode::SUCCESS
}

fn refresh_selected_reader(
    root: &Path,
    workspace: Option<&str>,
    journal: Option<&str>,
) -> Result<(), TexoError> {
    let config_path = root.join(".texo/config.toml");
    if !config_path.exists() {
        return Ok(());
    }
    let config =
        crate::config::TexoRootConfig::load(&config_path).map_err(|error| TexoError::Config {
            detail: error.to_string(),
            source: Some(Box::new(error)),
        })?;
    let (workspace_config, selected) =
        config
            .resolve_journal(workspace, journal)
            .map_err(|error| TexoError::Config {
                detail: error.to_string(),
                source: Some(Box::new(error)),
            })?;
    let _refresh = crate::replication::refresh_reader(
        root,
        Some(&workspace_config.workspace_id),
        selected.id.as_str(),
    )?;
    Ok(())
}

fn start_replica_server(
    addr: Option<String>,
    token_env: &str,
    journal: &crate::topology::ResolvedJournal,
    store: &crate::journal_store::JournalStore,
    workspace: &str,
    shutdown: &crate::surfaces::http::server::ShutdownHandle,
) -> Result<Option<std::thread::JoinHandle<Result<netbat::TcpServeStats, TexoError>>>, TexoError> {
    let Some(addr) = addr.or_else(|| std::env::var("TEXO_REPLICA_ADDR").ok()) else {
        return Ok(None);
    };
    let writer = store.writable_arc().ok_or_else(|| TexoError::Config {
        detail: "replica source listener requires a canonical journal".to_string(),
        source: None,
    })?;
    let token = std::env::var(token_env).map_err(|_| TexoError::Config {
        detail: format!("replica token environment variable `{token_env}` is not set"),
        source: None,
    })?;
    if token.is_empty() {
        return Err(TexoError::Config {
            detail: format!("replica token environment variable `{token_env}` is empty"),
            source: None,
        });
    }
    let listener = TcpListener::bind(&addr).map_err(|error| TexoError::Surface {
        which: crate::error::SurfaceKind::Http,
        detail: format!("replica listener {addr}: {error}"),
    })?;
    let server = crate::replica_net::Server {
        listener,
        store: writer,
        workspace_id: workspace.to_string(),
        journal_id: journal.id.to_string(),
        token,
    };
    let replica_shutdown = shutdown.clone();
    std::thread::Builder::new()
        .name("texo-replica-netbat".to_string())
        .spawn(move || crate::replica_net::serve(&server, &replica_shutdown))
        .map(Some)
        .map_err(|error| TexoError::Surface {
            which: crate::error::SurfaceKind::Http,
            detail: format!("start replica listener: {error}"),
        })
}

fn extract(path: &Path) -> ExitCode {
    match extract_impl(path) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _rendered = render::extract_error(error.as_ref());
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

fn open_host(
    root: &Path,
    workspace: Option<&str>,
    journal: Option<&str>,
) -> Result<TexoHost, TexoError> {
    let workspace = resolve_workspace(root, workspace)?;
    if let Some(journal) = journal {
        TexoHost::open_journal(root.to_path_buf(), workspace, journal, observed_at_ms())
    } else {
        TexoHost::open(root.to_path_buf(), workspace, observed_at_ms())
    }
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
