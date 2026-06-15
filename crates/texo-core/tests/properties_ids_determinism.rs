//! PROVES: INV-ID-DETERMINISTIC — same inputs yield stable domain ids.

use proptest::prelude::*;

use texo_core::types::ids::{claim_id_from_parts, source_id_from_hash};

fn arb_normalized_line() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("deploys happen on friday".to_string()),
        Just("alice owns release approval".to_string()),
        Just("decision deploys moved to tuesday".to_string()),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn claim_id_is_stable(line in arb_normalized_line(), line_no in 1u32..20u32) {
        let source_id = source_id_from_hash("fixturehash0001");
        let a = claim_id_from_parts(&source_id, line_no, &line);
        let b = claim_id_from_parts(&source_id, line_no, &line);
        prop_assert_eq!(a, b);
    }

    #[test]
    fn source_id_is_stable(hash in prop::sample::select(vec![
        "abcd1234efgh".to_string(),
        "000000000000".to_string(),
        "ffffffffffff".to_string(),
    ])) {
        let a = source_id_from_hash(&hash);
        let b = source_id_from_hash(&hash);
        prop_assert_eq!(a, b);
    }
}
