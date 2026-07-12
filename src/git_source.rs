//! Deterministic, bounded Git and worktree source capture.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path};

use crate::error::TexoError;
use crate::events::ids::blake3_bytes_hex;
use crate::knowledge::{
    AnalysisQuality, CoverageGap, CoverageGapKind, GitObjectFormat, GitObjectId, KnowledgeCoverage,
    RepositoryId, SourceSnapshotId,
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
    /// Digest of the exact Git index file, or the empty-byte digest.
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

/// Capture a trusted local repository without executing Git filters or hooks.
///
/// The ref is resolved exactly once through `HEAD`. Committed bytes come from
/// the object database; changed and untracked worktree files are read only
/// after rejecting symbolic links and unsafe paths.
///
/// # Errors
/// Returns a source error when the repository cannot be opened safely, its
/// head/tree cannot be read, status cannot be enumerated, or a captured regular
/// file changes while being bounded and read.
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
    let base_tree = git_object_id(tree.id.to_string())?;
    let mut accumulator = CaptureAccumulator::new(limits);
    capture_committed(&repo, &tree, &mut accumulator)?;
    let overlay = capture_overlay(&repo, work_dir, &mut accumulator)?;
    let index_digest_hex = fs::read(repo.index_path())
        .map_or_else(
            |error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    Ok(blake3_bytes_hex(&[]))
                } else {
                    Err(error)
                }
            },
            |bytes| Ok(blake3_bytes_hex(&bytes)),
        )
        .map_err(TexoError::Io)?;
    let overlay_digest_hex = overlay_digest(&overlay);
    let snapshot_material = format!(
        "{CAPTURE_SCHEMA}\u{1f}{repository_id}\u{1f}{}\u{1f}{}\u{1f}{index_digest_hex}\u{1f}{overlay_digest_hex}",
        base_commit.hex, base_tree.hex
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
            Ok(_) => continue,
            Err(_) => {
                accumulator.gap(None, CoverageGapKind::UnsupportedEncoding);
                continue;
            }
        };
        if entry.mode.is_commit() {
            accumulator.gap(Some(path), CoverageGapKind::Gitlink);
            continue;
        }
        let object = repo
            .find_object(entry.oid)
            .map_err(|error| git_error(repo.git_dir(), error))?;
        let blob = object
            .try_into_blob()
            .map_err(|error| git_error(repo.git_dir(), error))?;
        if entry.mode.is_link() {
            accumulator.gap(Some(path.clone()), CoverageGapKind::Symlink);
        }
        accumulator.insert(
            path,
            blob.data.clone(),
            Some(git_object_id(entry.oid.to_string())?),
            CapturedLayer::Committed,
        );
    }
    Ok(())
}

fn capture_overlay(
    repo: &gix::Repository,
    work_dir: &Path,
    accumulator: &mut CaptureAccumulator,
) -> Result<BTreeMap<String, OverlayDigestEntry>, TexoError> {
    let index = repo
        .index_or_empty()
        .map_err(|error| git_error(work_dir, error))?;
    let conflict_paths = index
        .entries()
        .iter()
        .filter(|entry| entry.stage_raw() != 0)
        .filter_map(|entry| std::str::from_utf8(entry.path(&index)).ok())
        .map(ToOwned::to_owned)
        .collect::<BTreeSet<_>>();
    for path in &conflict_paths {
        accumulator.gap(Some(path.clone()), CoverageGapKind::WorktreeConflict);
    }

    let mut overlay = BTreeMap::new();
    let status = repo
        .status(gix::progress::Discard)
        .map_err(|error| git_error(work_dir, error))?
        .untracked_files(gix::status::UntrackedFiles::Files)
        .into_iter(Vec::<gix::bstr::BString>::new())
        .map_err(|error| git_error(work_dir, error))?;
    for item in status {
        let item = item.map_err(|error| git_error(work_dir, error))?;
        let path = match std::str::from_utf8(item.location().as_ref()) {
            Ok(path) if source_path_is_in_scope(path) && safe_relative_path(Path::new(path)) => {
                path.to_string()
            }
            Ok(_) => continue,
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
            accumulator.gap(Some(path), CoverageGapKind::Symlink);
            continue;
        }
        if !metadata.is_file() {
            continue;
        }
        if metadata.len() > accumulator.limits.max_file_bytes {
            accumulator.gap(Some(path), CoverageGapKind::SourceTooLarge);
            continue;
        }
        let bytes = fs::read(&full)?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != metadata.len() {
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
            self.gap(Some(path), CoverageGapKind::LfsPointer);
            return;
        }
        let bytes_len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if bytes_len > self.limits.max_file_bytes {
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
        if let Some(source) = self.sources.remove(path) {
            self.total_bytes = self
                .total_bytes
                .saturating_sub(u64::try_from(source.bytes.len()).unwrap_or(u64::MAX));
        }
        self.deleted.insert(path.to_string());
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
    Path::new(path)
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
        assert!(!source_path_is_in_scope("target/generated.rs"));
        assert!(!source_path_is_in_scope("assets/image.png"));
        assert!(safe_relative_path(Path::new("src/lib.rs")));
        assert!(!safe_relative_path(Path::new("../outside.rs")));
        assert!(!safe_relative_path(Path::new("/absolute.rs")));
    }
}
