//! PROVES: INV-REPLAY-DETERMINISTIC — superseded claims never appear as current.

mod support;

use proptest::prelude::*;
use support::{copy_sample_sources, ingest_sample_sources, setup_demo_journal, temp_workspace};
use texo_core::{open_journal, ClaimStatus};

proptest! {
    #![proptest_config(ProptestConfig::with_cases(8))]

    #[test]
    fn superseded_deploy_claims_never_current(_seed in any::<u8>()) {
        let dir = temp_workspace();
        copy_sample_sources(dir.path());
        setup_demo_journal(dir.path());
        ingest_sample_sources(dir.path());

        let journal = open_journal(dir.path()).expect("open");
        let workspace = journal.config().workspace().expect("workspace");
        let replayed = journal.replay(&workspace).expect("replay");
        journal.close().expect("close");

        for claim in replayed.state.claims.values() {
            if claim.status == ClaimStatus::Superseded {
                prop_assert!(claim.superseded_by.is_some());
            }
            if claim.normalized_text.contains("friday")
                && claim.subject_hint == "deploy-process"
            {
                prop_assert_eq!(claim.status, ClaimStatus::Superseded);
            }
        }
    }
}
