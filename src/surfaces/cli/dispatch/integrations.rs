//! Installation, hook, and diagnostic commands.

use std::io::Read;
use std::process::ExitCode;

use serde_json::json;

use crate::error::TexoError;
use crate::install::ClientTarget;

use super::super::{open_host, render, DispatchContext, HookCmd};

pub(super) fn install(
    cli: &DispatchContext,
    client: &[ClientTarget],
    dry_run: bool,
    json_output: bool,
) -> Result<ExitCode, TexoError> {
    let workspace = cli.workspace.as_deref().unwrap_or("demo");
    let report = crate::install::install_for_journal(
        &cli.root,
        workspace,
        cli.journal.as_deref(),
        client,
        dry_run,
    )?;
    let output = serde_json::to_value(report)?;
    if json_output {
        render::json(&output)?;
    } else {
        render::installation(&output)?;
    }
    Ok(ExitCode::SUCCESS)
}

pub(super) fn uninstall(
    cli: &DispatchContext,
    client: &[ClientTarget],
    dry_run: bool,
    json_output: bool,
) -> Result<ExitCode, TexoError> {
    let report = crate::install::uninstall(&cli.root, client, dry_run)?;
    let output = serde_json::to_value(report)?;
    if json_output {
        render::json(&output)?;
    } else {
        render::installation(&output)?;
    }
    Ok(ExitCode::SUCCESS)
}

pub(super) fn hook(cli: &DispatchContext, command: &HookCmd) -> Result<ExitCode, TexoError> {
    let mut host = open_host(&cli.root, cli.workspace.as_deref(), cli.journal.as_deref())?;
    let (event, data, json_output) = match command {
        HookCmd::SessionStart { json } => (
            "session_start",
            host.invoke_json(
                "texo.context.agent",
                &json!({
                    "subject": null,
                    "include_stale": true,
                    "allow_unsettled": true
                }),
            )?,
            *json,
        ),
        HookCmd::FilesChanged { json } => {
            let mut bytes = Vec::new();
            std::io::stdin()
                .take((crate::hooks::MAX_INPUT_BYTES + 1) as u64)
                .read_to_end(&mut bytes)?;
            let input = crate::hooks::parse_files_changed(&bytes)?;
            let mut reports = Vec::with_capacity(input.paths.len());
            for path in input.paths {
                reports.push(host.invoke_json("texo.staleness.check", &json!({"path": path}))?);
            }
            ("files_changed", json!({"reports": reports}), *json)
        }
        HookCmd::PreCommit { json } => (
            "pre_commit",
            host.invoke_json("texo.verify.run", &json!({}))?,
            *json,
        ),
    };
    let output = json!({
        "schema": "texo.hook-result.v1",
        "event": event,
        "advisory": true,
        "data": data
    });
    let _ = json_output;
    render::json(&output)?;
    Ok(ExitCode::SUCCESS)
}

pub(super) fn doctor(
    cli: &DispatchContext,
    deep: bool,
    fix: bool,
    json_output: bool,
) -> Result<ExitCode, TexoError> {
    let report = crate::doctor::diagnose(&cli.root, cli.workspace.as_deref(), deep, fix);
    let broken = report.status == crate::doctor::DoctorStatus::Broken;
    let output = serde_json::to_value(report)?;
    if json_output {
        render::json(&output)?;
    } else {
        render::doctor(&output)?;
    }
    Ok(if broken {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}
