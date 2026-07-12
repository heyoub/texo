//! Precise SCIP, syntactic, lexical, and artifact-integrity contracts.

use std::path::Path;
use std::process::Command;

use assert_cmd::prelude::*;
use protobuf::{EnumOrUnknown, Message};
use tempfile::TempDir;
use texo::code_index::{build, load, persist, CodeIndexLimits, ARTIFACT_SCHEMA};
use texo::git_source::{capture, CaptureLimits};
use texo::knowledge::{AnalysisQuality, CodeIndexFormat, CodeOccurrenceRole, RepositoryId};

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
    std::fs::write(
        root.path().join("src/lib.rs"),
        "pub fn deploy() { helper(); }\nfn helper() {}\n",
    )?;
    std::fs::write(
        root.path().join("script.py"),
        "def release():\n    deploy()\n",
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
fn prior_schema_artifact_is_a_rebuildable_cache_miss() -> TestResult {
    let root = repository()?;
    let capture = capture(
        root.path(),
        RepositoryId::derive("old-code-index-schema"),
        CaptureLimits::default(),
    )?;
    let mut prepared = build(&capture, None, CodeIndexLimits::default())?;
    prepared.artifact.schema = "texo.code-index.v1".to_string();
    let bytes = batpak::encoding::to_bytes(&prepared.artifact)?;
    let digest = texo::events::ids::blake3_bytes_hex(&bytes);
    let directory = root.path().join(".texo/cache/code-index");
    std::fs::create_dir_all(&directory)?;
    std::fs::write(
        directory.join(format!("{}.bin", prepared.artifact.index_id.as_str())),
        bytes,
    )?;

    assert!(load(root.path(), &prepared.artifact.index_id, &digest)?.is_none());
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
    Ok(())
}
