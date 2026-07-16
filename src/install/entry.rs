//! Public installation entry point.

use std::path::Path;

use crate::error::TexoError;

use super::{ClientTarget, InstallReport};

/// Install the lightweight Texo appliance.
///
/// # Errors
/// Returns an error when existing client configuration is malformed, already
/// owns a conflicting `texo` entry, or a managed write fails.
pub fn install(
    root: &Path,
    workspace_id: &str,
    requested: &[ClientTarget],
    dry_run: bool,
) -> Result<InstallReport, TexoError> {
    super::install_for_journal(root, workspace_id, None, requested, dry_run)
}
