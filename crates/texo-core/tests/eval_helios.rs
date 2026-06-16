//! PROVES: the Helios acceptance oracle harness.
//!
//! This file is the eval harness for the texo semantic pipeline. It parses the
//! `examples/helios/ground_truth.toml` oracle, ingests `examples/helios/docs`
//! into a temp BatPak store via the texo-core PUBLIC API (the same
//! init_workspace/open_journal_with + ingest_sources pattern as the other
//! integration tests), replays, and scores each oracle category by
//! case-insensitive substring on claim text.
//!
//! Two test surfaces live here:
//!  * `helios_oracle_is_satisfied` — the ACCEPTANCE test, `#[ignore]`d because it
//!    is the intended RED baseline: today's heuristic pipeline does not satisfy
//!    every category. It turns green at Phase 4 (semantic pipeline).
//!  * always-on unit tests of the SCORER itself against tiny inline fixtures,
//!    proving precision/recall/F1 are computed correctly.
//!
//! The reusable `run_eval(corpus_dir, oracle_path) -> EvalReport` entry point is
//! `pub` so later phases can call it with the semantic pipeline.

mod support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use support::temp_workspace;
use texo_core::state::conflict_lifecycle::ConflictReport;
use texo_core::{
    check_staleness, detect_conflicts, ingest_sources, init_workspace, open_journal_with,
    ClaimState, ClaimStatus, ClaimView, IngestMode, StalenessReport, FIXTURE_OBSERVED_AT_MS,
};

const WORKSPACE_ID: &str = "helios";

// ───────────────────────── Oracle model ─────────────────────────

/// Parsed `ground_truth.toml` acceptance oracle.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Oracle {
    /// Claims that must stay CURRENT after ingest.
    #[serde(default)]
    pub current_claim: Vec<CurrentClaimCase>,
    /// Claims that must be SUPERSEDED after ingest.
    #[serde(default)]
    pub superseded_claim: Vec<SupersededClaimCase>,
    /// Conflicts that must be detected.
    #[serde(default)]
    pub conflict: Vec<ConflictCase>,
    /// Onboarding lines that `check-staleness` must flag.
    #[serde(default)]
    pub stale_line: Vec<StaleLineCase>,
    /// Text that must NOT become a claim.
    #[serde(default)]
    pub noise: Vec<NoiseCase>,
}

/// A `current_claim` oracle case.
#[derive(Debug, Clone, Deserialize)]
pub struct CurrentClaimCase {
    /// Canonical subject (informational).
    #[serde(default)]
    pub subject: String,
    /// Substring the current claim's text must contain.
    pub text_contains: String,
    /// Originating source document (informational).
    #[serde(default)]
    pub source: String,
}

/// A `superseded_claim` oracle case.
#[derive(Debug, Clone, Deserialize)]
pub struct SupersededClaimCase {
    /// Canonical subject (informational).
    #[serde(default)]
    pub subject: String,
    /// Substring the superseded claim's text must contain.
    pub text_contains: String,
}

/// A `conflict` oracle case.
#[derive(Debug, Clone, Deserialize)]
pub struct ConflictCase {
    /// Canonical subject (informational).
    #[serde(default)]
    pub subject: String,
    /// Substring one conflicting claim must contain.
    pub a_contains: String,
    /// Substring the other conflicting claim must contain.
    pub b_contains: String,
}

/// A `stale_line` oracle case.
#[derive(Debug, Clone, Deserialize)]
pub struct StaleLineCase {
    /// Source document `check-staleness` runs over.
    pub source: String,
    /// Substring of the line that must be flagged stale.
    pub text_contains: String,
}

/// A `noise` oracle case.
#[derive(Debug, Clone, Deserialize)]
pub struct NoiseCase {
    /// Source document (informational).
    #[serde(default)]
    pub source: String,
    /// Substring that must NOT appear in any claim.
    pub text_contains: String,
}

/// Load and parse a `ground_truth.toml` oracle.
pub fn load_oracle(path: &Path) -> Oracle {
    let raw = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read oracle {}: {e}", path.display()));
    toml::from_str(&raw).unwrap_or_else(|e| panic!("parse oracle {}: {e}", path.display()))
}

