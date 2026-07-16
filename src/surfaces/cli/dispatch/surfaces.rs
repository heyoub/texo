//! Host, protocol, and discovery surface commands.

use std::path::Path;
use std::process::ExitCode;

use serde_json::{json, Value};

use crate::error::TexoError;

use super::super::{
    extract as run_extract, open_host, refresh_selected_reader, render, serve as run_server,
    DispatchContext, HostCmd, OpsCmd, ServeOptions, SessionCmd,
};

pub(super) fn host(cli: &DispatchContext, command: &HostCmd) -> Result<ExitCode, TexoError> {
    match command {
        HostCmd::Fingerprint => {
            let mut host = open_host(&cli.root, cli.workspace.as_deref(), cli.journal.as_deref())?;
            let output = host.invoke_json("texo.host.fingerprint", &json!({}))?;
            render::json(&output)?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

pub(super) fn mcp(cli: &DispatchContext) -> Result<ExitCode, TexoError> {
    refresh_selected_reader(&cli.root, cli.workspace.as_deref(), cli.journal.as_deref())?;
    crate::surfaces::mcp_stdio::run(&cli.root, cli.workspace.as_deref(), cli.journal.as_deref())?;
    Ok(ExitCode::SUCCESS)
}

pub(super) fn serve(cli: &DispatchContext, options: ServeOptions) -> Result<ExitCode, TexoError> {
    run_server(options, &cli.root, cli.workspace.as_deref())
}

pub(super) fn extract(path: &Path) -> ExitCode {
    run_extract(path)
}

pub(super) fn session(cli: &DispatchContext, command: SessionCmd) -> Result<ExitCode, TexoError> {
    match command {
        SessionCmd::Export { session_id } => {
            let mut host = open_host(&cli.root, cli.workspace.as_deref(), cli.journal.as_deref())?;
            let output =
                host.invoke_json("texo.session.export", &json!({"session_id": session_id}))?;
            render::session_markdown(
                output
                    .get("markdown")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

pub(super) fn ops(command: OpsCmd) -> Result<ExitCode, TexoError> {
    let inventory = crate::agent_catalog::operation_inventory();
    match command {
        OpsCmd::List { json } => {
            if json {
                render::json(&inventory)?;
            } else {
                render::operations(&inventory)?;
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
                render::operations(&json!({"operations": [operation]}))?;
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}
