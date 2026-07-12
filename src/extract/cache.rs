//! Record-once caches for extraction and relation model calls.
//!
//! These caches make the nondeterministic LLM steps replayable: the first call
//! writes a content-addressed JSON record, and later calls over the same input
//! return that record without touching the model.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::events::ids::blake3_hash_hex;
use crate::semantics::{ClaimRelater, ProposedClaim, Proposer, RelationVerdict, SemanticsError};

/// Field separator mixed into cache key material.
const SEP: char = '\u{1f}';
/// Separator between heading-path frames.
const HEADING_SEP: char = '\u{1e}';

/// Process-and-thread-unique suffix so concurrent writers to the same content
/// key never share a temp file. A shared fixed `.json.tmp` lets two writers
/// (parallel judge workers, or two `texo relate` processes) `O_TRUNC`+write the
/// same inode — interleaving bytes — then race the rename so the loser gets a
/// spurious `ENOENT`. The final path is content-addressed, so the rename is
/// still idempotent; only the staging file must be private.
fn unique_tmp_path(path: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nonce = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let thread = format!("{:?}", std::thread::current().id());
    let stem = path
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    path.with_file_name(format!(
        ".{stem}.tmp.{pid}.{}.{nonce}",
        thread.trim_start_matches("ThreadId(").trim_end_matches(')')
    ))
}

fn write_atomic<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec(value).map_err(std::io::Error::other)?;
    let tmp = unique_tmp_path(path);
    // Best-effort durability: flush the staging file before the rename so a
    // crash cannot expose a half-written record under the final key.
    let file = std::fs::File::create(&tmp)?;
    {
        use std::io::Write as _;
        let mut writer = std::io::BufWriter::new(&file);
        writer.write_all(&bytes)?;
        writer.flush()?;
    }
    file.sync_all()?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Wraps a [`Proposer`] with an on-disk content-addressed cache.
pub struct CachingProposer<P: Proposer> {
    inner: P,
    dir: PathBuf,
}

impl<P: Proposer> CachingProposer<P> {
    /// Wrap `inner`, storing cache entries as JSON files under `dir`.
    #[must_use]
    pub fn new(inner: P, dir: PathBuf) -> Self {
        Self { inner, dir }
    }

    /// Content-addressed key for a span under the inner proposer's fingerprint.
    #[must_use]
    pub fn cache_key(&self, span_text: &str, heading_path: &[String]) -> String {
        let material = format!(
            "{}{SEP}{}{SEP}{}",
            self.inner.fingerprint(),
            heading_path.join(&HEADING_SEP.to_string()),
            span_text,
        );
        blake3_hash_hex(&material)
    }

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
        let path = self.path_for(&self.cache_key(span_text, heading_path));
        if let Ok(bytes) = std::fs::read(&path) {
            if let Ok(claims) = serde_json::from_slice::<Vec<ProposedClaim>>(&bytes) {
                return Ok(claims);
            }
        }

        let claims = self.inner.propose(span_text, heading_path)?;
        if let Err(error) = write_atomic(&path, &claims) {
            tracing::warn!(
                path = %path.display(),
                error = %error,
                "texo extract cache write skipped"
            );
        }
        Ok(claims)
    }

    fn fingerprint(&self) -> String {
        self.inner.fingerprint()
    }
}

/// Wraps a [`ClaimRelater`] with an on-disk content-addressed cache.
pub struct CachingRelater<R: ClaimRelater> {
    inner: R,
    dir: PathBuf,
}

impl<R: ClaimRelater> CachingRelater<R> {
    /// Wrap `inner`, storing cache entries as JSON files under `dir`.
    #[must_use]
    pub fn new(inner: R, dir: PathBuf) -> Self {
        Self { inner, dir }
    }

    /// Content-addressed key for an ordered `(older, newer)` pair.
    #[must_use]
    pub fn cache_key(&self, older: &str, newer: &str) -> String {
        let material = format!("{}{SEP}{older}{SEP}{newer}", self.inner.fingerprint());
        blake3_hash_hex(&material)
    }

    fn path_for(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.json"))
    }
}

