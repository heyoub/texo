//! Backup and replication commands.

use std::process::ExitCode;

use crate::config::TexoRootConfig;
use crate::error::TexoError;

use super::super::{
    follow_replica_until_shutdown, observed_at_ms, render, BackupCmd, DispatchContext, ReplicaCmd,
};

pub(super) fn backup(cli: &DispatchContext, command: BackupCmd) -> Result<ExitCode, TexoError> {
    match command {
        BackupCmd::Create { dest, json } => {
            let config_path = cli.root.join(".texo/config.toml");
            let root_config =
                TexoRootConfig::load(&config_path).map_err(|error| TexoError::Config {
                    detail: error.to_string(),
                    source: Some(Box::new(error)),
                })?;
            let workspace = root_config
                .resolve(cli.workspace.as_deref())
                .map_err(|error| TexoError::Config {
                    detail: error.to_string(),
                    source: Some(Box::new(error)),
                })?;
            let store = crate::host::open_workspace_store(&cli.root, &workspace.workspace_id)?;
            let report = crate::backup::create(
                &cli.root,
                &workspace,
                store.as_ref(),
                &dest,
                observed_at_ms(),
            )?;
            let output = serde_json::to_value(report)?;
            if json {
                render::json(&output)?;
            } else {
                render::backup(&output)?;
            }
            Ok(ExitCode::SUCCESS)
        }
        BackupCmd::Verify {
            dest,
            expect_manifest_hash,
            json,
        } => {
            let report = crate::backup::verify_with_expected_manifest_hash(
                &dest,
                expect_manifest_hash.as_deref(),
            )?;
            let verified = report.verified;
            let output = serde_json::to_value(report)?;
            if json {
                render::json(&output)?;
            } else {
                render::backup(&output)?;
            }
            Ok(if verified {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            })
        }
        BackupCmd::Restore {
            source,
            expect_manifest_hash,
            json,
        } => {
            let report =
                crate::backup::restore(&source, &cli.root, expect_manifest_hash.as_deref())?;
            let output = serde_json::to_value(report)?;
            if json {
                render::json(&output)?;
            } else {
                render::backup(&output)?;
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

pub(super) fn replica(cli: &DispatchContext, command: ReplicaCmd) -> Result<ExitCode, TexoError> {
    let report = match command {
        ReplicaCmd::Bootstrap { replica, json } => {
            let _ = json;
            crate::replication::bootstrap(&cli.root, cli.workspace.as_deref(), &replica)?
        }
        ReplicaCmd::Follow {
            replica,
            json,
            watch: false,
            interval_ms,
        } => {
            let _ = (json, interval_ms);
            crate::replication::follow_once(&cli.root, cli.workspace.as_deref(), &replica)?
        }
        ReplicaCmd::Follow {
            replica,
            json,
            watch: true,
            interval_ms,
        } => {
            let _ = json;
            return follow_replica_until_shutdown(
                &cli.root,
                cli.workspace.as_deref(),
                &replica,
                interval_ms,
            );
        }
    };
    render::json(&serde_json::to_value(report)?)?;
    Ok(ExitCode::SUCCESS)
}
