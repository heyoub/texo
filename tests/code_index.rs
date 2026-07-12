//! Precise SCIP, syntactic, lexical, and artifact-integrity contracts.

use std::path::Path;
use std::process::Command;

use assert_cmd::prelude::*;
use protobuf::{EnumOrUnknown, Message};
use tempfile::TempDir;
use texo::code_index::{build, load, persist, CodeIndexLimits, ARTIFACT_SCHEMA};
use texo::git_source::{capture, CaptureLimits};
use texo::knowledge::{
    AnalysisQuality, CodeIndexFormat, CodeOccurrenceRole, CoverageGapKind, RepositoryId,
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn git(root: &Path, args: &[&str]) -> TestResult {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()?;
    if output.status.success() {
        Ok(())
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
    std::fs::create_dir_all(root.path().join("src"))?;
    std::fs::create_dir_all(root.path().join("docs"))?;
    std::fs::create_dir_all(root.path().join("ui/dist"))?;
    std::fs::write(
        root.path().join("src/lib.rs"),
        "pub fn deploy() { helper(); }\nfn helper() {}\n",
    )?;
    std::fs::write(
        root.path().join("script.py"),
        "def release():\n    deploy()\n    return release\n",
    )?;
    std::fs::write(
        root.path().join("docs/notes.md"),
        "The release helper calls deploy.\n",
    )?;
    std::fs::write(
        root.path().join("ui/dist/generated.js"),
        "function generatedBundle() {}\n",
    )?;
    std::fs::write(
        root.path().join("pnpm-lock.yaml"),
        "generatedDependency: true\n",
    )?;
    git(root.path(), &["add", "."])?;
    git(root.path(), &["commit", "-qm", "code"])?;
    Ok(root)
}

fn scip_for_rust() -> TestResult<Vec<u8>> {
    let mut index = scip::types::Index::new();
    let mut document = scip::types::Document::new();
    document.relative_path = "src/lib.rs".to_string();
    document.position_encoding =
        EnumOrUnknown::new(scip::types::PositionEncoding::UTF8CodeUnitOffsetFromLineStart);
    let mut occurrence = scip::types::Occurrence::new();
    occurrence.range = vec![0, 7, 13];
    occurrence.symbol = "rust-analyzer cargo demo 0.1.0 lib/deploy().".to_string();
    occurrence.symbol_roles = 1;
    document.occurrences.push(occurrence);
    index.documents.push(document);
    Ok(index.write_to_bytes()?)
}

#[test]
fn built_in_index_is_deterministic_syntactic_then_lexical() -> TestResult {
    let root = repository()?;
    let capture = capture(
        root.path(),
        RepositoryId::derive("code-index-test"),
        CaptureLimits::default(),
    )?;
    let first = build(&capture, None, CodeIndexLimits::default())?;
    let second = build(&capture, None, CodeIndexLimits::default())?;
    assert_eq!(first, second);
    assert_eq!(first.artifact.schema, ARTIFACT_SCHEMA);
    assert_eq!(first.artifact.format, CodeIndexFormat::Syntax);
    assert_eq!(
        first.artifact.coverage.analysis_quality,
        AnalysisQuality::Syntactic
    );
    assert!(first.artifact.occurrences.iter().any(|occurrence| {
        occurrence.path == "src/lib.rs"
            && occurrence.display_name == "deploy"
            && occurrence.roles.contains(&CodeOccurrenceRole::Definition)
            && occurrence.analysis_quality == AnalysisQuality::Syntactic
            && occurrence.context.contains("pub fn deploy() { helper(); }")
    }));
    assert_eq!(
        first
            .artifact
            .occurrences
            .iter()
            .filter(|occurrence| {
                occurrence.path == "script.py" && occurrence.display_name == "release"
            })
            .count(),
        1,
        "lexical fallback keeps one discovery row per name and file"
    );
    assert!(!first
        .artifact
        .occurrences
        .iter()
        .any(|occurrence| occurrence.path == "docs/notes.md"));
    assert!(!first.artifact.occurrences.iter().any(|occurrence| {
        occurrence.path == "ui/dist/generated.js" || occurrence.path == "pnpm-lock.yaml"
    }));
    assert!(first.artifact.occurrences.iter().any(|occurrence| {
        occurrence.path == "script.py"
            && occurrence.display_name == "release"
            && occurrence.analysis_quality == AnalysisQuality::Lexical
    }));
    Ok(())
}

#[test]
fn scip_is_precise_and_fallback_covers_absent_documents() -> TestResult {
    let root = repository()?;
    let capture = capture(
        root.path(),
        RepositoryId::derive("scip-index-test"),
        CaptureLimits::default(),
    )?;
    let prepared = build(
        &capture,
        Some(&scip_for_rust()?),
        CodeIndexLimits::default(),
    )?;
    assert_eq!(prepared.artifact.format, CodeIndexFormat::Scip);
    assert_eq!(
        prepared.artifact.coverage.analysis_quality,
        AnalysisQuality::Precise
    );
    let precise = prepared
        .artifact
        .occurrences
        .iter()
        .find(|occurrence| occurrence.symbol.starts_with("rust-analyzer"))
        .ok_or("precise occurrence")?;
    assert_eq!(precise.excerpt, "deploy");
    assert_eq!(precise.line_range.start, 1);
    assert!(precise.roles.contains(&CodeOccurrenceRole::Definition));
    assert!(prepared.artifact.occurrences.iter().any(|occurrence| {
        occurrence.path == "script.py" && occurrence.analysis_quality == AnalysisQuality::Lexical
    }));
    Ok(())
}

#[test]
fn normalized_artifact_authenticates_and_tampering_fails_closed() -> TestResult {
    let root = repository()?;
    let capture = capture(
        root.path(),
        RepositoryId::derive("artifact-test"),
        CaptureLimits::default(),
    )?;
    let prepared = build(&capture, None, CodeIndexLimits::default())?;
    let path = persist(root.path(), &prepared)?;
    let loaded = load(
        root.path(),
        &prepared.artifact.index_id,
        &prepared.artifact_digest_hex,
    )?
    .ok_or("artifact exists")?;
    assert_eq!(loaded, prepared.artifact);

    let mut bytes = std::fs::read(&path)?;
    let first = bytes.first_mut().ok_or("artifact bytes")?;
    *first ^= 0x01;
    std::fs::write(&path, bytes)?;
    assert!(load(
        root.path(),
        &prepared.artifact.index_id,
        &prepared.artifact_digest_hex
    )
    .is_err());
    Ok(())
}

#[test]
fn prior_schema_artifacts_are_rebuildable_cache_misses() -> TestResult {
    let root = repository()?;
    let capture = capture(
        root.path(),
        RepositoryId::derive("old-code-index-schema"),
        CaptureLimits::default(),
    )?;
    for schema in ["texo.code-index.v1", "texo.code-index.v2"] {
        let mut prepared = build(&capture, None, CodeIndexLimits::default())?;
        prepared.artifact.schema = schema.to_string();
        let bytes = batpak::encoding::to_bytes(&prepared.artifact)?;
        let digest = texo::events::ids::blake3_bytes_hex(&bytes);
        let directory = root.path().join(".texo/cache/code-index");
        std::fs::create_dir_all(&directory)?;
        std::fs::write(
            directory.join(format!("{}.bin", prepared.artifact.index_id.as_str())),
            bytes,
        )?;
        assert!(load(root.path(), &prepared.artifact.index_id, &digest)?.is_none());
    }
    Ok(())
}

#[test]
fn occurrence_limit_is_explicit_partial_coverage() -> TestResult {
    let root = repository()?;
    let capture = capture(
        root.path(),
        RepositoryId::derive("bounded-index-test"),
        CaptureLimits::default(),
    )?;
    let prepared = build(
        &capture,
        None,
        CodeIndexLimits {
            max_occurrences: 1,
            ..CodeIndexLimits::default()
        },
    )?;
    assert_eq!(prepared.artifact.occurrences.len(), 1);
    assert!(prepared.artifact.coverage.truncated);
    assert!(prepared
        .artifact
        .coverage
        .gaps
        .iter()
        .any(|gap| gap.kind == texo::knowledge::CoverageGapKind::BudgetExceeded));
    Ok(())
}

#[test]
fn one_index_command_freezes_source_and_builds_code_intelligence() -> TestResult {
    let root = repository()?;
    Command::cargo_bin("texo")?
        .arg("--root")
        .arg(root.path())
        .arg("--workspace")
        .arg("demo")
        .arg("init")
        .assert()
        .success();
    let output = Command::cargo_bin("texo")?
        .arg("--root")
        .arg(root.path())
        .arg("--workspace")
        .arg("demo")
        .arg("index")
        .arg("--json")
        .output()?;
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(value["schema"], "texo.index.v2");
    assert_eq!(value["source"]["snapshot_id"], value["code"]["snapshot_id"]);
    assert_eq!(value["code"]["format"], "syntax");
    assert_eq!(value["code"]["already_indexed"], false);

    let repeated = Command::cargo_bin("texo")?
        .arg("--root")
        .arg(root.path())
        .arg("--workspace")
        .arg("demo")
        .arg("index")
        .arg("--json")
        .output()?;
    assert!(repeated.status.success());
    let repeated: serde_json::Value = serde_json::from_slice(&repeated.stdout)?;
    assert_eq!(repeated["source"]["already_indexed"], true);
    assert_eq!(repeated["code"]["already_indexed"], true);
    assert_eq!(repeated["code"]["receipt"], serde_json::Value::Null);

    let artifact = root.path().join(
        value["code"]["artifact_path"]
            .as_str()
            .ok_or("artifact path")?,
    );
    std::fs::remove_file(&artifact)?;
    let rebuilt = Command::cargo_bin("texo")?
        .arg("--root")
        .arg(root.path())
        .arg("--workspace")
        .arg("demo")
        .arg("index")
        .arg("--json")
        .output()?;
    assert!(rebuilt.status.success());
    let rebuilt: serde_json::Value = serde_json::from_slice(&rebuilt.stdout)?;
    assert_eq!(rebuilt["code"]["already_indexed"], false);
    assert!(artifact.is_file());
    Ok(())
}

#[test]
fn malformed_scip_and_unsupported_source_bytes_fail_honestly() -> TestResult {
    let root = repository()?;
    let initial_capture = capture(
        root.path(),
        RepositoryId::derive("malformed-index-test"),
        CaptureLimits::default(),
    )?;
    assert!(build(
        &initial_capture,
        Some(b"not protobuf"),
        CodeIndexLimits::default()
    )
    .is_err());

    std::fs::write(root.path().join("script.py"), b"def valid():\n\xff\xfe\n")?;
    git(root.path(), &["add", "script.py"])?;
    git(
        root.path(),
        &["commit", "-qm", "unsupported source encoding"],
    )?;
    let capture = capture(
        root.path(),
        RepositoryId::derive("unsupported-source-test"),
        CaptureLimits::default(),
    )?;
    let prepared = build(&capture, None, CodeIndexLimits::default())?;
    assert!(prepared
        .artifact
        .coverage
        .gaps
        .iter()
        .any(|gap| gap.path.as_deref() == Some("script.py")
            && gap.kind == CoverageGapKind::UnsupportedEncoding));
    Ok(())
}

#[cfg(unix)]
#[test]
fn scip_reader_rejects_escape_symlink_and_oversize_inputs() -> TestResult {
    use std::os::unix::fs::symlink;

    let root = repository()?;
    let outside = TempDir::new()?;
    let external = outside.path().join("index.scip");
    std::fs::write(&external, b"external")?;
    assert!(texo::code_index::read_scip(root.path(), &external, 1024).is_err());

    let linked = root.path().join("linked.scip");
    symlink(&external, &linked)?;
    assert!(texo::code_index::read_scip(root.path(), &linked, 1024).is_err());

    let large = root.path().join("large.scip");
    std::fs::write(&large, vec![0_u8; 17])?;
    assert!(texo::code_index::read_scip(root.path(), &large, 16).is_err());
    Ok(())
}

#[cfg(feature = "code-rust")]
#[test]
fn recovered_rust_parse_is_never_reported_as_complete() -> TestResult {
    let root = repository()?;
    std::fs::write(
        root.path().join("src/lib.rs"),
        b"pub fn incomplete(value: {\n",
    )?;
    git(root.path(), &["add", "src/lib.rs"])?;
    git(root.path(), &["commit", "-qm", "incomplete syntax"])?;
    let capture = capture(
        root.path(),
        RepositoryId::derive("parser-recovery-test"),
        CaptureLimits::default(),
    )?;
    let prepared = build(&capture, None, CodeIndexLimits::default())?;
    assert!(prepared.artifact.coverage.gaps.iter().any(|gap| {
        gap.path.as_deref() == Some("src/lib.rs") && gap.kind == CoverageGapKind::AnalysisIncomplete
    }));
    Ok(())
}
