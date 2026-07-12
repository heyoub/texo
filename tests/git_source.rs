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
    std::fs::remove_file(root.path().join("src/lib.rs"))?;
    symlink(&secret, root.path().join("src/lib.rs"))?;
    let capture = capture(root.path(), repo_id(), CaptureLimits::default())?;
    assert!(capture.dirty);
    assert!(capture.coverage.gaps.iter().any(|gap| {
        gap.path.as_deref() == Some("src/lib.rs") && gap.kind == CoverageGapKind::Symlink
    }));
    assert!(!capture
        .sources
        .iter()
        .any(|source| source.path == "src/lib.rs"));
    Ok(())
}

#[test]
fn lfs_pointers_and_gitlinks_are_explicit_omissions() -> TestResult {
    let root = repository()?;
    std::fs::write(
        root.path().join("src/large.rs"),
        b"version https://git-lfs.github.com/spec/v1\noid sha256:0000\nsize 42\n",
    )?;
    git(root.path(), &["add", "src/large.rs"])?;
    let head = git_stdout(root.path(), &["rev-parse", "HEAD"])?;
    git(
        root.path(),
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{head},vendor.rs"),
        ],
    )?;
    git(root.path(), &["commit", "-qm", "special entries"])?;

    let captured = capture(root.path(), repo_id(), CaptureLimits::default())?;
    assert!(captured.coverage.gaps.iter().any(|gap| {
        gap.path.as_deref() == Some("src/large.rs") && gap.kind == CoverageGapKind::LfsPointer
    }));
    assert!(captured.coverage.gaps.iter().any(|gap| {
        gap.path.as_deref() == Some("vendor.rs") && gap.kind == CoverageGapKind::Gitlink
    }));
    assert!(!captured
        .sources
        .iter()
        .any(|source| source.path == "src/large.rs" || source.path == "vendor.rs"));
    Ok(())
}

#[test]
fn unresolved_conflict_excludes_stale_base_bytes() -> TestResult {
    let root = repository()?;
    let base_branch = git_stdout(root.path(), &["branch", "--show-current"])?;
    git(root.path(), &["checkout", "-qb", "left"])?;
    std::fs::write(root.path().join("docs/decision.md"), b"Deploy Monday.\n")?;
    git(root.path(), &["commit", "-qam", "left"])?;
    git(root.path(), &["checkout", "-q", &base_branch])?;
    std::fs::write(root.path().join("docs/decision.md"), b"Deploy Tuesday.\n")?;
    git(root.path(), &["commit", "-qam", "right"])?;
    let merge = Command::new("git")
        .arg("-C")
        .arg(root.path())
        .args(["merge", "--no-edit", "left"])
        .output()?;
    assert!(!merge.status.success(), "fixture must leave a conflict");

    let captured = capture(root.path(), repo_id(), CaptureLimits::default())?;
    assert!(captured.dirty);
    assert!(captured.coverage.gaps.iter().any(|gap| {
        gap.path.as_deref() == Some("docs/decision.md")
            && gap.kind == CoverageGapKind::WorktreeConflict
    }));
    assert!(!captured
        .sources
        .iter()
        .any(|source| source.path == "docs/decision.md"));
    Ok(())
}

#[test]
fn equivalent_checkouts_and_ref_rewrites_keep_content_identity() -> TestResult {
    let root = repository()?;
    let first = capture(root.path(), repo_id(), CaptureLimits::default())?;
    let outside = TempDir::new()?;
    let clone = outside.path().join("clone");
    let cloned = Command::new("git")
        .args(["clone", "-q"])
        .arg(root.path())
        .arg(&clone)
        .output()?;
    assert!(cloned.status.success());
    let cloned = capture(&clone, repo_id(), CaptureLimits::default())?;
    assert_eq!(cloned.snapshot_id, first.snapshot_id);
    assert_eq!(cloned.index_digest_hex, first.index_digest_hex);

    std::fs::write(root.path().join("src/later.rs"), b"pub fn later() {}\n")?;
    git(root.path(), &["add", "."])?;
    git(root.path(), &["commit", "-qm", "later"])?;
    git(root.path(), &["reset", "--hard", "HEAD^"])?;
    let rewritten = capture(root.path(), repo_id(), CaptureLimits::default())?;
    assert_eq!(rewritten.snapshot_id, first.snapshot_id);
    assert_eq!(rewritten.index_digest_hex, first.index_digest_hex);
    Ok(())
}

#[test]
fn shallow_history_is_unknown_instead_of_concurrent() -> TestResult {
    let root = repository()?;
    let oldest = capture(root.path(), repo_id(), CaptureLimits::default())?;
    for index in 0..2 {
        std::fs::write(
            root.path().join(format!("src/step{index}.rs")),
            format!("pub fn step{index}() {{}}\n"),
        )?;
        git(root.path(), &["add", "."])?;
        git(root.path(), &["commit", "-qm", "step"])?;
    }
    let outside = TempDir::new()?;
    let shallow = outside.path().join("shallow");
    let url = format!("file://{}", root.path().display());
    let cloned = Command::new("git")
        .args(["clone", "-q", "--depth=1", &url])
        .arg(&shallow)
        .output()?;
    assert!(cloned.status.success());
    let newest = capture(&shallow, repo_id(), CaptureLimits::default())?;
    let comparison = compare_commits(&shallow, &oldest.base_commit, &newest.base_commit, 100)?;
    assert_eq!(comparison.relation, TemporalRelation::Unknown);
    assert_eq!(comparison.gap, Some(CoverageGapKind::ShallowHistory));
    Ok(())
}

#[cfg(unix)]
#[test]
fn non_utf8_committed_paths_surface_unsupported_encoding() -> TestResult {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt as _;

    let root = repository()?;
    let relative = std::path::PathBuf::from(OsString::from_vec(b"src/\xffinvalid.rs".to_vec()));
    std::fs::write(root.path().join(&relative), b"pub fn hidden() {}\n")?;
    let added = Command::new("git")
        .arg("-C")
        .arg(root.path())
        .arg("add")
        .arg("--")
        .arg(&relative)
        .output()?;
    assert!(added.status.success());
    git(root.path(), &["commit", "-qm", "invalid path"])?;
    let captured = capture(root.path(), repo_id(), CaptureLimits::default())?;
    assert!(captured
        .coverage
        .gaps
        .iter()
        .any(|gap| gap.kind == CoverageGapKind::UnsupportedEncoding));
    Ok(())
}

#[cfg(unix)]
#[test]
fn malformed_head_object_fails_closed() -> TestResult {
    use std::os::unix::fs::PermissionsExt as _;

    let root = repository()?;
    let head = git_stdout(root.path(), &["rev-parse", "HEAD"])?;
    let object = root
        .path()
        .join(".git/objects")
        .join(&head[..2])
        .join(&head[2..]);
    std::fs::set_permissions(&object, std::fs::Permissions::from_mode(0o600))?;
    std::fs::write(object, b"not a zlib Git object")?;
    let error = capture(root.path(), repo_id(), CaptureLimits::default())
        .expect_err("corrupt authority object must not produce a capture");
    assert!(error.to_string().contains("git capture"));
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
