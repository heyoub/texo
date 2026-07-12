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

#[test]
fn store_and_snapshot_evidence_tampering_fail_closed() -> TestResult {
    let root = TempDir::new()?;
    let outside = TempDir::new()?;
    assert!(run(root.path(), &["init", "--workspace", "demo"])?
        .status
        .success());

    let store_tamper = outside.path().join("store-tamper");
    create_backup(root.path(), &store_tamper)?;
    let segment = std::fs::read_dir(store_tamper.join("store"))?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|entry| entry.path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "fbat")
        })
        .ok_or("snapshot segment")?;
    std::fs::OpenOptions::new()
        .append(true)
        .open(segment)?
        .write_all(b"tamper")?;
    let store_report = verify_backup(root.path(), &store_tamper, &[])?;
    assert!(!store_report.0);
    assert!(has_finding(&store_report.1, "store_file_mismatch"));

    let evidence_tamper = outside.path().join("evidence-tamper");
    create_backup(root.path(), &evidence_tamper)?;
    let manifest_path = evidence_tamper.join("backup.json");
    let mut manifest: Value = serde_json::from_slice(&std::fs::read(&manifest_path)?)?;
    manifest["snapshot"]["body"]["fence_token"]["token"] = Value::from(999_999_u64);
    let mut bytes = serde_json::to_vec_pretty(&manifest)?;
    bytes.push(b'\n');
    std::fs::write(&manifest_path, bytes)?;
    let evidence_report = verify_backup(root.path(), &evidence_tamper, &[])?;
    assert!(!evidence_report.0);
    assert!(has_finding(
        &evidence_report.1,
        "snapshot_evidence_mismatch"
    ));
    Ok(())
}

#[test]
fn out_of_band_manifest_pin_detects_coordinated_manifest_rewrite() -> TestResult {
    let root = TempDir::new()?;
    let outside = TempDir::new()?;
    assert!(run(root.path(), &["init", "--workspace", "demo"])?
        .status
        .success());
    let dest = outside.path().join("pinned");
    let created = create_backup(root.path(), &dest)?;
    let expected = created["manifest_hash_hex"]
        .as_str()
        .ok_or("manifest hash")?;
    assert!(verify_backup(root.path(), &dest, &["--expect-manifest-hash", expected])?.0);

    let manifest_path = dest.join("backup.json");
    let mut manifest: Value = serde_json::from_slice(&std::fs::read(&manifest_path)?)?;
    manifest["created_at_ms"] = Value::from(
        manifest["created_at_ms"]
            .as_u64()
            .ok_or("created_at_ms")?
            .saturating_add(1),
    );
    let mut bytes = serde_json::to_vec_pretty(&manifest)?;
    bytes.push(b'\n');
    std::fs::write(&manifest_path, bytes)?;

    assert!(
        verify_backup(root.path(), &dest, &[])?.0,
        "unpinned verification proves consistency, not independent authenticity"
    );
    let pinned = verify_backup(root.path(), &dest, &["--expect-manifest-hash", expected])?;
    assert!(!pinned.0);
    assert!(has_finding(&pinned.1, "manifest_hash_mismatch"));
    Ok(())
}

fn create_backup(root: &std::path::Path, dest: &std::path::Path) -> TestResult<Value> {
    let output = run(root, &["backup", "create", path(dest)?, "--json"])?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn verify_backup(
    root: &std::path::Path,
    dest: &std::path::Path,
    extra: &[&str],
) -> TestResult<(bool, Value)> {
    let mut args = vec!["backup", "verify", path(dest)?, "--json"];
    args.extend_from_slice(extra);
    let output = run(root, &args)?;
    Ok((
        output.status.success(),
        serde_json::from_slice(&output.stdout)?,
    ))
}

fn has_finding(report: &Value, kind: &str) -> bool {
    report["findings"]
        .as_array()
        .is_some_and(|findings| findings.iter().any(|finding| finding["kind"] == kind))
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
