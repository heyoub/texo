//! Evidence-backed backup creation and offline verification contracts.

use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

#[test]
fn backup_is_self_verifying_immutable_and_excludes_derived_state() -> TestResult {
    let root = TempDir::new()?;
    let outside = TempDir::new()?;
    assert!(run(root.path(), &["init", "--workspace", "demo"])?
        .status
        .success());
    std::fs::create_dir_all(root.path().join(".texo/cache"))?;
    std::fs::write(root.path().join(".texo/cache/disposable"), b"not authority")?;
    let dest = outside.path().join("backup-one");

    let created = run(root.path(), &["backup", "create", path(&dest)?, "--json"])?;
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    let report: Value = serde_json::from_slice(&created.stdout)?;
    assert_eq!(report["schema"], "texo.backup-create.v1");
    assert!(dest.join("backup.json").is_file());
    assert!(dest.join("config.toml").is_file());
    assert!(dest.join("store").is_dir());
    assert!(!dest.join("cache").exists());

    let before = fingerprint(&dest)?;
    let verified = run(root.path(), &["backup", "verify", path(&dest)?, "--json"])?;
    assert!(verified.status.success());
    let verification: Value = serde_json::from_slice(&verified.stdout)?;
    assert_eq!(verification["verified"], true);
    assert_eq!(
        fingerprint(&dest)?,
        before,
        "verification must be read-only"
    );

    let duplicate = run(root.path(), &["backup", "create", path(&dest)?, "--json"])?;
    assert!(!duplicate.status.success());
    assert_eq!(
        fingerprint(&dest)?,
        before,
        "existing backup must be immutable"
    );
    Ok(())
}

#[test]
fn backup_verification_fails_closed_after_tampering() -> TestResult {
    let root = TempDir::new()?;
    let outside = TempDir::new()?;
    assert!(run(root.path(), &["init", "--workspace", "demo"])?
        .status
        .success());
    let dest = outside.path().join("backup-tamper");
    assert!(
        run(root.path(), &["backup", "create", path(&dest)?, "--json"])?
            .status
            .success()
    );

    std::fs::OpenOptions::new()
        .append(true)
        .open(dest.join("config.toml"))?
        .write_all(b"\n# tampered\n")?;
    let output = run(root.path(), &["backup", "verify", path(&dest)?, "--json"])?;
    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(report["verified"], false);
    assert!(report["findings"]
        .as_array()
        .ok_or("findings")?
        .iter()
        .any(|finding| finding["kind"] == "config_mismatch"));
    Ok(())
}

#[test]
fn backup_rejects_workspace_overlap_and_symbolic_link_roots() -> TestResult {
    let root = TempDir::new()?;
    assert!(run(root.path(), &["init", "--workspace", "demo"])?
        .status
        .success());
    let overlapping = root.path().join("backup");
    let output = run(
        root.path(),
        &["backup", "create", path(&overlapping)?, "--json"],
    )?;
    assert!(!output.status.success());
    assert!(!overlapping.exists());

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let outside = TempDir::new()?;
        let target = outside.path().join("target");
        std::fs::create_dir(&target)?;
        let link = outside.path().join("linked-backup");
        symlink(&target, &link)?;
        let verification = run(root.path(), &["backup", "verify", path(&link)?, "--json"])?;
        assert!(!verification.status.success());
        let report: Value = serde_json::from_slice(&verification.stdout)?;
        assert_eq!(report["findings"][0]["kind"], "backup_root_invalid");
    }
    Ok(())
}

fn run(root: &std::path::Path, args: &[&str]) -> std::io::Result<std::process::Output> {
    Command::new(env!("CARGO_BIN_EXE_texo"))
        .arg("--root")
        .arg(root)
        .args(args)
        .output()
}

fn path(value: &std::path::Path) -> TestResult<&str> {
    value.to_str().ok_or_else(|| "non-UTF-8 test path".into())
}

fn fingerprint(root: &std::path::Path) -> TestResult<Vec<(String, String)>> {
    let mut rows = walkdir::WalkDir::new(root)
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| {
            Ok((
                entry.path().strip_prefix(root)?.display().to_string(),
                blake3::hash(&std::fs::read(entry.path())?)
                    .to_hex()
                    .to_string(),
            ))
        })
        .collect::<TestResult<Vec<_>>>()?;
    rows.sort();
    Ok(rows)
}

use std::io::Write as _;
