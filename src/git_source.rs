//! Deterministic, bounded Git and worktree source capture.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path};

use crate::error::TexoError;
use crate::events::ids::blake3_bytes_hex;
use crate::knowledge::{
    AnalysisQuality, CoverageGap, CoverageGapKind, GitObjectFormat, GitObjectId, KnowledgeCoverage,
    RepositoryId, SourceSnapshotId, TemporalRelation,
};

const CAPTURE_SCHEMA: &str = "texo.git-capture.v1";
const LFS_HEADER: &[u8] = b"version https://git-lfs.github.com/spec/v1\n";
const MAX_GAPS: usize = 256;

/// Default bounded capture limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureLimits {
    /// Maximum source files retained in one capture.
    pub max_files: usize,
    /// Maximum bytes retained for one source.
    pub max_file_bytes: u64,
    /// Maximum bytes retained across all sources.
    pub max_total_bytes: u64,
}

impl Default for CaptureLimits {
    fn default() -> Self {
        Self {
            max_files: 20_000,
            max_file_bytes: 4 * 1024 * 1024,
            max_total_bytes: 128 * 1024 * 1024,
        }
    }
}

/// Source layer that supplied captured bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapturedLayer {
    /// Raw blob bytes from the resolved commit tree.
    Committed,
    /// Frozen worktree bytes overriding the base tree.
    Worktree,
}

/// One exact source captured at a resolved repository state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedSource {
    /// Slash-separated repository-relative path.
    pub path: String,
    /// Exact bytes used by downstream extraction.
    pub bytes: Vec<u8>,
    /// Committed blob identity when the source came from the base tree.
    pub blob_id: Option<GitObjectId>,
    /// Source layer.
    pub layer: CapturedLayer,
}

/// A path deleted by the worktree overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletedSource {
    /// Slash-separated repository-relative path.
    pub path: String,
}

/// Deterministic capture of one resolved commit plus its frozen worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitCapture {
    /// Stable repository identity supplied by Texo configuration/projection.
    pub repository_id: RepositoryId,
    /// Commit resolved once at operation start.
    pub base_commit: GitObjectId,
    /// Tree referenced by `base_commit`.
    pub base_tree: GitObjectId,
    /// Digest of semantic index entries, excluding volatile filesystem stats.
    pub index_digest_hex: String,
    /// Digest of sorted worktree overlay entries and exact bytes.
    pub overlay_digest_hex: String,
    /// Content-addressed snapshot identity.
    pub snapshot_id: SourceSnapshotId,
    /// Whether index/worktree state differs from the base tree.
    pub dirty: bool,
    /// Final captured sources after overlay application, sorted by path.
    pub sources: Vec<CapturedSource>,
    /// Explicit worktree deletions, sorted by path.
    pub deleted: Vec<DeletedSource>,
    /// Honest bounded coverage.
    pub coverage: KnowledgeCoverage,
}

/// Durable comparison result for two Git revisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GitComparison {
    /// Ancestry relation from `left` to `right`.
    pub relation: TemporalRelation,
    /// Typed reason the relation remained unknown.
    pub gap: Option<CoverageGapKind>,
}

