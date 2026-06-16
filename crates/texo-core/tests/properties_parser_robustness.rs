//! PROVES: INV-PARSER-TOTAL — every untrusted parser is a TOTAL function on
//! arbitrary input: it returns `Ok` or a typed `Err`, and NEVER panics. This is
//! the cargo-fuzz substitute (cargo-fuzz is not installed) implemented with
//! proptest over arbitrary bytes/strings.
//!
//! Covered parsers:
//!   * `MarkdownDocument::from_bytes` (source/markdown.rs) on arbitrary &[u8]
//!   * the external-cmd NDJSON parser (extract/cmd.rs parse_claims/build_claim),
//!     reached through the public `extract_via_cmd` by feeding arbitrary stdout
//!   * the event decoder (journal/replay.rs decode_stored_event), reached through
//!     a real BatPak store: arbitrary/unknown kinds must error, never panic

mod common;
mod support;

use common::proptest::config;
use proptest::prelude::*;
use proptest::test_runner::FileFailurePersistence;

use batpak::prelude::*;
use serde::{Deserialize, Serialize};
use support::{setup_demo_journal, temp_workspace};
use texo_core::source::markdown::MarkdownDocument;
use texo_core::types::ids::SourceId;
use texo_core::{extract_via_cmd, FIXTURE_OBSERVED_AT_MS};

// ---------------------------------------------------------------------------
// Parser 1: markdown bytes -> document. Arbitrary bytes must never panic.
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(config())]

    #[test]
    fn markdown_from_bytes_is_total(bytes in any::<Vec<u8>>()) {
        // Must not panic; result is Ok or a typed SourceError (e.g. non-UTF-8).
        // A typed Err is an acceptable total-function outcome; only the Ok branch
        // carries further structural checks.
        if let Ok(doc) = MarkdownDocument::from_bytes("fuzz.md", &bytes) {
            // When it parses, line numbers are 1-based and strictly within the
            // number of physical lines in the input (a non-vacuous structural
            // check: a parser that emitted bogus line numbers would fail here).
            let physical_lines =
                u32::try_from(String::from_utf8_lossy(&bytes).lines().count()).unwrap_or(u32::MAX);
            for line in &doc.lines {
                prop_assert!(line.number >= 1);
                prop_assert!(line.number <= physical_lines.max(1));
            }
            prop_assert!(doc.source_id.starts_with("src_"));
        }
    }

    // Restricting to arbitrary VALID utf-8 strings forces the Ok branch far more
    // often, exercising parse_lines (frontmatter / code-fence state machine) on
    // adversarial-but-decodable content.
    #[test]
    fn markdown_from_utf8_string_is_total(text in any::<String>()) {
        let result = MarkdownDocument::from_bytes("fuzz.md", text.as_bytes());
        // Valid utf-8 can still only fail on id derivation, which cannot happen
        // for a 64-hex blake3 digest; so this should always be Ok and never panic.
        let doc = result.expect("valid utf-8 bytes must parse");
        let physical = u32::try_from(text.lines().count()).unwrap_or(u32::MAX);
        for line in &doc.lines {
            prop_assert!(line.number >= 1 && line.number <= physical.max(1));
        }
    }
}

// ---------------------------------------------------------------------------
// Parser 2: external-cmd NDJSON. Feed arbitrary stdout via `cat` of a temp file
// and assert extract_via_cmd is total. Subprocess-bound, so use a smaller case
// count regardless of PROPTEST_CASES to keep wall time bounded.
// ---------------------------------------------------------------------------

fn cmd_config() -> ProptestConfig {
    ProptestConfig {
        cases: 48,
        failure_persistence: Some(Box::new(FileFailurePersistence::SourceParallel(
            "proptest-regressions",
        ))),
        ..ProptestConfig::default()
    }
}

/// Strategy biased toward JSON-line-shaped strings so the parser reaches
/// build_claim (not just the empty-line skip / serde reject paths).
fn arb_ndjson() -> impl Strategy<Value = String> {
    prop_oneof![
        // Wholly arbitrary text (mostly invalid json -> typed CmdJson error).
        any::<String>(),
        // Plausible-but-adversarial JSON object lines.
        (any::<u32>(), ".*", proptest::option::of(any::<u32>())).prop_map(
            |(line_start, text, conf)| {
                let conf_field = conf
                    .map(|c| format!(", \"confidence_ppm\": {c}"))
                    .unwrap_or_default();
                // serde_json::to_string would escape `text`; emit a hand-built
                // line so we also exercise malformed-escape rejection.
                let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
                format!("{{\"line_start\": {line_start}, \"text\": \"{escaped}\"{conf_field}}}")
            }
        ),
        // Multiple lines (blank lines, mixed valid/invalid).
        prop::collection::vec(prop_oneof![Just(String::new()), ".{0,40}"], 0..6)
            .prop_map(|v| v.join("\n")),
    ]
}

