//! Session transcripts and the session-end memorization pipeline.
//!
//! Transcripts live in process memory (a mutex-guarded map); the journal is
//! the durable state. Ending a session renders the transcript to a markdown
//! doc under `sessions/`, ingests it through the workspace's configured
//! extractor, and — when the semantic pipeline is enabled and a key is present
//! — runs the relate pass so changed facts are *superseded with receipts*
//! before the next session starts.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::Serialize;
use texo_core::{
    ingest_sources, open_journal_with, relate_claims, ClaimConflictDetected, ClaimId, ClaimStatus,
    ClaimSuperseded, ClaimView, IngestMode, Journal, Open, RelateThresholds, SemanticsConfig,
    WorkspaceId,
};
use texo_extract::CachingRelater;
use texo_semantics::{OpenRouterEmbedder, OpenRouterRelater};

/// Directory (under the workspace root) where session transcripts land. Must
/// stay in sync with the bootstrap `docs_glob` (`sessions/**/*.md`).
pub const SESSIONS_DIR: &str = "sessions";

/// Maximum accepted session id length.
pub const MAX_SESSION_ID_LEN: usize = 64;

/// Coarse cosine prefilter for relating, mirroring the CLI `relate` command:
/// it must sit below the lowest same-subject similarity (the relater does the
/// real separation), so it is intentionally lower than the clustering
/// `cosine_threshold` in `[semantics]`.
const PREFILTER: f32 = 0.60;
/// Environment variable selecting the record-once relate cache directory.
const ENV_RELATE_CACHE: &str = "TEXO_RELATE_CACHE";
/// Default relate cache directory, relative to the workspace root (the same
/// default as the CLI `relate` command).
const DEFAULT_RELATE_CACHE: &str = ".texo/relate-cache";
/// Environment variable holding the OpenAI-compatible backend bearer token.
const ENV_API_KEY: &str = "OPENROUTER_API_KEY";

/// Who spoke an utterance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Speaker {
    /// The human.
    User,
    /// The model.
    Assistant,
}

impl Speaker {
    /// Transcript line prefix for this speaker.
    pub fn prefix(self) -> &'static str {
        match self {
            Self::User => "User: ",
            Self::Assistant => "Assistant: ",
        }
    }

    /// Chat-completions role string.
    pub fn role(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }
}

/// One utterance in a session transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Utterance {
    /// Who spoke.
    pub speaker: Speaker,
    /// What they said.
    pub text: String,
}

/// In-memory transcripts, keyed by session id.
///
/// Only the *current* sessions' turns live here, until `/api/session/end`
/// renders and ingests them; a process restart loses unfinished transcripts
/// but never journaled memory.
#[derive(Debug, Default)]
pub struct SessionStore {
    sessions: Mutex<HashMap<String, Vec<Utterance>>>,
}

impl SessionStore {
    /// Empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append one turn to a session (creating it on first use).
    pub fn push(&self, session_id: &str, utterance: Utterance) {
        self.lock()
            .entry(session_id.to_owned())
            .or_default()
            .push(utterance);
    }

    /// Clone a session's transcript so far (empty when unknown).
    pub fn history(&self, session_id: &str) -> Vec<Utterance> {
        self.lock().get(session_id).cloned().unwrap_or_default()
    }

    /// Remove and return a session's transcript.
    pub fn take(&self, session_id: &str) -> Option<Vec<Utterance>> {
        self.lock().remove(session_id)
    }

    /// Put a transcript back (prepending it to any turns added meanwhile), so
    /// a failed session-end can be retried without losing the conversation.
    pub fn restore(&self, session_id: &str, mut transcript: Vec<Utterance>) {
        let mut sessions = self.lock();
        let entry = sessions.entry(session_id.to_owned()).or_default();
        transcript.append(entry);
        *entry = transcript;
    }

