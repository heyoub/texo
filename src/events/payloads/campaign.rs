use serde::{Deserialize, Serialize};

use crate::events::ids::WorkspaceId;
use crate::relate::settlement::CampaignPhase;

/// Durable progress proof for one workspace relation campaign.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, batpak::EventPayload)]
#[batpak(category = 0xE, type_id = 18, version = 1)]
pub struct RelationCampaignCheckpointV1 {
    /// Workspace scope identifier.
    pub workspace_id: WorkspaceId,
    /// Claim basis evaluated by this page.
    pub evaluated_basis_digest_hex: String,
    /// Claim basis resulting after any complete-page authority publication.
    pub result_basis_digest_hex: String,
    /// Candidate-discovery policy identity, excluding credentials.
    pub candidate_policy_digest_hex: String,
    /// Closed partial-or-complete campaign state.
    pub phase: CampaignPhase,
    /// Observation wall-clock time in milliseconds; never an ordering input.
    pub observed_at_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_checkpoint_round_trips_canonically() {
        let checkpoint = RelationCampaignCheckpointV1 {
            workspace_id: WorkspaceId::try_from("workspace").expect("valid workspace"),
            evaluated_basis_digest_hex: "a".repeat(64),
            result_basis_digest_hex: "a".repeat(64),
            candidate_policy_digest_hex: "b".repeat(64),
            phase: CampaignPhase::Partial {
                next_candidate_cursor: 17,
            },
            observed_at_ms: 23,
        };
        let encoded = batpak::canonical::to_bytes(&checkpoint).expect("checkpoint encodes");
        let decoded = batpak::canonical::from_bytes(&encoded).expect("checkpoint decodes");
        assert_eq!(checkpoint, decoded);
    }
}
