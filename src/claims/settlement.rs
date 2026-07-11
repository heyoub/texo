//! Durable semantic-settlement projection.

use batpak::event::RawMsgpackInput;
use serde::{Deserialize, Serialize};

use crate::events::payloads::{RelationDeferredV1, RelationJudgedV1};
use crate::relate::settlement::{RelationFailureClass, SettledRelation};

/// One projected judgment row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JudgmentRecord {
    /// Verdict.
    pub relation: SettledRelation,
    /// Confidence in parts per million.
    pub score_ppm: u32,
    /// Attempt provenance.
    pub judge_fingerprint: String,
    /// Paid-cache key.
    pub cache_key_hex: String,
    /// Observation timestamp.
    pub observed_at_ms: u64,
}

/// One projected deferral row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeferralRecord {
    /// Failure class.
    pub failure_class: RelationFailureClass,
    /// Provider attempt count.
    pub attempts: u32,
    /// Observation timestamp.
    pub observed_at_ms: u64,
}

/// Settlement history for one provider-neutral logical pair.
///
/// The first judgment in journal commit order is authoritative forever. Later
/// judgments remain visible provenance but never change derived authority.
#[derive(Default, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, batpak::EventSourced)]
#[batpak(input = RawMsgpackInput, cache_version = 1, state_max_cardinality = 1)]
#[batpak(event = RelationJudgedV1, handler = on_judged)]
#[batpak(event = RelationDeferredV1, handler = on_deferred)]
pub struct SettlementCard {
    /// Workspace id.
    pub workspace_id: String,
    /// Older claim id.
    pub older_claim: String,
    /// Newer claim id.
    pub newer_claim: String,
    /// First judgment in commit order.
    pub authoritative: Option<JudgmentRecord>,
    /// Later non-authoritative judgments in commit order.
    pub later_judgments: Vec<JudgmentRecord>,
    /// Failed attempts in commit order.
    pub deferrals: Vec<DeferralRecord>,
}

impl SettlementCard {
    fn on_judged(&mut self, event: &RelationJudgedV1) {
        self.workspace_id = event.workspace_id.to_string();
        self.older_claim = event.older_claim.to_string();
        self.newer_claim = event.newer_claim.to_string();
        let record = JudgmentRecord {
            relation: event.relation,
            score_ppm: event.score_ppm,
            judge_fingerprint: event.judge_fingerprint.clone(),
            cache_key_hex: event.cache_key_hex.clone(),
            observed_at_ms: event.observed_at_ms,
        };
        if self.authoritative.is_none() {
            self.authoritative = Some(record);
        } else {
            self.later_judgments.push(record);
        }
    }

    fn on_deferred(&mut self, event: &RelationDeferredV1) {
        self.workspace_id = event.workspace_id.to_string();
        self.older_claim = event.older_claim.to_string();
        self.newer_claim = event.newer_claim.to_string();
        self.deferrals.push(DeferralRecord {
            failure_class: event.failure_class,
            attempts: event.attempts,
            observed_at_ms: event.observed_at_ms,
        });
    }
}

#[cfg(test)]
mod tests {
    use batpak::store::{Freshness, Store, StoreConfig};

    use super::*;
    use crate::events::coordinate::coordinate_for_relation_pair;
    use crate::events::ids::{relation_pair_id, ClaimId, WorkspaceId};

    #[test]
    fn first_judgment_is_authoritative_and_later_judgments_are_history() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(StoreConfig::new(dir.path())).expect("store");
        let workspace_id = WorkspaceId::new("demo").expect("workspace");
        let older = ClaimId::try_from("claim_aaaaaaaaaaaa").expect("older");
        let newer = ClaimId::try_from("claim_bbbbbbbbbbbb").expect("newer");
        let pair_id = relation_pair_id(&workspace_id, &older, &newer);
        let coordinate = coordinate_for_relation_pair(workspace_id.as_str(), pair_id.as_str())
            .expect("coordinate");
        let judgment = |relation, fingerprint: &str| RelationJudgedV1 {
            workspace_id: workspace_id.clone(),
            older_claim: older.clone(),
            newer_claim: newer.clone(),
            relation,
            score_ppm: 800_000,
            judge_fingerprint: fingerprint.to_string(),
            cache_key_hex: fingerprint.to_string(),
            observed_at_ms: 1,
        };
        let _ = store
            .append_typed(&coordinate, &judgment(SettledRelation::Supersedes, "first"))
            .expect("first");
        let _ = store
            .append_typed(&coordinate, &judgment(SettledRelation::Conflicts, "second"))
            .expect("second");

        let card = store
            .project::<SettlementCard>(
                &crate::events::coordinate::entity_for_relation_pair(pair_id.as_str()),
                &Freshness::Consistent,
            )
            .expect("project")
            .expect("card");
        assert_eq!(
            card.authoritative.as_ref().map(|row| row.relation),
            Some(SettledRelation::Supersedes)
        );
        assert_eq!(card.later_judgments.len(), 1);
        assert_eq!(card.later_judgments[0].relation, SettledRelation::Conflicts);
    }
}