    /// Lock the map, recovering from a poisoned mutex (the data is a plain
    /// append-only map, valid regardless of where a panicking thread stopped).
    fn lock(&self) -> MutexGuard<'_, HashMap<String, Vec<Utterance>>> {
        self.sessions.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Whether a session id is safe to use as a filename stem: non-empty, at most
/// [`MAX_SESSION_ID_LEN`] bytes, ASCII alphanumerics plus `-` and `_` only
/// (no path separators, no dots — the id is interpolated into a path).
pub fn valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_SESSION_ID_LEN
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Stable transcript path for a session: `<root>/sessions/<session_id>.md`.
pub fn session_doc_path(root: &Path, session_id: &str) -> PathBuf {
    root.join(SESSIONS_DIR).join(format!("{session_id}.md"))
}

/// Render a transcript to markdown the extractor sees as clean prose: one
/// utterance per line, speaker-prefixed, blank line between utterances (each
/// line is its own prose span). Newlines and runs of whitespace inside an
/// utterance are collapsed so the one-line invariant holds; blank utterances
/// are dropped. Deterministic: same transcript, same bytes.
pub fn render_transcript(session_id: &str, utterances: &[Utterance]) -> String {
    let mut out = format!("# Session {session_id}\n");
    for utterance in utterances {
        let clean = utterance
            .text
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if clean.is_empty() {
            continue;
        }
        out.push('\n');
        out.push_str(utterance.speaker.prefix());
        out.push_str(&clean);
        out.push('\n');
    }
    out
}

/// What happened to the semantic relate pass at session end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RelateOutcome {
    /// The pass ran; counts of events appended to the journal.
    Ran {
        /// `ClaimSuperseded` events appended.
        supersessions: usize,
        /// `ClaimConflictDetected` events appended.
        conflicts: usize,
    },
    /// The pass did not run (ingest still happened).
    Skipped {
        /// Why it was skipped.
        reason: String,
    },
}

/// Result of memorizing one session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionEndReport {
    /// The session that was memorized.
    pub session_id: String,
    /// Rendered transcript path, relative to the workspace root.
    pub doc_path: String,
    /// New sources journaled by the ingest.
    pub sources_observed: usize,
    /// Claims journaled by the ingest.
    pub claims_recorded: usize,
    /// Supersessions appended *during ingest* (heuristic path only; with
    /// semantics enabled the relate pass owns supersession).
    pub ingest_supersessions: usize,
    /// Outcome of the semantic relate pass.
    pub relate: RelateOutcome,
}

/// Render a session's transcript to `sessions/<id>.md`, ingest it, and run
/// the relate pass. Synchronous (BatPak I/O + possibly model HTTP through the
/// extractor subprocess); call on a `spawn_blocking` worker.
pub fn memorize_session(
    root: &Path,
    workspace: Option<&str>,
    session_id: &str,
    utterances: &[Utterance],
    observed_at_ms: u64,
) -> Result<SessionEndReport> {
    if !valid_session_id(session_id) {
        bail!("invalid session id: {session_id:?}");
    }
    let doc_path = session_doc_path(root, session_id);
    let sessions_dir = doc_path
        .parent()
        .context("session doc path has no parent")?
        .to_path_buf();
    std::fs::create_dir_all(&sessions_dir)
        .with_context(|| format!("creating {}", sessions_dir.display()))?;
    std::fs::write(&doc_path, render_transcript(session_id, utterances))
        .with_context(|| format!("writing {}", doc_path.display()))?;

    let journal = open_journal_with(root, workspace).context("opening texo journal")?;
    let config = journal.config().clone();
    let workspace_id = config.workspace()?;
    // Ingest the whole sessions directory: content-hash dedup makes this
    // idempotent, and it picks up any transcript a crashed earlier session-end
    // rendered but never journaled.
    let committed = ingest_sources(
        journal.handle(),
        &config,
        &workspace_id,
        &sessions_dir,
        IngestMode::Commit,
        observed_at_ms,
        root,
    )
    .context("ingesting session transcript")?;
    let ingest_supersessions = committed
        .receipts
        .iter()
        .filter(|receipt| receipt.kind == "ClaimSuperseded")
        .count();

    let relate = run_relate_pass(&journal, &workspace_id, root, observed_at_ms)?;
    journal.close()?;

    Ok(SessionEndReport {
        session_id: session_id.to_owned(),
        doc_path: doc_path
            .strip_prefix(root)
            .unwrap_or(&doc_path)
            .display()
            .to_string(),
        sources_observed: committed.sources_observed,
        claims_recorded: committed.claims_recorded,
        ingest_supersessions,
        relate,
    })
}