// ───────────────────────── Scoring model ─────────────────────────

/// Precision / recall / F1 + pass counts for one oracle category.
#[derive(Debug, Clone, PartialEq)]
pub struct CategoryScore {
    /// Category name (e.g. "current_claim").
    pub name: String,
    /// Oracle cases that passed.
    pub passed: usize,
    /// Total oracle cases in this category.
    pub total: usize,
    /// True positives.
    pub tp: usize,
    /// False positives (only meaningful for categories that draw from a
    /// candidate set; for substring categories tp == passed and fp == 0).
    pub fp: usize,
    /// False negatives (oracle cases not satisfied).
    pub fn_: usize,
}

impl CategoryScore {
    /// Precision = tp / (tp + fp).
    pub fn precision(&self) -> f64 {
        ratio(self.tp, self.tp + self.fp)
    }

    /// Recall = tp / (tp + fn).
    pub fn recall(&self) -> f64 {
        ratio(self.tp, self.tp + self.fn_)
    }

    /// Harmonic mean of precision and recall.
    pub fn f1(&self) -> f64 {
        let p = self.precision();
        let r = self.recall();
        if p + r == 0.0 {
            0.0
        } else {
            2.0 * p * r / (p + r)
        }
    }

    /// All oracle cases in this category satisfied.
    pub fn is_satisfied(&self) -> bool {
        self.passed == self.total
    }
}

fn ratio(num: usize, den: usize) -> f64 {
    if den == 0 {
        // No predictions / no truth: treat as perfect so an empty category does
        // not drag the report down.
        1.0
    } else {
        // Oracle counts are tiny; convert through u32 so the f64 cast is lossless
        // (avoids clippy::cast_precision_loss without an #[allow]).
        let num = u32::try_from(num).unwrap_or(u32::MAX);
        let den = u32::try_from(den).unwrap_or(u32::MAX);
        f64::from(num) / f64::from(den)
    }
}

/// Full eval report across every oracle category.
#[derive(Debug, Clone)]
pub struct EvalReport {
    /// Per-category scores keyed by category name (sorted).
    pub categories: BTreeMap<String, CategoryScore>,
}

impl EvalReport {
    /// Total passed oracle cases across all categories.
    pub fn passed(&self) -> usize {
        self.categories.values().map(|c| c.passed).sum()
    }

    /// Total oracle cases across all categories.
    pub fn total(&self) -> usize {
        self.categories.values().map(|c| c.total).sum()
    }

    /// Every category fully satisfied.
    pub fn all_satisfied(&self) -> bool {
        self.categories.values().all(CategoryScore::is_satisfied)
    }

    /// Render a human-readable report (printed via eprintln in tests).
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("\n===== Helios eval report =====\n");
        out.push_str(&format!(
            "{:<18} {:>7} {:>7} {:>9} {:>9} {:>9}\n",
            "category", "passed", "total", "precision", "recall", "f1"
        ));
        for score in self.categories.values() {
            out.push_str(&format!(
                "{:<18} {:>7} {:>7} {:>9.3} {:>9.3} {:>9.3}\n",
                score.name,
                score.passed,
                score.total,
                score.precision(),
                score.recall(),
                score.f1(),
            ));
        }
        out.push_str(&format!(
            "{:<18} {:>7} {:>7}\n",
            "OVERALL",
            self.passed(),
            self.total()
        ));
        out.push_str(&format!(
            "all categories satisfied: {}\n",
            self.all_satisfied()
        ));
        out.push_str("==============================\n");
        out
    }
}

// ───────────────────────── Scorer ─────────────────────────

