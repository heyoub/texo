//! Composed workspace and agent-integration diagnostics.

use std::path::Path;

use serde::Serialize;
use serde_json::json;

use crate::config::{TexoRootConfig, WorkspaceConfig};
use crate::gateway::{ModelRole, RoleOverrides};

/// One diagnostic check state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    /// Check passed.
    Pass,
    /// Product remains usable, but operator attention is recommended.
    Warn,
    /// A core workspace contract is unavailable or invalid.
    Fail,
}

/// One stable diagnostic row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DoctorCheck {
    /// Stable check identifier.
    pub id: &'static str,
    /// Check state.
    pub status: CheckStatus,
    /// Sanitized evidence.
    pub detail: String,
    /// Concrete repair command when one is available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

/// Aggregate doctor state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DoctorStatus {
    /// Every required and advisory check passed.
    Healthy,
    /// Core is usable, but an advisory integration check needs attention.
    Degraded,
    /// A required config/store/verification check failed.
    Broken,
}

/// Stable machine-readable doctor report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DoctorReport {
    /// Report schema.
    pub schema: &'static str,
    /// Aggregate state.
    pub status: DoctorStatus,
    /// Workspace selected for checks.
    pub workspace_id: String,
    /// Whether journal verification was requested.
    pub deep: bool,
    /// Whether safe managed-file repair was requested.
    pub fix_requested: bool,
    /// Ordered diagnostic evidence.
    pub checks: Vec<DoctorCheck>,
}

/// Diagnose one workspace without mutating source truth.
///
/// `fix` is deliberately narrow: it only reconciles files owned by
/// [`crate::install`]. It never edits journal data, user adapter entries, or
/// model credentials.
#[must_use]
pub fn diagnose(
    root: &Path,
    requested_workspace: Option<&str>,
    deep: bool,
    fix: bool,
) -> DoctorReport {
    let config_path = root.join(".texo/config.toml");
    let loaded = TexoRootConfig::load(&config_path);
    let workspace_id = requested_workspace.map_or_else(
        || {
            loaded.as_ref().map_or_else(
                |_| "demo".to_string(),
                |config| config.default_workspace.clone(),
            )
        },
        str::to_string,
    );
    let mut checks = Vec::new();

    if fix {
        match crate::install::install(root, &workspace_id, &[], false) {
            Ok(report) => {
                let changed = report
                    .changes
                    .iter()
                    .filter(|change| change.action != crate::install::ChangeAction::Unchanged)
                    .count();
                checks.push(pass(
                    "repair.managed",
                    format!("reconciled {changed} managed paths"),
                ));
            }
            Err(error) => checks.push(warn(
                "repair.managed",
                format!("safe repair declined: {error}"),
                "resolve the reported conflict, then run `texo doctor --fix`",
            )),
        }
    }

    let config = match TexoRootConfig::load(&config_path) {
        Ok(config) => {
            checks.push(pass(
                "config.parse",
                format!("loaded {}", config_path.display()),
            ));
            match config.resolve(Some(&workspace_id)) {
                Ok(workspace) => Some(workspace),
                Err(error) => {
                    checks.push(fail(
                        "config.workspace",
                        error.to_string(),
                        format!("texo init --workspace {workspace_id}"),
                    ));
                    None
                }
            }
        }
        Err(error) => {
            checks.push(fail(
                "config.parse",
                error.to_string(),
                format!("texo init --workspace {workspace_id}"),
            ));
            None
        }
    };

    check_integrations(root, &workspace_id, &mut checks);
    check_gateway(config.as_ref(), &mut checks);
    if let Some(workspace) = config {
        check_extractor(&workspace, &mut checks);
        check_workspace(root, &workspace, deep, &mut checks);
    }

    let status = if checks.iter().any(|check| check.status == CheckStatus::Fail) {
        DoctorStatus::Broken
    } else if checks.iter().any(|check| check.status == CheckStatus::Warn) {
        DoctorStatus::Degraded
    } else {
        DoctorStatus::Healthy
    };
    DoctorReport {
        schema: "texo.doctor.v1",
        status,
        workspace_id,
        deep,
        fix_requested: fix,
        checks,
    }
}

fn check_extractor(workspace: &WorkspaceConfig, checks: &mut Vec<DoctorCheck>) {
    if workspace.extractor_cmd.is_none() {
        checks.push(pass(
            "extractor.boundary",
            "built-in extractor requires no external execution boundary",
        ));
        return;
    }
    match crate::compat::bvisor::readiness() {
        Ok(detail) => checks.push(pass("extractor.boundary", detail)),
        Err(detail) => checks.push(fail(
            "extractor.boundary",
            detail,
            "install texo-bvisor-extractor and bvisor-linux-launcher, then set BVISOR_LAUNCHER_BIN",
        )),
    }
}

