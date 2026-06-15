//! PROVES: extract output stays within source lines; normalize is idempotent.

mod common;

use std::collections::HashSet;

use proptest::prelude::*;
use texo_core::{
    extract::{extract_claims, normalize_line},
    source::markdown::{MarkdownDocument, MarkdownLine},
    types::ids::SourceId,
    FIXTURE_OBSERVED_AT_MS,
};

use common::proptest::config;

fn arb_doc() -> impl Strategy<Value = MarkdownDocument> {
    prop::collection::vec(any::<String>(), 0..24).prop_map(|lines| {
        let numbered = lines
            .into_iter()
            .enumerate()
            .map(|(idx, text)| MarkdownLine {
                number: u32::try_from(idx + 1).expect("line number"),
                text,
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
}