fn contains_ci(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

/// Build a category score from a per-case satisfied predicate.
fn score_cases<T>(name: &str, cases: &[T], satisfied: impl Fn(&T) -> bool) -> CategoryScore {
    let total = cases.len();
    let passed = cases.iter().filter(|c| satisfied(c)).count();
    CategoryScore {
        name: name.to_string(),
        passed,
        total,
        tp: passed,
        fp: 0,
        fn_: total - passed,
    }
}

/// A current claim's text contains the oracle substring.
fn current_claim_satisfied(state: &ClaimState, case: &CurrentClaimCase) -> bool {
    state
        .claims
        .values()
        .filter(|c| c.status == ClaimStatus::Current)
        .any(|c| contains_ci(&c.text, &case.text_contains))
}

/// A claim with the oracle text exists AND is Superseded (not current).
fn superseded_claim_satisfied(state: &ClaimState, case: &SupersededClaimCase) -> bool {
    let matches: Vec<&ClaimView> = state
        .claims
        .values()
        .filter(|c| contains_ci(&c.text, &case.text_contains))
        .collect();
    !matches.is_empty() && matches.iter().any(|c| c.status == ClaimStatus::Superseded)
}

/// `detect_conflicts` returns a conflict whose two claims contain a/b (any order).
fn conflict_satisfied(state: &ClaimState, report: &ConflictReport, case: &ConflictCase) -> bool {
    report.conflicts.iter().any(|entry| {
        let Some(a) = state.claim(&entry.claim_a) else {
            return false;
        };
        let Some(b) = state.claim(&entry.claim_b) else {
            return false;
        };
        let forward =
            contains_ci(&a.text, &case.a_contains) && contains_ci(&b.text, &case.b_contains);
        let reverse =
            contains_ci(&b.text, &case.a_contains) && contains_ci(&a.text, &case.b_contains);
        forward || reverse
    })
}

/// `check_staleness` over the named source flags a line containing the substring.
fn stale_line_satisfied(
    reports: &[StalenessReport],
    state: &ClaimState,
    case: &StaleLineCase,
) -> bool {
    reports.iter().any(|report| {
        report.diagnostics.iter().any(|diag| {
            // The diagnostic must come from the oracle source and the flagged
            // claim's text must contain the oracle substring.
            if !diag.file.ends_with(&case.source) {
                return false;
            }
            state
                .claim(&diag.claim_id)
                .is_some_and(|c| contains_ci(&c.text, &case.text_contains))
        })
    })
}

/// NO claim (current OR superseded) has text containing the noise substring.
fn noise_satisfied(state: &ClaimState, case: &NoiseCase) -> bool {
    !state
        .claims
        .values()
        .any(|c| contains_ci(&c.text, &case.text_contains))
}

/// Score every oracle category against replayed state + derived reports.
pub fn score(
    oracle: &Oracle,
    state: &ClaimState,
    conflicts: &ConflictReport,
    stale_reports: &[StalenessReport],
) -> EvalReport {
    let mut categories = BTreeMap::new();
    for s in [
        score_cases("current_claim", &oracle.current_claim, |c| {
            current_claim_satisfied(state, c)
        }),
        score_cases("superseded_claim", &oracle.superseded_claim, |c| {
            superseded_claim_satisfied(state, c)
        }),
        score_cases("conflict", &oracle.conflict, |c| {
            conflict_satisfied(state, conflicts, c)
        }),
        score_cases("stale_line", &oracle.stale_line, |c| {
            stale_line_satisfied(stale_reports, state, c)
        }),
        score_cases("noise", &oracle.noise, |c| noise_satisfied(state, c)),
    ] {
        categories.insert(s.name.clone(), s);
    }
    EvalReport { categories }
}

// ───────────────────────── Harness ─────────────────────────

/// Ingest `corpus_dir`, replay, derive conflict + staleness reports, and score
/// against the oracle at `oracle_path`. The store lives in a temp directory.
pub fn run_eval(corpus_dir: &Path, oracle_path: &Path) -> EvalReport {
    let oracle = load_oracle(oracle_path);
    let dir = temp_workspace();
    let root = dir.path();

    init_workspace(root, WORKSPACE_ID).unwrap_or_else(|e| panic!("init workspace: {e}"));

    // Ingest the whole corpus directory so the current heuristic pipeline
    // computes supersession + conflict edges.
    {
        let journal = open_journal_with(root, Some(WORKSPACE_ID))
            .unwrap_or_else(|e| panic!("open journal for ingest: {e}"));
        let workspace = journal
            .config()
            .workspace()
            .unwrap_or_else(|e| panic!("resolve workspace: {e}"));
        ingest_sources(
            journal.handle(),
            journal.config(),
            &workspace,
            corpus_dir,
            IngestMode::Commit,
            FIXTURE_OBSERVED_AT_MS,
            root,
        )
        .unwrap_or_else(|e| panic!("ingest corpus: {e}"));
        journal
            .close()
            .unwrap_or_else(|e| panic!("close after ingest: {e}"));
    }

    // Replay to a real ClaimState and derive the read-only reports.
    let journal = open_journal_with(root, Some(WORKSPACE_ID))
        .unwrap_or_else(|e| panic!("open journal for replay: {e}"));
    let workspace = journal
        .config()
        .workspace()
        .unwrap_or_else(|e| panic!("resolve workspace: {e}"));
    let replayed = journal
        .replay(&workspace)
        .unwrap_or_else(|e| panic!("replay: {e}"));
    let state = replayed.state;

    let conflicts = detect_conflicts(&state, &workspace);

    // check_staleness over each distinct source named by a stale_line case.
    let mut sources: Vec<String> = oracle.stale_line.iter().map(|c| c.source.clone()).collect();
    sources.sort();
    sources.dedup();
    let mut stale_reports = Vec::new();
    for source in &sources {
        let path = corpus_dir.join(source);
        let report = check_staleness(&state, &workspace, &path, root)
            .unwrap_or_else(|e| panic!("check staleness {source}: {e}"));
        stale_reports.push(report);
    }

    journal
        .close()
        .unwrap_or_else(|e| panic!("close after replay: {e}"));

    score(&oracle, &state, &conflicts, &stale_reports)
}

fn helios_corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/helios/docs")
}