/// Compare two commits by bounded parent traversal without consulting dates.
///
/// # Errors
/// Returns a source error for an untrusted repository or malformed object id.
pub fn compare_commits(
    root: &Path,
    left: &GitObjectId,
    right: &GitObjectId,
    max_commits: usize,
) -> Result<GitComparison, TexoError> {
    if left == right {
        return Ok(GitComparison {
            relation: TemporalRelation::Same,
            gap: None,
        });
    }
    let repo = gix::open_opts(root, gix::open::Options::isolated().bail_if_untrusted(true))
        .map_err(|error| git_error(root, error))?;
    match ancestor_of(&repo, left, right, max_commits)? {
        AncestorResult::Found => Ok(GitComparison {
            relation: TemporalRelation::Before,
            gap: None,
        }),
        AncestorResult::Unknown(kind) => Ok(GitComparison {
            relation: TemporalRelation::Unknown,
            gap: Some(kind),
        }),
        AncestorResult::NotFound => match ancestor_of(&repo, right, left, max_commits)? {
            AncestorResult::Found => Ok(GitComparison {
                relation: TemporalRelation::After,
                gap: None,
            }),
            AncestorResult::NotFound => Ok(GitComparison {
                relation: TemporalRelation::Concurrent,
                gap: None,
            }),
            AncestorResult::Unknown(kind) => Ok(GitComparison {
                relation: TemporalRelation::Unknown,
                gap: Some(kind),
            }),
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AncestorResult {
    Found,
    NotFound,
    Unknown(CoverageGapKind),
}

fn ancestor_of(
    repo: &gix::Repository,
    ancestor: &GitObjectId,
    descendant: &GitObjectId,
    max_commits: usize,
) -> Result<AncestorResult, TexoError> {
    let ancestor = parse_object_id(ancestor)?;
    let descendant = parse_object_id(descendant)?;
    let mut pending = vec![descendant];
    let mut seen = BTreeSet::new();
    while let Some(commit_id) = pending.pop() {
        if commit_id == ancestor {
            return Ok(AncestorResult::Found);
        }
        if !seen.insert(commit_id.to_string()) {
            continue;
        }
        if seen.len() > max_commits {
            return Ok(AncestorResult::Unknown(CoverageGapKind::BudgetExceeded));
        }
        let Ok(commit) = repo.find_commit(commit_id) else {
            return Ok(AncestorResult::Unknown(CoverageGapKind::ShallowHistory));
        };
        pending.extend(commit.parent_ids().map(gix::Id::detach));
    }
    Ok(AncestorResult::NotFound)
}

fn parse_object_id(id: &GitObjectId) -> Result<gix::ObjectId, TexoError> {
    gix::ObjectId::from_hex(id.hex.as_bytes()).map_err(|error| TexoError::Source {
        path: ".git/objects".to_string(),
        detail: error.to_string(),
    })
}

/// Capture a trusted local repository without executing Git filters or hooks.
///
/// The ref is resolved exactly once through `HEAD`. Committed bytes come from
/// the object database; changed and untracked worktree files are read only
/// after rejecting symbolic links and unsafe paths.
///
/// # Errors
/// Returns a source error when the repository cannot be opened safely, its
/// head/tree cannot be read, status cannot be enumerated, or a captured regular
/// file changes in length or modification time while it is being read (a
/// length-preserving edit with no timestamp change is not guaranteed detected).
pub fn capture(
    root: &Path,
    repository_id: RepositoryId,
    limits: CaptureLimits,
) -> Result<GitCapture, TexoError> {
    let repo = gix::open_opts(root, gix::open::Options::isolated().bail_if_untrusted(true))
        .map_err(|error| git_error(root, error))?;
    let work_dir = repo.workdir().ok_or_else(|| TexoError::Source {
        path: root.display().to_string(),
        detail: "bare repositories have no developer worktree to snapshot".to_string(),
    })?;
    let commit = repo.head_commit().map_err(|error| git_error(root, error))?;
    let base_commit = git_object_id(commit.id.to_string())?;
    let tree = commit.tree().map_err(|error| git_error(root, error))?;
    let resolved_tree_id = tree.id;
    let base_tree = git_object_id(tree.id.to_string())?;
    let mut accumulator = CaptureAccumulator::new(limits);
    capture_committed(&repo, &tree, &mut accumulator)?;
    let index = repo
        .index_or_empty()
        .map_err(|error| git_error(work_dir, error))?;
    let index_digest_hex = semantic_index_digest(&index);
    let overlay = capture_overlay(&repo, &index, resolved_tree_id, work_dir, &mut accumulator)?;
    let ending_index = repo
        .index_or_empty()
        .map_err(|error| git_error(work_dir, error))?;
    if semantic_index_digest(&ending_index) != index_digest_hex {
        return Err(TexoError::Source {
            path: repo.index_path().display().to_string(),
            detail: "Git index changed while the snapshot was being captured".to_string(),
        });
    }
    let overlay_digest_hex = overlay_digest(&overlay);
    // Fold the capture bounds into the identity: a committed file omitted as
    // too-large under small limits leaves index/overlay digests unchanged, so
    // without this the same commit re-indexed with larger bounds would collide
    // on snapshot_id and the recoverable sources could never be recorded.
    let snapshot_material = format!(
        "{CAPTURE_SCHEMA}\u{1f}{repository_id}\u{1f}{}\u{1f}{}\u{1f}{index_digest_hex}\u{1f}{overlay_digest_hex}\u{1f}{}\u{1f}{}\u{1f}{}",
        base_commit.hex,
        base_tree.hex,
        limits.max_files,
        limits.max_file_bytes,
        limits.max_total_bytes
    );
    let snapshot_id = SourceSnapshotId::derive(&snapshot_material);
    let (sources, deleted, coverage) = accumulator.finish();
    Ok(GitCapture {
        repository_id,
        base_commit,
        base_tree,
        index_digest_hex,
        overlay_digest_hex,
        snapshot_id,
        dirty: !overlay.is_empty(),
        sources,
        deleted,
        coverage,
    })
}

fn capture_committed(
    repo: &gix::Repository,
    tree: &gix::Tree<'_>,
    accumulator: &mut CaptureAccumulator,
) -> Result<(), TexoError> {
    let mut entries = tree
        .traverse()
        .breadthfirst
        .files()
        .map_err(|error| git_error(repo.git_dir(), error))?;
    entries.sort_by(|left, right| left.filepath.cmp(&right.filepath));
    for entry in entries {
        let path = match std::str::from_utf8(entry.filepath.as_ref()) {
            Ok(path) if source_path_is_in_scope(path) => path.to_string(),
            Ok(path) => {
                note_out_of_scope(accumulator, path);
                continue;
            }
            Err(_) => {
                accumulator.gap(None, CoverageGapKind::UnsupportedEncoding);
                continue;
            }
        };
        if entry.mode.is_commit() {
            accumulator.gap(Some(path), CoverageGapKind::Gitlink);
            continue;
        }
        if entry.mode.is_link() {
            accumulator.gap(Some(path), CoverageGapKind::Symlink);
            continue;
        }
        // Bound the blob by its stored object size before materializing it, so a
        // large committed file is recorded as a gap and skipped without ever
        // decompressing or copying its full contents into memory.
        let header = repo
            .find_header(entry.oid)
            .map_err(|error| git_error(repo.git_dir(), error))?;
        if header.size() > accumulator.limits.max_file_bytes {
            accumulator.gap(Some(path), CoverageGapKind::SourceTooLarge);
            continue;
        }
        let object = repo
            .find_object(entry.oid)
            .map_err(|error| git_error(repo.git_dir(), error))?;
        let mut blob = object
            .try_into_blob()
            .map_err(|error| git_error(repo.git_dir(), error))?;
        accumulator.insert(
            path,
            blob.take_data(),
            Some(git_object_id(entry.oid.to_string())?),
            CapturedLayer::Committed,
        );
    }
    Ok(())
}

fn semantic_index_digest(index: &gix::worktree::Index) -> String {
    let mut material = b"texo.git-index.v1".to_vec();
    for entry in index.entries() {
        let path = entry.path(index);
        material.extend_from_slice(&u64::try_from(path.len()).unwrap_or(u64::MAX).to_be_bytes());
        material.extend_from_slice(path.as_ref());
        material.extend_from_slice(entry.id.as_bytes());
        material.extend_from_slice(&entry.mode.bits().to_be_bytes());
        material.extend_from_slice(&entry.stage_raw().to_be_bytes());
    }
    blake3_bytes_hex(&material)
}

fn capture_overlay(
    repo: &gix::Repository,
    index: &gix::worktree::Index,
    resolved_tree_id: gix::ObjectId,
    work_dir: &Path,
    accumulator: &mut CaptureAccumulator,
) -> Result<BTreeMap<String, OverlayDigestEntry>, TexoError> {
    let conflict_paths = index
        .entries()
        .iter()
        .filter(|entry| entry.stage_raw() != 0)
        .filter_map(|entry| std::str::from_utf8(entry.path(index)).ok())
        .map(ToOwned::to_owned)
        .collect::<BTreeSet<_>>();
    for path in &conflict_paths {
        accumulator.gap(Some(path.clone()), CoverageGapKind::WorktreeConflict);
    }

    let mut overlay = BTreeMap::new();
    let status = repo
        .status(gix::progress::Discard)
        .map_err(|error| git_error(work_dir, error))?
        .head_tree(resolved_tree_id)
        .untracked_files(gix::status::UntrackedFiles::Files)
        .into_iter(Vec::<gix::bstr::BString>::new())
        .map_err(|error| git_error(work_dir, error))?;
    for path in conflict_paths
        .iter()
        .filter(|path| source_path_is_in_scope(path) && safe_relative_path(Path::new(path)))
    {
        accumulator.omit(path);
        overlay.insert(path.clone(), OverlayDigestEntry::Omitted("conflict"));
    }
    for item in status {
        let item = item.map_err(|error| git_error(work_dir, error))?;
        let path = match std::str::from_utf8(item.location().as_ref()) {
            Ok(path) if source_path_is_in_scope(path) && safe_relative_path(Path::new(path)) => {
                path.to_string()
            }
            Ok(path) => {
                if safe_relative_path(Path::new(path)) {
                    note_out_of_scope(accumulator, path);
                }
                continue;
            }
            Err(_) => {
                accumulator.gap(None, CoverageGapKind::UnsupportedEncoding);
                continue;
            }
        };
        if conflict_paths.contains(&path) {
            continue;
        }
        let full = work_dir.join(&path);
        let metadata = match fs::symlink_metadata(&full) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                accumulator.remove(&path);
                overlay.insert(path, OverlayDigestEntry::Deleted);
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        if metadata.file_type().is_symlink() {
            accumulator.omit(&path);
            accumulator.gap(Some(path.clone()), CoverageGapKind::Symlink);
            overlay.insert(path, OverlayDigestEntry::Omitted("symlink"));
            continue;
        }
        if !metadata.is_file() {
            continue;
        }
        if metadata.len() > accumulator.limits.max_file_bytes {
            accumulator.omit(&path);
            accumulator.gap(Some(path.clone()), CoverageGapKind::SourceTooLarge);
            overlay.insert(path, OverlayDigestEntry::Omitted("source-too-large"));
            continue;
        }
        let bytes = fs::read(&full)?;
        // Re-stat after the read: a same-length edit escapes a length-only check,
        // so compare length and modification time against the pre-read stat.
        let after = fs::symlink_metadata(&full)?;
        if !after.is_file()
            || after.len() != metadata.len()
            || u64::try_from(bytes.len()).unwrap_or(u64::MAX) != metadata.len()
            || modified_time_changed(&metadata, &after)
        {
            return Err(TexoError::Source {
                path: path.clone(),
                detail: "worktree file changed while the snapshot was being captured".to_string(),
            });
        }
        let digest = blake3_bytes_hex(&bytes);
        overlay.insert(path.clone(), OverlayDigestEntry::Bytes(digest));
        accumulator.insert(path, bytes, None, CapturedLayer::Worktree);
    }
    Ok(overlay)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum OverlayDigestEntry {
    Bytes(String),
    Deleted,
    Omitted(&'static str),
}

fn overlay_digest(entries: &BTreeMap<String, OverlayDigestEntry>) -> String {
    let mut material = Vec::new();
    material.extend_from_slice(CAPTURE_SCHEMA.as_bytes());
    for (path, entry) in entries {
        material.push(0x1f);
        material.extend_from_slice(path.as_bytes());
        material.push(0x1e);
        match entry {
            OverlayDigestEntry::Bytes(digest) => material.extend_from_slice(digest.as_bytes()),
            OverlayDigestEntry::Deleted => material.extend_from_slice(b"deleted"),
            OverlayDigestEntry::Omitted(kind) => material.extend_from_slice(kind.as_bytes()),
        }
    }
    blake3_bytes_hex(&material)
}

struct CaptureAccumulator {
    limits: CaptureLimits,
    sources: BTreeMap<String, CapturedSource>,
    deleted: BTreeSet<String>,
    total_bytes: u64,
    sources_examined: u64,
    truncated: bool,
    gaps: Vec<CoverageGap>,
}

impl CaptureAccumulator {
    fn new(limits: CaptureLimits) -> Self {
        Self {
            limits,
            sources: BTreeMap::new(),
            deleted: BTreeSet::new(),
            total_bytes: 0,
            sources_examined: 0,
            truncated: false,
            gaps: Vec::new(),
        }
    }

    fn insert(
        &mut self,
        path: String,
        bytes: Vec<u8>,
        blob_id: Option<GitObjectId>,
        layer: CapturedLayer,
    ) {
        self.sources_examined = self.sources_examined.saturating_add(1);
        if bytes.starts_with(LFS_HEADER) {
            self.omit(&path);
            self.gap(Some(path), CoverageGapKind::LfsPointer);
            return;
        }
        let bytes_len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if bytes_len > self.limits.max_file_bytes {
            self.omit(&path);
            self.gap(Some(path), CoverageGapKind::SourceTooLarge);
            return;
        }
        let previous_len = self.sources.get(&path).map_or(0, |source| {
            u64::try_from(source.bytes.len()).unwrap_or(u64::MAX)
        });
        let next_total = self
            .total_bytes
            .saturating_sub(previous_len)
            .saturating_add(bytes_len);
        if self.sources.len() >= self.limits.max_files && !self.sources.contains_key(&path)
            || next_total > self.limits.max_total_bytes
        {
            self.omit(&path);
            self.truncated = true;
            self.gap(Some(path), CoverageGapKind::BudgetExceeded);
            return;
        }
        self.total_bytes = next_total;
        self.deleted.remove(&path);
        self.sources.insert(
            path.clone(),
            CapturedSource {
                path,
                bytes,
                blob_id,
                layer,
            },
        );
    }

    fn remove(&mut self, path: &str) {
        self.omit(path);
        self.deleted.insert(path.to_string());
    }

    fn omit(&mut self, path: &str) {
        if let Some(source) = self.sources.remove(path) {
            self.total_bytes = self
                .total_bytes
                .saturating_sub(u64::try_from(source.bytes.len()).unwrap_or(u64::MAX));
        }
    }

    fn gap(&mut self, path: Option<String>, kind: CoverageGapKind) {
        if self.gaps.len() < MAX_GAPS {
            self.gaps.push(CoverageGap { path, kind });
        } else {
            self.truncated = true;
        }
    }

    fn finish(self) -> (Vec<CapturedSource>, Vec<DeletedSource>, KnowledgeCoverage) {
        let occurrences = u64::try_from(self.sources.len()).unwrap_or(u64::MAX);
        (
            self.sources.into_values().collect(),
            self.deleted
                .into_iter()
                .map(|path| DeletedSource { path })
                .collect(),
            KnowledgeCoverage {
                analysis_quality: AnalysisQuality::Unavailable,
                sources_examined: self.sources_examined,
                occurrences,
                truncated: self.truncated,
                gaps: self.gaps,
            },
        )
    }
}

fn source_path_is_in_scope(path: &str) -> bool {
    if path.starts_with(".texo/") || path.starts_with("target/") || path.starts_with(".git/") {
        return false;
    }
    let candidate = Path::new(path);
    if candidate
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(is_wellknown_source_basename)
    {
        return true;
    }
    candidate
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "md" | "rs"
                    | "py"
                    | "js"
                    | "jsx"
                    | "ts"
                    | "tsx"
                    | "go"
                    | "java"
                    | "kt"
                    | "c"
                    | "h"
                    | "cc"
                    | "cpp"
                    | "hpp"
                    | "toml"
                    | "yaml"
                    | "yml"
                    | "json"
                    | "sh"
            )
        })
}

/// Well-known build/config files that carry evidence but have no extension.
///
/// Without this, an extension-only scope silently drops a tracked `Makefile` or
/// `Dockerfile` with neither a source row nor a coverage gap, so search and
/// triangulation report a real file as absent. Shared with the code index so
/// capture and indexing scopes stay in agreement.
pub(crate) fn is_wellknown_source_basename(name: &str) -> bool {
    matches!(
        name,
        "Makefile"
            | "makefile"
            | "GNUmakefile"
            | "Dockerfile"
            | "Containerfile"
            | "justfile"
            | "Justfile"
            | "Rakefile"
            | "Gemfile"
            | "Procfile"
            | "Jenkinsfile"
            | "BUILD"
            | "WORKSPACE"
    )
}

/// Detect a length-preserving concurrent edit by comparing modification times.
///
/// Returns `false` when either timestamp is unavailable, so the caller falls
/// back to length-only detection instead of failing a capture spuriously.
fn modified_time_changed(before: &fs::Metadata, after: &fs::Metadata) -> bool {
    match (before.modified(), after.modified()) {
        (Ok(before), Ok(after)) => before != after,
        _ => false,
    }
}

/// Record a tracked but out-of-scope extensionless file as a visible gap.
///
/// Extensionless files are almost always config or scripts (`Vagrantfile`,
/// `Fastfile`, `Podfile`, ...), so any allowlist is inherently incomplete and
/// their silent absence misleads search/triangulation into treating a real file
/// as nonexistent. Extensioned assets (images, lockfiles) are legitimately not
/// source and stay silent to keep coverage free of non-source noise.
fn note_out_of_scope(accumulator: &mut CaptureAccumulator, path: &str) {
    if Path::new(path).extension().is_none() {
        accumulator.gap(Some(path.to_string()), CoverageGapKind::OutOfScope);
    }
}

fn safe_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn git_object_id(hex: String) -> Result<GitObjectId, TexoError> {
    let format = match hex.len() {
        40 => GitObjectFormat::Sha1,
        64 => GitObjectFormat::Sha256,
        _ => {
            return Err(TexoError::Source {
                path: ".git".to_string(),
                detail: "repository uses an unsupported Git object format".to_string(),
            });
        }
    };
    GitObjectId::new(format, hex).map_err(|error| TexoError::Source {
        path: ".git".to_string(),
        detail: error.to_string(),
    })
}

fn git_error(path: &Path, error: impl std::fmt::Display) -> TexoError {
    TexoError::Source {
        path: path.display().to_string(),
        detail: format!("git capture: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_digest_is_order_independent_and_deletion_sensitive() {
        let mut first = BTreeMap::new();
        first.insert(
            "src/a.rs".to_string(),
            OverlayDigestEntry::Bytes("a".repeat(64)),
        );
        first.insert("src/b.rs".to_string(), OverlayDigestEntry::Deleted);
        let mut second = BTreeMap::new();
        second.insert("src/b.rs".to_string(), OverlayDigestEntry::Deleted);
        second.insert(
            "src/a.rs".to_string(),
            OverlayDigestEntry::Bytes("a".repeat(64)),
        );
        assert_eq!(overlay_digest(&first), overlay_digest(&second));
        second.insert(
            "src/b.rs".to_string(),
            OverlayDigestEntry::Bytes("b".repeat(64)),
        );
        assert_ne!(overlay_digest(&first), overlay_digest(&second));
    }

    #[test]
    fn scope_and_path_guards_are_closed() {
        assert!(source_path_is_in_scope("src/lib.rs"));
        assert!(source_path_is_in_scope("config/settings.json"));
        assert!(source_path_is_in_scope("Makefile"));
        assert!(source_path_is_in_scope("deploy/Dockerfile"));
        assert!(source_path_is_in_scope("justfile"));
        assert!(!source_path_is_in_scope("target/generated.rs"));
        assert!(!source_path_is_in_scope("assets/image.png"));
        assert!(!source_path_is_in_scope("notes.txt"));
        assert!(safe_relative_path(Path::new("src/lib.rs")));
        assert!(!safe_relative_path(Path::new("../outside.rs")));
        assert!(!safe_relative_path(Path::new("/absolute.rs")));
    }
}