/// The semantic supersession + conflict pass, mirroring the CLI `relate`
/// command: current claims (journal order) -> embed -> cluster -> LLM judge ->
/// journaled `ClaimSuperseded`/`ClaimConflictDetected` events. Skips (rather
/// than fails) when the workspace has semantics disabled or no API key is set,
/// so heuristic/offline workspaces still memorize sessions.
fn run_relate_pass(
    journal: &Journal<Open>,
    workspace_id: &WorkspaceId,
    root: &Path,
    observed_at_ms: u64,
) -> Result<RelateOutcome> {
    let semantics_enabled = journal
        .config()
        .semantics
        .as_ref()
        .is_some_and(|s| s.enabled);
    if !semantics_enabled {
        return Ok(RelateOutcome::Skipped {
            reason: "semantics disabled for this workspace".to_owned(),
        });
    }
    let has_key = std::env::var(ENV_API_KEY)
        .ok()
        .is_some_and(|key| !key.trim().is_empty());
    if !has_key {
        return Ok(RelateOutcome::Skipped {
            reason: format!("{ENV_API_KEY} not set"),
        });
    }

    let replayed = journal.replay(workspace_id)?;
    // Current claims only, ordered by journal sequence (then id) for stable runs.
    let mut claims: Vec<(ClaimId, ClaimView)> = replayed
        .state
        .claims
        .iter()
        .filter(|(_, view)| view.status == ClaimStatus::Current)
        .map(|(id, view)| (id.clone(), view.clone()))
        .collect();
    claims.sort_by(|a, b| {
        a.1.receipt
            .sequence
            .get()
            .cmp(&b.1.receipt.sequence.get())
            .then_with(|| a.0.as_str().cmp(b.0.as_str()))
    });

    let embedder = OpenRouterEmbedder::new(None).context("building embedder")?;
    let cache_dir = std::env::var_os(ENV_RELATE_CACHE)
        .map_or_else(|| root.join(DEFAULT_RELATE_CACHE), PathBuf::from);
    let relater = CachingRelater::new(
        OpenRouterRelater::new(None).context("building relater")?,
        cache_dir,
    );
    let cluster = journal.config().semantics.as_ref().map_or_else(
        || SemanticsConfig::default().cosine_threshold,
        |semantics| semantics.cosine_threshold,
    );
    let thresholds = RelateThresholds {
        cluster,
        prefilter: PREFILTER,
    };
    let out = relate_claims(&claims, &embedder, &relater, thresholds).context("relating claims")?;

    let handle = journal.handle();
    for (old, new, reason) in &out.supersessions {
        handle.append_superseded(&ClaimSuperseded {
            old_claim_id: old.to_string(),
            new_claim_id: new.to_string(),
            workspace_id: workspace_id.to_string(),
            reason: reason.clone(),
            decided_by: "texo-agent".to_owned(),
            observed_at_ms,
        })?;
    }
    for entry in &out.conflicts {
        handle.append_conflict(&ClaimConflictDetected {
            conflict_id: entry.conflict_id.to_string(),
            workspace_id: workspace_id.to_string(),
            claim_a: entry.claim_a.to_string(),
            claim_b: entry.claim_b.to_string(),
            reason: entry.reason.clone(),
            status: "open".to_owned(),
            observed_at_ms,
        })?;
    }

    Ok(RelateOutcome::Ran {
        supersessions: out.supersessions.len(),
        conflicts: out.conflicts.len(),
    })
}