fn helios_oracle_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/helios/ground_truth.toml")
}

#[test]
#[ignore = "acceptance oracle — green at Phase 4"]
fn helios_oracle_is_satisfied() {
    let report = run_eval(&helios_corpus_dir(), &helios_oracle_path());
    eprintln!("{}", report.render());
    assert!(
        report.all_satisfied(),
        "Helios oracle not satisfied by the current pipeline: {} / {} cases passed",
        report.passed(),
        report.total()
    );
}

// ───────────────────────── Scorer unit tests (always on) ─────────────────────────

#[cfg(test)]
mod scorer_tests {
    use super::*;
    use texo_core::stale::diagnostic::{DiagnosticSeverity, StaleDiagnostic};
    use texo_core::state::conflict_lifecycle::ConflictEntry;
    use texo_core::types::ids::WorkspaceId;
    use texo_core::types::ids::{conflict_id_from_pair, SourceId};
    use texo_core::types::receipt::receipt_view;
    use texo_core::{ClaimId, ConflictStatus};

    fn empty_conflicts() -> ConflictReport {
        ConflictReport {
            workspace_id: WorkspaceId::new("demo").expect("workspace id"),
            conflicts: Vec::new(),
        }
    }

    fn claim(id: &str, subject: &str, text: &str, status: ClaimStatus) -> ClaimView {
        ClaimView {
            claim_id: ClaimId::try_from(id).expect("claim id"),
            workspace_id: "demo".to_string(),
            source_id: SourceId::try_from("src_abc123def456").expect("source id"),
            source_path: "x.md".to_string(),
            line_start: 1,
            line_end: 1,
            text: text.to_string(),
            normalized_text: text.to_ascii_lowercase(),
            subject_hint: subject.to_string(),
            predicate_hint: "unknown".to_string(),
            object_hint: String::new(),
            confidence_ppm: 650_000,
            extractor_kind: "test".to_string(),
            status,
            receipt: receipt_view(1, 1, "ClaimRecorded", "workspace:demo", id),
            supersedes: Vec::new(),
            superseded_by: None,
        }
    }

    fn state_with(claims: Vec<ClaimView>) -> ClaimState {
        let mut state = ClaimState {
            replayed_through_sequence: 1,
            ..Default::default()
        };
        for c in claims {
            state.claims.insert(c.claim_id.clone(), c);
        }
        state
    }