impl<R: ClaimRelater> ClaimRelater for CachingRelater<R> {
    fn relate(&self, older: &str, newer: &str) -> Result<RelationVerdict, SemanticsError> {
        let path = self.path_for(&self.cache_key(older, newer));
        if let Ok(bytes) = std::fs::read(&path) {
            if let Ok(verdict) = serde_json::from_slice::<RelationVerdict>(&bytes) {
                return Ok(verdict);
            }
        }

        let verdict = self.inner.relate(older, newer)?;
        if let Err(error) = write_atomic(&path, &verdict) {
            tracing::warn!(
                path = %path.display(),
                error = %error,
                "texo relate cache write skipped"
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
    use std::cell::Cell;

    use crate::semantics::{ClaimRelation, RelationVerdict};

    use super::*;

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
                    text: "Deploys moved to Tuesday.".to_string(),
                    subject: "deploys".to_string(),
                    predicate: "scheduled".to_string(),
                    object: "Tuesday".to_string(),
                    confidence_ppm: 900_000,
                }],
                fingerprint: fingerprint.to_string(),
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

    fn tmp_dir() -> PathBuf {
        let base = std::env::temp_dir();
        let unique = blake3_hash_hex(&format!("{:p}", &base));
        base.join(format!("texo-extract-cache-test-{unique}"))
    }

    #[test]
    fn second_call_is_a_cache_hit_and_skips_inner() {
        let dir = tmp_dir();
        let _removed = std::fs::remove_dir_all(&dir);
        let proposer = CachingProposer::new(CountingProposer::new("fp-1"), dir.clone());

        let first = proposer
            .propose("a span", &["H".to_string()])
            .expect("first proposal succeeds");
        let second = proposer
            .propose("a span", &["H".to_string()])
            .expect("second proposal succeeds");

        assert_eq!(first, second);
        assert_eq!(proposer.inner.calls.get(), 1);
        let _removed = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn different_span_or_fingerprint_misses() {
        let dir = tmp_dir().join("variants");
        let _removed = std::fs::remove_dir_all(&dir);
        let proposer = CachingProposer::new(CountingProposer::new("fp-1"), dir.clone());

        proposer.propose("span one", &[]).expect("first miss");
        proposer.propose("span two", &[]).expect("second miss");
        proposer
            .propose("span one", &["ctx".to_string()])
            .expect("context miss");
        assert_eq!(proposer.inner.calls.get(), 3);

        let other = CachingProposer::new(CountingProposer::new("fp-2"), dir.clone());
        other
            .propose("span one", &[])
            .expect("new fingerprint miss");
        assert_eq!(other.inner.calls.get(), 1);
        let _removed = std::fs::remove_dir_all(&dir);
    }

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
        let _removed = std::fs::remove_dir_all(&dir);
        let relater = CachingRelater::new(
            CountingRelater {
                calls: Cell::new(0),
                relation: ClaimRelation::Supersedes,
                fingerprint: "fp-1".to_string(),
            },
            dir.clone(),
        );

        let first = relater.relate("Friday", "Tuesday").expect("first relate");
        let second = relater.relate("Friday", "Tuesday").expect("cached relate");
        relater
            .relate("Monday", "Friday")
            .expect("distinct pair relates");

        assert_eq!(first.relation, ClaimRelation::Supersedes);
        assert_eq!(first, second);
        assert_eq!(relater.inner.calls.get(), 2);
        let _removed = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_entry_is_recomputed() {
        let dir = tmp_dir().join("corrupt");
        let _removed = std::fs::remove_dir_all(&dir);
        let proposer = CachingProposer::new(CountingProposer::new("fp-1"), dir.clone());
        let key = proposer.cache_key("span", &[]);
        let path = proposer.path_for(&key);
        std::fs::create_dir_all(&dir).expect("cache dir created");
        std::fs::write(&path, b"{ not valid json").expect("corrupt entry written");

        let out = proposer.propose("span", &[]).expect("recompute succeeds");
        assert_eq!(out, proposer.inner.out);
        assert_eq!(proposer.inner.calls.get(), 1);
        proposer
            .propose("span", &[])
            .expect("cached after overwrite");
        assert_eq!(proposer.inner.calls.get(), 1);
        let _removed = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn concurrent_writes_to_one_key_never_corrupt_the_record() {
        // Many threads write the SAME content key at once; the final file must
        // be exactly one valid record and no writer may observe a torn read.
        let dir = tmp_dir();
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("shared-key.json");
        let verdict = RelationVerdict {
            relation: ClaimRelation::Supersedes,
            score: 1.0,
        };
        std::thread::scope(|scope| {
            for _ in 0..16 {
                let path = path.clone();
                scope.spawn(move || {
                    for _ in 0..32 {
                        write_atomic(&path, &verdict).expect("atomic write");
                        // Every observable state of the final path is a whole
                        // record — the unique staging file guarantees it.
                        let bytes = std::fs::read(&path).expect("read");
                        let parsed: RelationVerdict =
                            serde_json::from_slice(&bytes).expect("no torn record");
                        assert_eq!(parsed.relation, ClaimRelation::Supersedes);
                    }
                });
            }
        });
        // No staging files leak into the cache dir.
        let leaked: Vec<_> = std::fs::read_dir(&dir)
            .expect("readdir")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp."))
            .collect();
        assert!(leaked.is_empty(), "staging files leaked: {leaked:?}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
