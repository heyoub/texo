//! Record-once cache for Stage-1 proposals.
//!
//! The proposer (an LLM) is the one nondeterministic step in extraction. This
//! cache makes it *record-once*: the first time a span is proposed, the result is
//! written to a content-addressed file; subsequent runs over the same span read
//! that file and never call the model. That gives reproducible, offline,
//! zero-cost re-ingests (and a demo that does not need a live key every run),
//! which is the determinism boundary the pipeline relies on.
//!
//! The cache key mixes the proposer's [`Proposer::fingerprint`] (model + prompt
//! version) with the heading context and span text, so changing the model or the
//! prompt invalidates stale entries instead of silently reusing them.

use std::path::{Path, PathBuf};

use serde::Serialize;
use texo_core::types::ids::blake3_hash_hex;
use texo_core::{ClaimRelater, ProposedClaim, Proposer, RelationVerdict, SemanticsError};

/// Field separator (ASCII Unit Separator) mixed into the cache key material so
/// distinct fields can never collide by concatenation.
const SEP: char = '\u{1f}';
/// Separator between heading-path frames.
const HEADING_SEP: char = '\u{1e}';

/// Write `value` as JSON to `path` atomically (temp file + rename) so a
/// concurrent or interrupted run never observes a half-written cache entry.
fn write_atomic<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec(value).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Wraps any [`Proposer`] with an on-disk, content-addressed record-once cache.
pub struct CachingProposer<P: Proposer> {
    inner: P,
    dir: PathBuf,
}

impl<P: Proposer> CachingProposer<P> {
    /// Wrap `inner`, storing cache entries as JSON files under `dir`.
    pub fn new(inner: P, dir: PathBuf) -> Self {
        Self { inner, dir }
    }

    /// Content-addressed key for a span under the inner proposer's fingerprint.
    fn key(&self, span_text: &str, heading_path: &[String]) -> String {
        let material = format!(
            "{}{SEP}{}{SEP}{}",
            self.inner.fingerprint(),
            heading_path.join(&HEADING_SEP.to_string()),
            span_text,
        );
        blake3_hash_hex(&material)
    }

    /// Filesystem path for a cache key.
    fn path_for(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.json"))
    }
}

impl<P: Proposer> Proposer for CachingProposer<P> {
    fn propose(
        &self,
        span_text: &str,
        heading_path: &[String],
    ) -> Result<Vec<ProposedClaim>, SemanticsError> {
        let path = self.path_for(&self.key(span_text, heading_path));

        // Cache hit: a readable, well-formed entry is returned verbatim. A
        // corrupt/partial entry falls through to recompute and overwrite.
        if let Ok(bytes) = std::fs::read(&path) {
            if let Ok(claims) = serde_json::from_slice::<Vec<ProposedClaim>>(&bytes) {
                return Ok(claims);
            }
        }

        let claims = self.inner.propose(span_text, heading_path)?;

        // Best-effort persist: a cache-write failure must not fail an otherwise
        // valid extraction, but it is surfaced (not silently swallowed).
        if let Err(err) = write_atomic(&path, &claims) {
            eprintln!(
                "texo-extract: cache write skipped ({}): {err}",
                path.display()
            );
        }
        Ok(claims)
    }

    fn fingerprint(&self) -> String {
        self.inner.fingerprint()
    }
}

/// Wraps any [`ClaimRelater`] with an on-disk, content-addressed record-once
/// cache. This makes a long O(n^2) relate pass **resumable**: a re-run (e.g. after
/// a transient failure) returns already-judged pairs instantly and only calls the
/// model for the rest.
pub struct CachingRelater<R: ClaimRelater> {
    inner: R,
    dir: PathBuf,
}

impl<R: ClaimRelater> CachingRelater<R> {
    /// Wrap `inner`, storing cache entries as JSON files under `dir`.
    pub fn new(inner: R, dir: PathBuf) -> Self {
        Self { inner, dir }
    }

    /// Content-addressed key for an ordered `(older, newer)` pair under the inner
    /// relater's fingerprint.
    fn key(&self, older: &str, newer: &str) -> String {
        let material = format!("{}{SEP}{older}{SEP}{newer}", self.inner.fingerprint());
        blake3_hash_hex(&material)
    }

    /// Filesystem path for a cache key.
    fn path_for(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.json"))
    }
}

impl<R: ClaimRelater> ClaimRelater for CachingRelater<R> {
    fn relate(&self, older: &str, newer: &str) -> Result<RelationVerdict, SemanticsError> {
        let path = self.path_for(&self.key(older, newer));

        if let Ok(bytes) = std::fs::read(&path) {
            if let Ok(verdict) = serde_json::from_slice::<RelationVerdict>(&bytes) {
                return Ok(verdict);
            }
        }

        let verdict = self.inner.relate(older, newer)?;
        if let Err(err) = write_atomic(&path, &verdict) {
            eprintln!(
                "texo-extract: relate cache write skipped ({}): {err}",
                path.display()
            );
        }
        Ok(verdict)
    }