/// Observation timestamp (ms since the Unix epoch) for journal writes.
///
/// `TEXO_OBSERVED_AT_MS`, when set and parsable, overrides wall-clock time so
/// tests and demos can pin deterministic timestamps (the CLI convention).
pub fn observed_at_ms() -> u64 {
    if let Ok(raw) = std::env::var("TEXO_OBSERVED_AT_MS") {
        if let Ok(parsed) = raw.trim().parse::<u64>() {
            return parsed;
        }
    }
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    u64::try_from(millis).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utterance(speaker: Speaker, text: &str) -> Utterance {
        Utterance {
            speaker,
            text: text.to_owned(),
        }
    }

    #[test]
    fn transcript_renders_one_speaker_prefixed_line_per_utterance() {
        let transcript = vec![
            utterance(Speaker::User, "Deploys happen on Friday."),
            utterance(Speaker::Assistant, "Okay, noted."),
        ];
        let rendered = render_transcript("session-1", &transcript);
        assert_eq!(
            rendered,
            "# Session session-1\n\nUser: Deploys happen on Friday.\n\nAssistant: Okay, noted.\n"
        );
    }

    #[test]
    fn transcript_collapses_internal_newlines_and_drops_blank_turns() {
        let transcript = vec![
            utterance(Speaker::User, "line one\nline two\n\n  spaced   out  "),
            utterance(Speaker::Assistant, "   \n \t "),
        ];
        let rendered = render_transcript("s", &transcript);
        // The multi-line utterance became ONE line; the blank one vanished.
        assert_eq!(
            rendered,
            "# Session s\n\nUser: line one line two spaced out\n"
        );
    }

    #[test]
    fn transcript_rendering_is_deterministic() {
        let transcript = vec![utterance(Speaker::User, "I moved to Berlin.")];
        assert_eq!(
            render_transcript("abc", &transcript),
            render_transcript("abc", &transcript)
        );
    }

    #[test]
    fn session_doc_path_is_stable_under_sessions_dir() {
        let path = session_doc_path(Path::new("/ws"), "session-42");
        assert_eq!(path, Path::new("/ws/sessions/session-42.md"));
    }

    #[test]
    fn session_id_validation_rejects_path_escapes() {
        assert!(valid_session_id("session-1"));
        assert!(valid_session_id("A_b-9"));
        assert!(!valid_session_id(""));
        assert!(!valid_session_id("../etc/passwd"));
        assert!(!valid_session_id("a/b"));
        assert!(!valid_session_id("a.b"));
        assert!(!valid_session_id("white space"));
        assert!(!valid_session_id(&"x".repeat(MAX_SESSION_ID_LEN + 1)));
    }

    #[test]
    fn store_push_history_take_restore_round_trip() {
        let store = SessionStore::new();
        assert!(store.history("s").is_empty());
        assert_eq!(store.take("s"), None);

        store.push("s", utterance(Speaker::User, "one"));
        store.push("s", utterance(Speaker::Assistant, "two"));
        assert_eq!(store.history("s").len(), 2);

        let taken = store.take("s").expect("transcript present");
        assert_eq!(taken.len(), 2);
        assert!(store.history("s").is_empty());

        // Restore puts the transcript back BEFORE any turns added meanwhile.
        store.push("s", utterance(Speaker::User, "three"));
        store.restore("s", taken);
        let merged = store.history("s");
        assert_eq!(
            merged.iter().map(|u| u.text.as_str()).collect::<Vec<_>>(),
            vec!["one", "two", "three"]
        );
    }

    #[test]
    fn memorize_rejects_invalid_session_ids_before_touching_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = memorize_session(dir.path(), None, "../oops", &[], 1)
            .expect_err("path-escaping id must be rejected");
        assert!(err.to_string().contains("invalid session id"));
        assert!(
            !dir.path().join("sessions").exists(),
            "nothing may be written for a rejected id"
        );
    }
}
