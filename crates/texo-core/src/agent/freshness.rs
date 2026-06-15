//! Agent freshness metadata.

use serde::{Deserialize, Serialize};

/// Freshness frontier description for agent surfaces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FreshnessView {
    /// Frontier kind label.
    pub kind: String,
    /// Human-readable description.
    pub description: String,
}

impl FreshnessView {
    /// Build the standard local BatPak frontier view.
    pub fn batpak_local(frontier: u64) -> Self {
        Self {
            kind: "batpak-local-frontier".to_string(),
            description: format!(
                "Projection replayed through local store sequence {frontier}. \
                 No global order or consensus is claimed."
            ),
        }
    }
}
