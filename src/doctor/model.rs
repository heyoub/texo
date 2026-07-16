//! Stable diagnostic report shapes.

use serde::Serialize;

/// One stable diagnostic row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DoctorCheck {
    /// Stable check identifier.
    pub id: &'static str,
    /// Check state.
    pub status: super::CheckStatus,
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