    #[test]
    fn current_claim_scored_present_and_absent() {
        let state = state_with(vec![
            claim(
                "claim_aaaaaaaaaaaa",
                "deploy",
                "Deploys moved to Tuesday.",
                ClaimStatus::Current,
            ),
            claim(
                "claim_bbbbbbbbbbbb",
                "deploy",
                "Deploys happen on Friday.",
                ClaimStatus::Superseded,
            ),
        ]);
        let oracle = Oracle {
            current_claim: vec![
                CurrentClaimCase {
                    subject: "deploy".into(),
                    text_contains: "moved to tuesday".into(), // case-insensitive
                    source: String::new(),
                },
                CurrentClaimCase {
                    subject: "deploy".into(),
                    // Present in the store but only as a SUPERSEDED claim, so this
                    // current_claim case must FAIL.
                    text_contains: "happen on Friday".into(),
                    source: String::new(),
                },
            ],
            ..Default::default()
        };
        let report = score(&oracle, &state, &empty_conflicts(), &[]);
        let s = &report.categories["current_claim"];
        assert_eq!((s.passed, s.total), (1, 2));
        assert_eq!(s.tp, 1);
        assert_eq!(s.fn_, 1);
        // precision = 1/1 = 1.0, recall = 1/2 = 0.5, f1 = 2*1*0.5/1.5 = 0.6667
        assert!((s.precision() - 1.0).abs() < 1e-9);
        assert!((s.recall() - 0.5).abs() < 1e-9);
        assert!((s.f1() - (2.0 / 3.0)).abs() < 1e-9);
        assert!(!s.is_satisfied());
    }

    #[test]
    fn superseded_claim_requires_superseded_status() {
        let state = state_with(vec![
            claim(
                "claim_aaaaaaaaaaaa",
                "deploy",
                "Deploys happen on Friday.",
                ClaimStatus::Superseded,
            ),
            claim(
                "claim_bbbbbbbbbbbb",
                "approval",
                "Alice owns release approval.",
                ClaimStatus::Current, // still current -> superseded case must fail
            ),
        ]);
        let oracle = Oracle {
            superseded_claim: vec![
                SupersededClaimCase {
                    subject: "deploy".into(),
                    text_contains: "happen on Friday".into(),
                },
                SupersededClaimCase {
                    subject: "approval".into(),
                    text_contains: "Alice owns release approval".into(),
                },
                SupersededClaimCase {
                    subject: "missing".into(),
                    text_contains: "this text is not in any claim".into(),
                },
            ],
            ..Default::default()
        };
        let report = score(&oracle, &state, &empty_conflicts(), &[]);
        let s = &report.categories["superseded_claim"];
        // Only the first case passes: present AND superseded.
        assert_eq!((s.passed, s.total), (1, 3));
        assert!((s.recall() - (1.0 / 3.0)).abs() < 1e-9);
    }

    #[test]
    fn conflict_scored_either_order() {
        let a = claim(
            "claim_aaaaaaaaaaaa",
            "release",
            "Releases happen on Monday.",
            ClaimStatus::Current,
        );
        let b = claim(
            "claim_bbbbbbbbbbbb",
            "release",
            "Releases go out on Friday.",
            ClaimStatus::Current,
        );
        let state = state_with(vec![a.clone(), b.clone()]);
        let conflicts = ConflictReport {
            workspace_id: WorkspaceId::new("demo").expect("workspace id"),
            conflicts: vec![ConflictEntry {
                conflict_id: conflict_id_from_pair(&a.claim_id, &b.claim_id),
                claim_a: a.claim_id.clone(),
                claim_b: b.claim_id.clone(),
                subject_hint: "release".into(),
                reason: "test".into(),
                status: ConflictStatus::Open,
            }],
        };
        let oracle = Oracle {
            conflict: vec![
                // b_contains matches claim_a, a_contains matches claim_b: reverse order.
                ConflictCase {
                    subject: "release".into(),
                    a_contains: "go out on Friday".into(),
                    b_contains: "happen on Monday".into(),
                },
                // No detected conflict pairs these two texts.
                ConflictCase {
                    subject: "release".into(),
                    a_contains: "go out on Friday".into(),
                    b_contains: "uses BatPak".into(),
                },
            ],
            ..Default::default()
        };
        let report = score(&oracle, &state, &conflicts, &[]);
        let s = &report.categories["conflict"];
        assert_eq!((s.passed, s.total), (1, 2));
        assert!((s.f1() - (2.0 / 3.0)).abs() < 1e-9);
    }