fn check_integrations(root: &Path, workspace_id: &str, checks: &mut Vec<DoctorCheck>) {
    match crate::install::install(root, workspace_id, &[], true) {
        Ok(report) => {
            let drift = report
                .changes
                .iter()
                .filter(|change| change.action != crate::install::ChangeAction::Unchanged)
                .map(|change| change.path.as_str())
                .collect::<Vec<_>>();
            if drift.is_empty() {
                checks.push(pass("integration.managed", "managed files match"));
            } else {
                checks.push(warn(
                    "integration.managed",
                    format!("drift: {}", drift.join(", ")),
                    "texo doctor --fix",
                ));
            }
        }
        Err(error) => checks.push(warn(
            "integration.managed",
            error.to_string(),
            "resolve the adapter conflict; Texo will not overwrite user configuration",
        )),
    }
}

fn check_gateway(config: Option<&WorkspaceConfig>, checks: &mut Vec<DoctorCheck>) {
    let gateway = config.and_then(|workspace| workspace.gateway.as_ref());
    let enabled = [
        ModelRole::Embed,
        ModelRole::Propose,
        ModelRole::Relate,
        ModelRole::Chat,
    ]
    .into_iter()
    .filter(|role| {
        crate::gateway::resolve_role(*role, &RoleOverrides::default(), gateway).is_enabled()
    })
    .map(|role| format!("{role:?}").to_lowercase())
    .collect::<Vec<_>>();
    let detail = if enabled.is_empty() {
        "heuristic-only mode; no model key resolved".to_string()
    } else {
        format!("model key resolved for roles: {}", enabled.join(", "))
    };
    checks.push(pass("gateway.readiness", detail));
}

fn check_workspace(
    root: &Path,
    workspace: &WorkspaceConfig,
    deep: bool,
    checks: &mut Vec<DoctorCheck>,
) {
    let store_path = workspace.store_path_buf(root);
    if !store_path.is_dir() {
        checks.push(fail(
            "store.open",
            format!("store is missing at {}", store_path.display()),
            format!("texo init --workspace {}", workspace.workspace_id),
        ));
        return;
    }
    let mut host = match crate::host::TexoHost::open(root, &workspace.workspace_id, 0) {
        Ok(host) => host,
        Err(error) => {
            checks.push(fail(
                "store.open",
                error.to_string(),
                "close competing writers, then run `texo doctor --deep`",
            ));
            return;
        }
    };
    checks.push(pass(
        "store.open",
        format!("opened {}", store_path.display()),
    ));
    match host.invoke_json("texo.workspace.status", &json!({})) {
        Ok(status) => checks.push(pass(
            "workspace.projection",
            format!(
                "freshness={}, frontier={}, settlement_complete={}",
                status["freshness"].as_str().unwrap_or("unknown"),
                status["frontier"].as_u64().unwrap_or(0),
                status["settlement_complete"].as_bool().unwrap_or(false)
            ),
        )),
        Err(error) => checks.push(fail(
            "workspace.projection",
            error.to_string(),
            "texo verify",
        )),
    }
    if deep {
        match host.invoke_json("texo.verify.run", &json!({})) {
            Ok(output)
                if output["journal_ok"].as_bool() == Some(true)
                    && output["projection_ok"].as_bool() == Some(true)
                    && output["transitions_ok"].as_bool() == Some(true) =>
            {
                checks.push(pass(
                    "workspace.verify",
                    "journal, projection, and transitions verified",
                ));
            }
            Ok(output) => checks.push(fail(
                "workspace.verify",
                format!("verification failed: {}", output["errors"]),
                "texo verify --json",
            )),
            Err(error) => checks.push(fail(
                "workspace.verify",
                error.to_string(),
                "texo verify --json",
            )),
        }
    }
}

fn pass(id: &'static str, detail: impl Into<String>) -> DoctorCheck {
    DoctorCheck {
        id,
        status: CheckStatus::Pass,
        detail: detail.into(),
        fix: None,
    }
}

fn warn(id: &'static str, detail: impl Into<String>, fix: impl Into<String>) -> DoctorCheck {
    DoctorCheck {
        id,
        status: CheckStatus::Warn,
        detail: detail.into(),
        fix: Some(fix.into()),
    }
}

fn fail(id: &'static str, detail: impl Into<String>, fix: impl Into<String>) -> DoctorCheck {
    DoctorCheck {
        id,
        status: CheckStatus::Fail,
        detail: detail.into(),
        fix: Some(fix.into()),
    }
}
