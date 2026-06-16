//! PROVES: INV-ID-DETERMINISTIC + INV-ID-INJECTIVE — domain ids are stable for
//! equal inputs and (modulo hash collision, treated as impossible for distinct
//! short inputs) distinct for distinct inputs. Collision would break supersession
//! and conflict identity, so injectivity is load-bearing, not cosmetic.

mod common;

use common::proptest::config;
use proptest::prelude::*;

use texo_core::types::ids::{
    blake3_bytes_hex, claim_id_from_parts, conflict_id_from_pair, source_id_from_hash, ClaimId,
};

/// Distinct short tokens whose normalized forms differ; used to build distinct
/// claim/source inputs without relying on full free-text generation (which could
/// normalize to the same string and create spurious "collisions").
fn arb_token() -> impl Strategy<Value = String> {
    "[a-z]{1,12}"
}

proptest! {
    #![proptest_config(config())]

    // DETERMINISM: equal (source, line, text) always yields the same claim id.
    #[test]
    fn claim_id_is_stable(text in arb_token(), line in 1u32..5000u32) {
        let source_id = source_id_from_hash(&blake3_bytes_hex(b"fixture-body")).expect("source id");
        let a = claim_id_from_parts(&source_id, line, &text);
        let b = claim_id_from_parts(&source_id, line, &text);
        prop_assert_eq!(a, b);
    }

    // INJECTIVITY over text: at a fixed (source, line), two DIFFERENT normalized
    // texts must produce DIFFERENT claim ids. f(x)==f(y) here would mean two
    // distinct claims share identity — a silent merge.
    #[test]
    fn claim_id_distinct_text_distinct_id(a in arb_token(), b in arb_token(), line in 1u32..5000u32) {
        prop_assume!(a != b);
        let source_id = source_id_from_hash(&blake3_bytes_hex(b"fixture-body")).expect("source id");
        let id_a = claim_id_from_parts(&source_id, line, &a);
        let id_b = claim_id_from_parts(&source_id, line, &b);
        prop_assert_ne!(id_a, id_b);
    }

    // INJECTIVITY over line: same text on two DIFFERENT lines must differ. This
    // guards the line component actually participating in identity.
    #[test]
    fn claim_id_distinct_line_distinct_id(text in arb_token(), l1 in 1u32..5000u32, l2 in 1u32..5000u32) {
        prop_assume!(l1 != l2);
        let source_id = source_id_from_hash(&blake3_bytes_hex(b"fixture-body")).expect("source id");
        let id_1 = claim_id_from_parts(&source_id, l1, &text);
        let id_2 = claim_id_from_parts(&source_id, l2, &text);
        prop_assert_ne!(id_1, id_2);
    }

    // INJECTIVITY over source: same (line, text) under two DIFFERENT sources must
    // differ. Guards the source component participating in identity.
    #[test]
    fn claim_id_distinct_source_distinct_id(text in arb_token(), s1 in arb_token(), s2 in arb_token(), line in 1u32..5000u32) {
        prop_assume!(s1 != s2);
        let src1 = source_id_from_hash(&blake3_bytes_hex(s1.as_bytes())).expect("source id");
        let src2 = source_id_from_hash(&blake3_bytes_hex(s2.as_bytes())).expect("source id");
        // The two source ids derive from distinct bodies; if their 12-hex prefixes
        // happened to collide, the claim ids legitimately collide too, so skip.
        prop_assume!(src1 != src2);
        let id_1 = claim_id_from_parts(&src1, line, &text);
        let id_2 = claim_id_from_parts(&src2, line, &text);
        prop_assert_ne!(id_1, id_2);
    }

    // SOURCE ID DETERMINISM: equal body hash yields equal source id.
    #[test]
    fn source_id_is_stable(body in any::<Vec<u8>>()) {
        let hash = blake3_bytes_hex(&body);
        let a = source_id_from_hash(&hash).expect("source id");
        let b = source_id_from_hash(&hash).expect("source id");
        prop_assert_eq!(a, b);
    }

    // SOURCE ID INJECTIVITY: distinct body hashes (full 64-hex blake3 digests)
    // produce distinct source ids, except on the vanishingly unlikely event that
    // their 12-hex prefixes collide — which we detect by comparing the full
    // hashes' prefixes directly, so the assertion is exact, not probabilistic.
    #[test]
    fn source_id_distinct_hash_distinct_id(a in any::<Vec<u8>>(), b in any::<Vec<u8>>()) {
        let ha = blake3_bytes_hex(&a);
        let hb = blake3_bytes_hex(&b);
        let id_a = source_id_from_hash(&ha).expect("source id");
        let id_b = source_id_from_hash(&hb).expect("source id");
        // source id is derived from the first 12 hex chars of the body hash.
        if ha[..12] == hb[..12] {
            prop_assert_eq!(id_a, id_b);
        } else {
            prop_assert_ne!(id_a, id_b);
        }
    }

    // CONFLICT ID COMMUTATIVITY + INJECTIVITY: order-independent for a pair, and
    // distinct unordered pairs (modulo prefix collision) yield distinct ids.
    #[test]
    fn conflict_id_is_commutative_and_distinguishes_pairs(
        a in "claim_[0-9a-f]{12}",
        b in "claim_[0-9a-f]{12}",
        c in "claim_[0-9a-f]{12}",
    ) {
        let ca = ClaimId::try_from(a.as_str()).expect("claim a");
        let cb = ClaimId::try_from(b.as_str()).expect("claim b");
        let cc = ClaimId::try_from(c.as_str()).expect("claim c");

        // Commutativity: the pair {a,b} has one identity regardless of order.
        prop_assert_eq!(conflict_id_from_pair(&ca, &cb), conflict_id_from_pair(&cb, &ca));

        // Injectivity: differing unordered pairs differ. Compare {a,b} vs {a,c}
        // only when b != c (so the unordered pairs genuinely differ).
        prop_assume!(cb != cc);
        prop_assert_ne!(
            conflict_id_from_pair(&ca, &cb),
            conflict_id_from_pair(&ca, &cc)
        );
    }
}
