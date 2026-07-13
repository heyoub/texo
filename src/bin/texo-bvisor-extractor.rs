//! Standalone bvisor process for configured extractor confinement.
//!
//! This binary intentionally does not link the Texo library: bvisor 0.10's
//! payload inventory currently collides with downstream domain allocations.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use bvisor::{
    run_confined, BoundarySpec, BudgetRequest, BudgetRequirements, Capability, EnvEntry, EnvPolicy,
    EvidenceRequirements, FdPolicy, FsAccess, FsConfinement, HostControl, LinuxBackend,
    MinGuarantee, NetPolicy, Outcome, PathSet, Workload,
};

const MAX_OUTPUT_BYTES: u64 = 16 * 1024 * 1024;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ignored = writeln!(std::io::stderr(), "bvisor extractor: {error}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<(), String> {
    let args = Args::parse()?;
    let launcher = launcher_path()?;
    validate_paths(&args.input, &args.output)?;
    let parent = args
        .output
        .parent()
        .ok_or_else(|| "output has no parent directory".to_string())?;
    let success = parent.join("bvisor-success");
    if success.exists() {
        return Err("stale bvisor completion marker exists".to_string());
    }
    let shell = format!(
        "cd \"$1\" && if {{ {} \"$2\"; }} > \"$3\"; then : > \"$4\"; fi",
        args.cmd
    );
    let spec = BoundarySpec {
        workload: Workload::Process {
            exe: "/bin/sh".to_string(),
            args: vec![
                "-c".to_string(),
                shell,
                "texo-extract".to_string(),
                parent.display().to_string(),
                args.input.display().to_string(),
                args.output.display().to_string(),
                success.display().to_string(),
            ],
        },
        capabilities: vec![
            filesystem_capability(FsAccess::ReadWrite, vec![parent.display().to_string()]),
            filesystem_capability(FsAccess::Read, runtime_roots()),
            filesystem_capability(FsAccess::ReadWrite, device_roots()),
            Capability::Network {
                policy: NetPolicy::DenyAll,
            },
            Capability::Environment {
                policy: EnvPolicy::Exact(vec![
                    EnvEntry::literal("PATH", "/usr/bin:/bin"),
                    EnvEntry::literal("LANG", "C.UTF-8"),
                ]),
            },
            Capability::InheritedFds {
                policy: FdPolicy::None,
            },
        ],
        controls: vec![HostControl::LaunchWorkload],
        budgets: extractor_budgets(),
        evidence: EvidenceRequirements {
            require_captured_streams: false,
            require_exit_status: true,
        },
    };
    let report = run_confined(&spec, Arc::new(LinuxBackend::with_launcher_path(launcher)))
        .map_err(|error| format!("admission refused: {error}"))?;
    if report.body.outcome != Outcome::Completed || !success.is_file() {
        return Err(format!(
            "confined workload did not complete: outcome={:?}, marker={}",
            report.body.outcome,
            success.is_file()
        ));
    }
    let metadata = fs::symlink_metadata(&args.output).map_err(|error| error.to_string())?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_OUTPUT_BYTES {
        return Err(format!(
            "output must be a regular file no larger than {MAX_OUTPUT_BYTES} bytes"
        ));
    }
    File::open(&args.output)
        .and_then(|file| file.sync_all())
        .map_err(|error| error.to_string())
}

struct Args {
    cmd: String,
    input: PathBuf,
    output: PathBuf,
}

impl Args {
    fn parse() -> Result<Self, String> {
        let mut args = std::env::args_os().skip(1);
        let cmd_flag = args.next().ok_or_else(|| "missing --cmd".to_string())?;
        let cmd = args.next().ok_or_else(|| "missing command".to_string())?;
        let input_flag = args.next().ok_or_else(|| "missing --input".to_string())?;
        let input = args.next().ok_or_else(|| "missing input".to_string())?;
        let output_flag = args.next().ok_or_else(|| "missing --output".to_string())?;
        let output = args.next().ok_or_else(|| "missing output".to_string())?;
        if cmd_flag != "--cmd"
            || input_flag != "--input"
            || output_flag != "--output"
            || args.next().is_some()
        {
            return Err("expected --cmd <command> --input <path> --output <path>".to_string());
        }
        let cmd = cmd
            .into_string()
            .map_err(|_| "command must be UTF-8".to_string())?;
        if cmd.trim().is_empty() {
            return Err("command must not be empty".to_string());
        }
        Ok(Self {
            cmd,
            input: PathBuf::from(input),
            output: PathBuf::from(output),
        })
    }
}

fn validate_paths(input: &Path, output: &Path) -> Result<(), String> {
    if !input.is_absolute() || !output.is_absolute() {
        return Err("input and output paths must be absolute".to_string());
    }
    let input_parent = input
        .parent()
        .ok_or_else(|| "input has no parent directory".to_string())?;
    let output_parent = output
        .parent()
        .ok_or_else(|| "output has no parent directory".to_string())?;
    if input_parent != output_parent || input.file_name().is_none_or(|name| name != "input.md") {
        return Err("input and output must use the same private staging directory".to_string());
    }
    if output
        .file_name()
        .is_none_or(|name| name != "claims.ndjson")
        || output.exists()
    {
        return Err("output must be a fresh claims.ndjson path".to_string());
    }
    let parent = fs::symlink_metadata(input_parent).map_err(|error| error.to_string())?;
    let source = fs::symlink_metadata(input).map_err(|error| error.to_string())?;
    if !parent.file_type().is_dir() || !source.file_type().is_file() {
        return Err("staging paths must be regular and symlink-free".to_string());
    }
    Ok(())
}

fn launcher_path() -> Result<PathBuf, String> {
    let value = std::env::var_os("BVISOR_LAUNCHER_BIN")
        .ok_or_else(|| "BVISOR_LAUNCHER_BIN is not set".to_string())?;
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err("BVISOR_LAUNCHER_BIN must be absolute".to_string());
    }
    let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
    if !metadata.file_type().is_file() {
        return Err("launcher must be a regular file, not a symlink".to_string());
    }
    Ok(path)
}

fn filesystem_capability(access: FsAccess, roots: Vec<String>) -> Capability {
    Capability::Filesystem {
        access,
        scope: PathSet { roots },
        recursive: true,
        confinement: FsConfinement::DeclaredRootsOnly,
    }
}

fn runtime_roots() -> Vec<String> {
    [
        Path::new("/bin"),
        Path::new("/usr/bin"),
        Path::new("/lib"),
        Path::new("/lib64"),
        Path::new("/usr/lib"),
        Path::new("/usr/lib64"),
    ]
    .into_iter()
    .filter(|path| path.exists())
    .map(|path| path.display().to_string())
    .collect()
}

fn device_roots() -> Vec<String> {
    [Path::new("/dev/null")]
        .into_iter()
        .filter(|path| path.exists())
        .map(|path| path.display().to_string())
        .collect()
}

fn extractor_budgets() -> BudgetRequirements {
    let mediated = |limit| BudgetRequest::uniform(limit, MinGuarantee::Mediated);
    BudgetRequirements {
        wall_micros: mediated(900_000_000),
        cpu_micros: mediated(900_000_000),
        resident_bytes: mediated(512 * 1024 * 1024),
        process_count: mediated(64),
        handle_count: mediated(256),
        storage_bytes: mediated(MAX_OUTPUT_BYTES * 2),
        network_bytes: BudgetRequest::deny_all(),
    }
}
