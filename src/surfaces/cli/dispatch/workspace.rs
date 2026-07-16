//! Workspace, projection, and inspection commands.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::{json, Value};

use crate::error::TexoError;
use crate::host::TexoHost;

use super::super::{observed_at_ms, open_host, render, DispatchContext};

pub(super) fn init(cli: &DispatchContext, workspace: &str) -> Result<ExitCode, TexoError> {
    let mut host =
        TexoHost::open_for_init(cli.root.clone(), workspace.to_string(), observed_at_ms())?;
    let output = host.invoke_json("texo.workspace.init", &json!({ "workspace_id": workspace }))?;
    render::init(&cli.root, &output)?;
    Ok(ExitCode::SUCCESS)
}

pub(super) fn ingest(
    cli: &DispatchContext,
    path: &Path,
    dry_run: bool,
    strict: bool,
    json_output: bool,
) -> Result<ExitCode, TexoError> {
    let mut host = open_host(&cli.root, cli.workspace.as_deref(), cli.journal.as_deref())?;
    let output = host.invoke_json(
        "texo.ingest.run",
        &json!({
            "path": path,
            "dry_run": dry_run,
            "strict": strict,
            "observed_at_ms": observed_at_ms()
        }),
    )?;
    if json_output {
        render::json(&output)?;
    } else {
        render::ingest(&output)?;
    }
    Ok(partial_exit(&output))
}

pub(super) fn claims(
    cli: &DispatchContext,
    subject: Option<&str>,
    json_output: bool,
) -> Result<ExitCode, TexoError> {
    let mut host = open_host(&cli.root, cli.workspace.as_deref(), cli.journal.as_deref())?;
    let output = host.invoke_json("texo.claims.list", &json!({"subject": subject}))?;
    if json_output {
        let claims = output
            .get("claims")
            .cloned()
            .unwrap_or(Value::Array(Vec::new()));
        render::json(&claims)?;
    } else {
        render::claims(&output)?;
    }
    Ok(ExitCode::SUCCESS)
}

pub(super) fn supersede(
    cli: &DispatchContext,
    old: &str,
    new: &str,
    reason: &str,
    decided_by: &str,
    json_output: bool,
) -> Result<ExitCode, TexoError> {
    let mut host = open_host(&cli.root, cli.workspace.as_deref(), cli.journal.as_deref())?;
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
    if json_output {
        render::json(&output)?;
    } else {
        render::supersede(&output)?;
    }
    Ok(ExitCode::SUCCESS)
}

pub(super) fn check_staleness(
    cli: &DispatchContext,
    path: &Path,
    json_output: bool,
) -> Result<ExitCode, TexoError> {
    let mut host = open_host(&cli.root, cli.workspace.as_deref(), cli.journal.as_deref())?;
    let output = host.invoke_json("texo.staleness.check", &json!({"path": path}))?;
    let has_findings = output
        .get("diagnostics")
        .and_then(Value::as_array)
        .is_some_and(|diagnostics| !diagnostics.is_empty());
    if json_output {
        render::json(&output)?;
    } else {
        render::staleness(&output)?;
    }
    Ok(if has_findings {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

pub(super) fn agent_context(
    cli: &DispatchContext,
    subject: Option<&str>,
    out: Option<PathBuf>,
    json_output: bool,
    allow_unsettled: bool,
) -> Result<ExitCode, TexoError> {
    let mut host = open_host(&cli.root, cli.workspace.as_deref(), cli.journal.as_deref())?;
    let output = host.invoke_json(
        "texo.context.agent",
        &json!({
            "subject": subject,
            "include_stale": true,
            "allow_unsettled": allow_unsettled
        }),
    )?;
    let rendered = serde_json::to_string_pretty(&output)?;
    if let Some(path) = out {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, &rendered)?;
        if json_output {
            render::json(&output)?;
        }
    } else {
        let _ = json_output;
        render::json(&output)?;
    }
    Ok(ExitCode::SUCCESS)
}

pub(super) fn compile(
    cli: &DispatchContext,
    out: &Path,
    allow_unsettled: bool,
) -> Result<ExitCode, TexoError> {
    let mut host = open_host(&cli.root, cli.workspace.as_deref(), cli.journal.as_deref())?;
    let output = host.invoke_json(
        "texo.compile.run",
        &json!({
            "out_dir": out,
            "observed_at_ms": observed_at_ms(),
            "allow_unsettled": allow_unsettled
        }),
    )?;
    render::compile(out, &output)?;
    Ok(ExitCode::SUCCESS)
}

pub(super) fn conflicts(
    cli: &DispatchContext,
    json_output: bool,
    commit: bool,
) -> Result<ExitCode, TexoError> {
    let mut host = open_host(&cli.root, cli.workspace.as_deref(), cli.journal.as_deref())?;
    if commit {
        let output = host.invoke_json(
            "texo.conflicts.commit",
            &json!({"observed_at_ms": observed_at_ms()}),
        )?;
        if json_output {
            render::json(&output)?;
        } else {
            render::conflicts_committed(&output)?;
        }
    } else {
        let output = host.invoke_json("texo.conflicts.list", &json!({}))?;
        if json_output {
            render::json(&output)?;
        } else {
            render::conflicts(&output)?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

pub(super) fn verify(cli: &DispatchContext, json_output: bool) -> Result<ExitCode, TexoError> {
    let mut host = open_host(&cli.root, cli.workspace.as_deref(), cli.journal.as_deref())?;
    let output = host.invoke_json("texo.verify.run", &json!({}))?;
    let failed = ["projection_ok", "journal_ok", "transitions_ok"]
        .iter()
        .any(|key| output.get(*key).and_then(Value::as_bool) == Some(false));
    if json_output {
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
        render::verify(&output)?;
    }
    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

pub(super) fn stats(cli: &DispatchContext, json_output: bool) -> Result<ExitCode, TexoError> {
    let _ = json_output;
    let mut host = open_host(&cli.root, cli.workspace.as_deref(), cli.journal.as_deref())?;
    let output = host.invoke_json("texo.stats.read", &json!({}))?;
    render::json(&output)?;
    Ok(ExitCode::SUCCESS)
}

fn partial_exit(output: &Value) -> ExitCode {
    if output.get("outcome").and_then(Value::as_str) == Some("partial") {
        ExitCode::from(2)
    } else {
        ExitCode::SUCCESS
    }
}
