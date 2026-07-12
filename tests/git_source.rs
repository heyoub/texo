//! Git object-database and frozen worktree capture contracts.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;
use texo::git_source::{capture, compare_commits, CaptureLimits, CapturedLayer};
use texo::knowledge::{CoverageGapKind, RepositoryId, TemporalRelation};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn git(root: &Path, args: &[&str]) -> TestResult {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(())
}

fn git_stdout(root: &Path, args: &[&str]) -> TestResult<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()?;
    if output.status.success() {
        Ok(String::from_utf8(output.stdout)?.trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned().into())
    }
}

fn repository() -> TestResult<TempDir> {
    let root = TempDir::new()?;
    git(root.path(), &["init", "-q"])?;
    git(root.path(), &["config", "user.name", "Texo Test"])?;
    git(
        root.path(),
        &["config", "user.email", "texo@example.invalid"],
    )?;
    std::fs::create_dir_all(root.path().join("docs"))?;
    std::fs::create_dir_all(root.path().join("src"))?;
    std::fs::write(root.path().join("docs/decision.md"), b"Deploy Friday.\n")?;
    std::fs::write(root.path().join("src/lib.rs"), b"pub fn old() {}\n")?;
    std::fs::write(root.path().join("ignored.bin"), b"not source")?;
    git(root.path(), &["add", "."])?;
    git(root.path(), &["commit", "-qm", "initial"])?;
    Ok(root)
}

fn repo_id() -> RepositoryId {
    RepositoryId::derive("git-source-integration")
}

#[test]
fn committed_capture_is_deterministic_and_reads_raw_blobs() -> TestResult {
    let root = repository()?;
    let first = capture(root.path(), repo_id(), CaptureLimits::default())?;
    let second = capture(root.path(), repo_id(), CaptureLimits::default())?;
    assert_eq!(first, second);
    assert!(!first.dirty);
    assert_eq!(
        first.base_commit.format,
        texo::knowledge::GitObjectFormat::Sha1
    );
    assert!(first
        .sources
        .iter()
        .any(|source| source.path == "docs/decision.md"
            && source.bytes == b"Deploy Friday.\n"
            && source.layer == CapturedLayer::Committed
            && source.blob_id.is_some()));
    assert!(!first
        .sources
        .iter()
        .any(|source| source.path == "ignored.bin"));
    Ok(())
}

#[test]
fn dirty_overlay_freezes_modified_untracked_and_deleted_sources() -> TestResult {
    let root = repository()?;
    let clean = capture(root.path(), repo_id(), CaptureLimits::default())?;
    std::fs::write(root.path().join("docs/decision.md"), b"Deploy Tuesday.\n")?;
    std::fs::write(root.path().join("src/new.rs"), b"pub fn new() {}\n")?;
    std::fs::remove_file(root.path().join("src/lib.rs"))?;

    let dirty = capture(root.path(), repo_id(), CaptureLimits::default())?;
    assert!(dirty.dirty);
    assert_eq!(dirty.base_commit, clean.base_commit);
    assert_ne!(dirty.overlay_digest_hex, clean.overlay_digest_hex);
    assert_ne!(dirty.snapshot_id, clean.snapshot_id);
    assert!(dirty.sources.iter().any(|source| {
        source.path == "docs/decision.md"
            && source.bytes == b"Deploy Tuesday.\n"
            && source.layer == CapturedLayer::Worktree
            && source.blob_id.is_none()
    }));
    assert!(dirty
        .sources
        .iter()
        .any(|source| { source.path == "src/new.rs" && source.layer == CapturedLayer::Worktree }));
    assert!(dirty
        .deleted
        .iter()
        .any(|source| source.path == "src/lib.rs"));
    assert!(!dirty
        .sources
        .iter()
        .any(|source| source.path == "src/lib.rs"));
    Ok(())
}

#[test]
fn source_limits_surface_typed_partial_coverage() -> TestResult {
    let root = repository()?;
    let capture = capture(
        root.path(),
        repo_id(),
        CaptureLimits {
            max_files: 1,
            max_file_bytes: 100,
            max_total_bytes: 8,
        },
    )?;
    assert!(capture.coverage.truncated);
    assert!(capture.coverage.gaps.iter().any(|gap| {
        matches!(
            gap.kind,
            CoverageGapKind::SourceTooLarge | CoverageGapKind::BudgetExceeded
        )
    }));
    Ok(())
}

#[cfg(unix)]
#[test]
fn worktree_symbolic_link_is_never_followed() -> TestResult {
    use std::os::unix::fs::symlink;

    let root = repository()?;
    let outside = TempDir::new()?;
    let secret = outside.path().join("secret.rs");
    std::fs::write(&secret, b"do not read")?;
    symlink(&secret, root.path().join("src/link.rs"))?;
    let capture = capture(root.path(), repo_id(), CaptureLimits::default())?;
    assert!(capture.coverage.gaps.iter().any(|gap| {
        gap.path.as_deref() == Some("src/link.rs") && gap.kind == CoverageGapKind::Symlink
    }));
    assert!(!capture
        .sources
        .iter()
        .any(|source| source.path == "src/link.rs"));
    Ok(())
}

#[test]
fn git_dag_comparison_never_uses_commit_timestamps_as_order() -> TestResult {
    let root = repository()?;
    let base = capture(root.path(), repo_id(), CaptureLimits::default())?;
    let base_branch = git_stdout(root.path(), &["branch", "--show-current"])?;

    git(root.path(), &["checkout", "-qb", "left"])?;
    std::fs::write(root.path().join("src/left.rs"), "pub fn left() {}\n")?;
    git(root.path(), &["add", "."])?;
    git(root.path(), &["commit", "-qm", "left"])?;
    let left = capture(root.path(), repo_id(), CaptureLimits::default())?;

    git(root.path(), &["checkout", "-q", &base_branch])?;
    git(root.path(), &["checkout", "-qb", "right"])?;
    std::fs::write(root.path().join("src/right.rs"), "pub fn right() {}\n")?;
    git(root.path(), &["add", "."])?;
    git(root.path(), &["commit", "-qm", "right"])?;
    let right = capture(root.path(), repo_id(), CaptureLimits::default())?;

    assert_eq!(
        compare_commits(root.path(), &base.base_commit, &left.base_commit, 100)?.relation,
        TemporalRelation::Before
    );
    assert_eq!(
        compare_commits(root.path(), &left.base_commit, &base.base_commit, 100)?.relation,
        TemporalRelation::After
    );
    assert_eq!(
        compare_commits(root.path(), &left.base_commit, &right.base_commit, 100)?.relation,
        TemporalRelation::Concurrent
    );
    assert_eq!(
        compare_commits(root.path(), &right.base_commit, &right.base_commit, 0)?.relation,
        TemporalRelation::Same
    );
    let bounded = compare_commits(root.path(), &base.base_commit, &left.base_commit, 0)?;
    assert_eq!(bounded.relation, TemporalRelation::Unknown);
    assert_eq!(bounded.gap, Some(CoverageGapKind::BudgetExceeded));
    Ok(())
}