    fn fingerprint(&self) -> String {
        self.inner.fingerprint()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use texo_core::ClaimRelation;

    /// Proposer stub that counts calls (to prove cache hits skip the inner call).
    struct CountingProposer {
        calls: Cell<usize>,
        out: Vec<ProposedClaim>,
        fingerprint: String,
    }

    impl CountingProposer {
        fn new(fingerprint: &str) -> Self {
            Self {
                calls: Cell::new(0),
                out: vec![ProposedClaim {
                    text: "Deploys moved to Tuesday.".to_owned(),
                    subject: "deploys".to_owned(),
                    predicate: "scheduled".to_owned(),
                    object: "Tuesday".to_owned(),
                    confidence_ppm: 900_000,
                }],
                fingerprint: fingerprint.to_owned(),
            }
        }
    }

    impl Proposer for CountingProposer {
        fn propose(
            &self,
            _span_text: &str,
            _heading_path: &[String],
        ) -> Result<Vec<ProposedClaim>, SemanticsError> {
            self.calls.set(self.calls.get() + 1);
            Ok(self.out.clone())
        }
        fn fingerprint(&self) -> String {
            self.fingerprint.clone()
        }
    }

    fn tmp_dir() -> std::path::PathBuf {
        // Per-test unique dir under the system temp, avoiding a tempfile dep.
        let base = std::env::temp_dir();
        let unique = blake3_hash_hex(&format!("{:p}", &base));
        base.join(format!("texo-extract-cache-test-{unique}"))
    }

    #[test]
    fn second_call_is_a_cache_hit_and_skips_inner() {
        let dir = tmp_dir();
        let _ = std::fs::remove_dir_all(&dir);
        let proposer = CachingProposer::new(CountingProposer::new("fp-1"), dir.clone());

        let first = proposer
            .propose("a span", &["H".to_owned()])
            .expect("first");
        let second = proposer
            .propose("a span", &["H".to_owned()])
            .expect("second");

        assert_eq!(first, second, "cached result matches the live result");
        assert_eq!(
            proposer.inner.calls.get(),
            1,
            "the second call must be served from cache, not the inner proposer"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn different_span_or_fingerprint_misses() {
        let dir = tmp_dir().join("variants");
        let _ = std::fs::remove_dir_all(&dir);
        let proposer = CachingProposer::new(CountingProposer::new("fp-1"), dir.clone());

        proposer.propose("span one", &[]).expect("a");
        proposer.propose("span two", &[]).expect("b");
        proposer
            .propose("span one", &["ctx".to_owned()])
            .expect("c");
        assert_eq!(
            proposer.inner.calls.get(),
            3,
            "distinct span/heading keys each miss"
        );

        // A different fingerprint (model/prompt) must not reuse fp-1 entries.
        let other = CachingProposer::new(CountingProposer::new("fp-2"), dir.clone());
        other.propose("span one", &[]).expect("d");
        assert_eq!(other.inner.calls.get(), 1, "new fingerprint -> cache miss");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Relater stub that counts calls, to prove relate cache hits skip the model.
    struct CountingRelater {
        calls: Cell<usize>,
        relation: ClaimRelation,
        fingerprint: String,
    }

    impl ClaimRelater for CountingRelater {
        fn relate(&self, _older: &str, _newer: &str) -> Result<RelationVerdict, SemanticsError> {
            self.calls.set(self.calls.get() + 1);
            Ok(RelationVerdict {
                relation: self.relation,
                score: 1.0,
            })
        }
        fn fingerprint(&self) -> String {
            self.fingerprint.clone()
        }
    }

    #[test]
    fn relate_cache_hit_skips_inner_and_resumes() {
        let dir = tmp_dir().join("relate");
        let _ = std::fs::remove_dir_all(&dir);
        let relater = CachingRelater::new(
            CountingRelater {
                calls: Cell::new(0),
                relation: ClaimRelation::Supersedes,
                fingerprint: "fp-1".to_owned(),
            },
            dir.clone(),
        );

        let a = relater.relate("Friday", "Tuesday").expect("a");
        let b = relater.relate("Friday", "Tuesday").expect("b"); // cached
        relater.relate("Monday", "Friday").expect("c"); // distinct pair -> miss

        assert_eq!(a.relation, ClaimRelation::Supersedes);
        assert_eq!(a, b, "cached verdict matches the live one");
        assert_eq!(
            relater.inner.calls.get(),
            2,
            "the repeated pair is served from cache; only 2 distinct pairs judged"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_entry_is_recomputed() {
        let dir = tmp_dir().join("corrupt");
        let _ = std::fs::remove_dir_all(&dir);
        let proposer = CachingProposer::new(CountingProposer::new("fp-1"), dir.clone());
        // Pre-seed a corrupt cache file at the exact key path.
        let key = proposer.key("span", &[]);
        let path = proposer.path_for(&key);
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(&path, b"{ not valid json").expect("seed");

        let out = proposer.propose("span", &[]).expect("recompute");
        assert_eq!(out, proposer.inner.out);
        assert_eq!(
            proposer.inner.calls.get(),
            1,
            "corrupt entry forces recompute"
        );
        // And the corrupt entry was overwritten with a valid one (next call hits).
        proposer.propose("span", &[]).expect("hit");
        assert_eq!(proposer.inner.calls.get(), 1, "overwritten entry now hits");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
