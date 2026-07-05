//! PROVES: extract output stays within source lines; claim byte offsets stay
//! within the source body and slice back to the claim text; normalize is
//! idempotent; the faithfulness gate is total, bounded, reflexive, and monotone
//! in coverage.

mod common;

use std::collections::HashSet;

use proptest::prelude::*;
use texo_core::{
    assess_faithfulness,
    extract::{extract_claims, normalize_line},
    source::markdown::{MarkdownDocument, MarkdownLine},
    types::ids::SourceId,
    DEFAULT_GROUNDING_THRESHOLD_PPM, FIXTURE_OBSERVED_AT_MS,
};

use common::proptest::config;

fn arb_doc() -> impl Strategy<Value = MarkdownDocument> {
    prop::collection::vec(any::<String>(), 0..24).prop_map(|lines| {
        // Synthetic offsets consistent with a newline-joined body: each line
        // starts one byte (the '\n') after the previous line's end.
        let mut offset = 0usize;
        let numbered = lines
            .into_iter()
            .enumerate()
            .map(|(idx, text)| {
                let char_start = offset;
                offset += text.len() + 1;
                MarkdownLine {
                    number: u32::try_from(idx + 1).expect("line number"),
                    text,
                    char_start,
                }
            })
            .collect();
        MarkdownDocument {
            path: "fixture.md".to_string(),
            body_hash_hex: "fixturehash".to_string(),
            source_id: "src_fixture".to_string(),
            lines: numbered,
        }
    })
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn normalize_line_is_idempotent(line in any::<String>()) {
        let once = normalize_line(&line);
        let twice = normalize_line(&once);
        prop_assert_eq!(once, twice);
    }

    #[test]
    fn extracted_claim_lines_are_from_source(doc in arb_doc()) {
        let source_id = SourceId::try_from("src_abc123def456").expect("source id");
        let extracted = extract_claims(&doc, &source_id, "demo", FIXTURE_OBSERVED_AT_MS)
            .expect("extract");
        let source_lines: HashSet<u32> = doc.lines.iter().map(|l| l.number).collect();
        for claim in extracted {
            prop_assert!(source_lines.contains(&claim.payload.line_start));
            let source_text = doc
                .lines
                .iter()
                .find(|l| l.number == claim.payload.line_start)
                .expect("source line")
                .text
                .clone();
            prop_assert_eq!(claim.payload.text, source_text);
        }
    }

    // INV: for a document parsed from real bytes, every extracted claim's byte
    // span is ordered, ends within the source body (`char_end <= source.len()`),
    // and slices the ORIGINAL source back to exactly the claim text.
    #[test]
    fn extracted_claim_offsets_stay_within_source(source in any::<String>()) {
        let doc = MarkdownDocument::from_bytes("fixture.md", source.as_bytes())
            .expect("valid utf-8 source must parse");
        let source_id = SourceId::try_from("src_abc123def456").expect("source id");
        let extracted = extract_claims(&doc, &source_id, "demo", FIXTURE_OBSERVED_AT_MS)
            .expect("extract");
        for claim in extracted {
            let start = usize::try_from(claim.payload.char_start).expect("start fits usize");
            let end = usize::try_from(claim.payload.char_end).expect("end fits usize");
            prop_assert!(start <= end);
            prop_assert!(end <= source.len());
            prop_assert_eq!(&source[start..end], claim.payload.text.as_str());
        }
    }

    // INV: the faithfulness gate is a total function; recall is a bounded ppm and
    // `grounded` is exactly `recall_ppm >= threshold`. Arbitrary input, no panic.
    #[test]
    fn faithfulness_is_total_and_bounded(
        claim in any::<String>(),
        source in any::<String>(),
        threshold in 0u32..=1_000_000,
    ) {
        let f = assess_faithfulness(&claim, &source, threshold);
        prop_assert!(f.recall_ppm <= 1_000_000);
        prop_assert_eq!(f.grounded, f.recall_ppm >= threshold);
    }

    // INV: a claim with content tokens is fully grounded in itself.
    #[test]
    fn faithfulness_is_reflexive_on_content(
        words in prop::collection::vec("[a-z]{2,8}", 1..8)
    ) {
        let claim = words.join(" ");
        let f = assess_faithfulness(&claim, &claim, DEFAULT_GROUNDING_THRESHOLD_PPM);
        prop_assert!(f.grounded);
        prop_assert_eq!(f.recall_ppm, 1_000_000);
    }

    // INV: a claim is fully grounded in any source that contains all its content
    // tokens (the claim's words plus arbitrary extra words).
    #[test]
    fn faithfulness_grounded_in_token_superset(
        words in prop::collection::vec("[a-z]{2,8}", 1..6),
        extra in prop::collection::vec("[a-z]{2,8}", 0..6),
    ) {
        let claim = words.join(" ");
        let mut all = words;
        all.extend(extra);
        let source = all.join(" ");
        let f = assess_faithfulness(&claim, &source, DEFAULT_GROUNDING_THRESHOLD_PPM);
        prop_assert_eq!(f.recall_ppm, 1_000_000);
        prop_assert!(f.grounded);
    }
}
