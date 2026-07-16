use serde::{Deserialize, Serialize};

use crate::events::payloads::RelationCampaignCheckpointV1;

/// Latest durable progress proof for one workspace relation campaign.
#[derive(Default, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, batpak::EventSourced)]
#[batpak(input = batpak::event::RawMsgpackInput, cache_version = 1, state_max_cardinality = 1)]
#[batpak(event = RelationCampaignCheckpointV1, handler = on_checkpoint)]
pub struct CampaignCard {
    /// Last checkpoint in journal commit order.
    pub latest: Option<RelationCampaignCheckpointV1>,
}

impl CampaignCard {
    fn on_checkpoint(&mut self, event: &RelationCampaignCheckpointV1) {
        self.latest = Some(event.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::ids::WorkspaceId;
    use crate::relate::settlement::CampaignPhase;

    fn checkpoint(phase: CampaignPhase, observed_at_ms: u64) -> RelationCampaignCheckpointV1 {
        RelationCampaignCheckpointV1 {
            workspace_id: WorkspaceId::try_from("workspace").expect("valid workspace"),
            evaluated_basis_digest_hex: "a".repeat(64),
            result_basis_digest_hex: "a".repeat(64),
            candidate_policy_digest_hex: "b".repeat(64),
            phase,
            observed_at_ms,
        }
    }

    #[test]
    fn latest_checkpoint_wins_in_event_order() {
        let partial = checkpoint(
            CampaignPhase::Partial {
                next_candidate_cursor: 10,
            },
            20,
        );
        let complete = checkpoint(CampaignPhase::Complete, 10);
        let mut card = CampaignCard::default();
        card.on_checkpoint(&partial);
        card.on_checkpoint(&complete);
        assert_eq!(card.latest, Some(complete));
    }
}