proptest! {
    #![proptest_config(cmd_config())]

    #[test]
    fn extract_via_cmd_ndjson_is_total(stdout in arb_ndjson()) {
        let dir = temp_workspace();
        let root = dir.path();
        // A 3-line document so some line_start values land in-range and others
        // out of range -> both build_claim branches are reachable.
        let doc = MarkdownDocument::from_bytes("doc.md", b"line one\nline two\nline three\n")
            .expect("doc");
        let source_id = SourceId::try_from(doc.source_id.as_str()).expect("source id");

        // Write arbitrary stdout to a file under root and emit it verbatim. The
        // adapter appends `"$1"` (the doc path) to the command; `cat <file>`
        // ignores trailing args, so the child's stdout is exactly `stdout`.
        let payload_path = root.join("ndjson_stdout.txt");
        std::fs::write(&payload_path, stdout.as_bytes()).expect("write payload");
        let cmd = "cat ndjson_stdout.txt #";

        // MUST NOT PANIC. Ok or any typed ExtractError variant is acceptable.
        let result = extract_via_cmd(
            cmd,
            &doc,
            &source_id,
            "demo",
            FIXTURE_OBSERVED_AT_MS,
            root,
        );
        // A typed ExtractError is an acceptable total-function outcome; only the
        // Ok branch carries the validation-gate checks below.
        if let Ok(claims) = result {
            // Any claim that parsed must carry an in-range line_start; the parser
            // rejects out-of-range, so a surviving claim proves the validation
            // gate let only valid rows through (non-vacuous).
            for c in &claims {
                prop_assert!(c.payload.line_start >= 1 && c.payload.line_start <= 3);
                prop_assert!(c.payload.confidence_ppm <= 1_000_000);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Parser 3: event decoder. Inject events with arbitrary unknown kinds (category
// 0xE, arbitrary non-registered type_id) and assert replay surfaces a typed
// DecodeError rather than panicking or silently dropping the entry.
// ---------------------------------------------------------------------------

/// A texo-category payload with a type_id that is NOT one of the five registered
/// kinds (1..=5). The type_id is fixed here (a macro attribute must be a const),
/// but the decoder routes purely on (category, type_id), so this exercises the
/// "matched no known kind -> UnsupportedKind" branch on a genuinely unknown row.
#[derive(Debug, Clone, Serialize, Deserialize, EventPayload)]
#[batpak(category = 0xE, type_id = 0x0AB)]
struct UnknownKindEvent {
    note: String,
}

proptest! {
    // Each case opens a real store; keep the count modest but > 1 so the
    // arbitrary note content is genuinely varied. Events are loaded on the SAME
    // open journal (no close/reopen) so cases never contend on the store's
    // exclusive on-disk lock — the decode path is identical either way.
    #![proptest_config(ProptestConfig { cases: 8, ..ProptestConfig::default() })]

    #[test]
    fn decode_unknown_kind_errors_never_panics(note in any::<String>(), entity in "[a-z]{1,16}") {
        let dir = temp_workspace();
        let journal = setup_demo_journal(dir.path());
        let workspace = journal.config().workspace().expect("workspace");

        let coord =
            Coordinate::new(format!("claim:{entity}"), workspace.scope()).expect("coordinate");
        journal
            .handle()
            .store()
            .append_typed(&coord, &UnknownKindEvent { note })
            .expect("append unknown-kind event");

        // Load (and thus decode) the just-appended event on the same handle. The
        // unknown kind must surface as a typed decode error during decode.
        let result = journal.handle().load_events(&workspace);
        journal.close().expect("close");

        // The unknown kind must surface as a typed decode error, never a panic
        // and never a silent skip (which would make the load spuriously succeed).
        let err = result.expect_err("unknown kind must error");
        prop_assert!(
            err.to_string().contains("unsupported event kind"),
            "expected UnsupportedKind, got: {}",
            err
        );
    }
}