    #[test]
    fn stale_line_matches_source_and_text() {
        let flagged = claim(
            "claim_aaaaaaaaaaaa",
            "deploy",
            "Deploys happen on Friday.",
            ClaimStatus::Superseded,
        );
        let state = state_with(vec![flagged.clone()]);
        let report = StalenessReport {
            workspace_id: WorkspaceId::new("demo").expect("workspace id"),
            checked_path: "docs/01_onboarding_wiki.md".into(),
            replayed_through_sequence: 1,
            diagnostics: vec![StaleDiagnostic {
                file: "docs/01_onboarding_wiki.md".into(),
                line_start: 3,
                line_end: 3,
                severity: DiagnosticSeverity::Warning,
                message: "stale".into(),
                claim_id: flagged.claim_id.clone(),
                superseded_by: None,
                source: None,
                receipt: None,
            }],
        };
        let oracle = Oracle {
            stale_line: vec![
                StaleLineCase {
                    source: "01_onboarding_wiki.md".into(),
                    text_contains: "happen on Friday".into(),
                },
                // Right source, but no flagged claim contains this text.
                StaleLineCase {
                    source: "01_onboarding_wiki.md".into(),
                    text_contains: "uses Postgres".into(),
                },
                // Right text, but the diagnostic is for a different source file.
                StaleLineCase {
                    source: "99_other.md".into(),
                    text_contains: "happen on Friday".into(),
                },
            ],
            ..Default::default()
        };
        let scored = score(&oracle, &state, &empty_conflicts(), &[report]);
        let s = &scored.categories["stale_line"];
        assert_eq!((s.passed, s.total), (1, 3));
    }

    #[test]
    fn noise_passes_when_text_absent_from_all_claims() {
        let state = state_with(vec![
            claim(
                "claim_aaaaaaaaaaaa",
                "deploy",
                "Deploys moved to Tuesday.",
                ClaimStatus::Current,
            ),
            claim(
                "claim_bbbbbbbbbbbb",
                "storage",
                "## Decision: use BatPak.",
                ClaimStatus::Superseded,
            ),
        ]);
        let oracle = Oracle {
            noise: vec![
                // Absent from every claim -> noise correctly excluded -> pass.
                NoiseCase {
                    source: "05_meeting_dump.md".into(),
                    text_contains: "lunch was tacos".into(),
                },
                // Present as a claim (even though superseded) -> noise leaked in -> fail.
                NoiseCase {
                    source: "07_storage_adr.md".into(),
                    text_contains: "## Decision".into(),
                },
            ],
            ..Default::default()
        };
        let report = score(&oracle, &state, &empty_conflicts(), &[]);
        let s = &report.categories["noise"];
        assert_eq!((s.passed, s.total), (1, 2));
        assert!((s.recall() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn perfect_report_is_all_satisfied_with_unit_metrics() {
        let state = state_with(vec![claim(
            "claim_aaaaaaaaaaaa",
            "storage",
            "Helios uses BatPak for storage.",
            ClaimStatus::Current,
        )]);
        let oracle = Oracle {
            current_claim: vec![CurrentClaimCase {
                subject: "storage".into(),
                text_contains: "uses BatPak".into(),
                source: String::new(),
            }],
            ..Default::default()
        };
        let report = score(&oracle, &state, &empty_conflicts(), &[]);
        assert!(report.all_satisfied());
        let s = &report.categories["current_claim"];
        assert!((s.precision() - 1.0).abs() < 1e-9);
        assert!((s.recall() - 1.0).abs() < 1e-9);
        assert!((s.f1() - 1.0).abs() < 1e-9);
        // Empty categories must score perfectly so they never block all_satisfied.
        assert!(report.categories["conflict"].is_satisfied());
        assert!(report.render().contains("Helios eval report"));
    }
}
