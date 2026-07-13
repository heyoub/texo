//! Ignored live Helios e2e over the CLI/op kit.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;
use tempfile::TempDir;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

#[derive(Debug, Deserialize)]
struct Oracle {
    #[serde(default)]
    current_claim: Vec<TextCase>,
    #[serde(default)]
    superseded_claim: Vec<TextCase>,
    #[serde(default)]
    conflict: Vec<ConflictCase>,
    #[serde(default)]
    noise: Vec<TextCase>,
}

#[derive(Debug, Deserialize)]
struct TextCase {
    text_contains: String,
}

#[derive(Debug, Deserialize)]
struct ConflictCase {
    a_contains: String,
    b_contains: String,
}

fn repo_path(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path)
}

fn load_oracle() -> TestResult<Oracle> {
    let raw = std::fs::read_to_string(repo_path("examples/helios/ground_truth.toml"))?;
    Ok(toml::from_str(&raw)?)
}

fn write_config(root: &Path, bin: &Path) -> TestResult {
    let config_dir = root.join(".texo");
    std::fs::create_dir_all(&config_dir)?;
    let extract = format!("{} extract", bin.display());
    let raw = format!(
        r#"default_workspace = "helios"

[workspaces.helios]
primary_journal = "canonical"
docs_glob = "examples/helios/docs/**/*.md"
extractor_cmd = "{extract}"

[workspaces.helios.semantics]
enabled = true

[workspaces.helios.journals.canonical]
role = "canonical"
store_path = ".texo/helios-store"
"#
    );
    std::fs::write(config_dir.join("config.toml"), raw)?;
    Ok(())
}

fn run_texo(bin: &Path, root: &Path, args: &[&str], cache_root: &Path) -> TestResult {
    let output = Command::new(bin)
        .arg("--root")
        .arg(root)
        .args(args)
        .env("TEXO_EXTRACT_CACHE", cache_root.join("extract"))
        .env("TEXO_RELATE_CACHE", cache_root.join("relate"))
        .output()?;
    assert!(
        output.status.success(),
        "texo {:?} failed\nstdout:\n{}\nstderr:\n{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

fn contains_ci(haystack: &str, needle: &str) -> bool {
    haystack
        .to_ascii_lowercase()
        .contains(&needle.to_ascii_lowercase())
}

#[test]
#[ignore = "live model e2e; requires TEXO_LLM_API_KEY"]
fn helios_live_e2e_reaches_oracle() -> TestResult {
    let Ok(key) = std::env::var("TEXO_LLM_API_KEY") else {
        return Ok(());
    };
    if key.trim().is_empty() {
        return Ok(());
    }

    let bin = assert_cmd::cargo::cargo_bin("texo");
    let dir = TempDir::new()?;
    let root = dir.path();
    let cache_root = repo_path(".texo/cache");
    write_config(root, &bin)?;
    let docs = repo_path("examples/helios/docs");
    let docs_arg = docs.to_string_lossy().into_owned();

    run_texo(&bin, root, &["ingest", docs_arg.as_str()], &cache_root)?;
    run_texo(&bin, root, &["relate"], &cache_root)?;
    run_texo(
        &bin,
        root,
        &["compile", "--out", "public/helios"],
        &cache_root,
    )?;

    let trophy = std::fs::read_to_string(root.join("public/helios/onboarding.generated.md"))?;
    let oracle = load_oracle()?;
    for case in &oracle.current_claim {
        assert!(
            contains_ci(&trophy, &case.text_contains),
            "current oracle text missing: {}",
            case.text_contains
        );
    }
    for case in &oracle.superseded_claim {
        assert!(
            contains_ci(&trophy, &case.text_contains),
            "superseded oracle text missing: {}",
            case.text_contains
        );
    }
    for case in &oracle.conflict {
        assert!(
            contains_ci(&trophy, &case.a_contains) && contains_ci(&trophy, &case.b_contains),
            "conflict oracle text missing: {} <> {}",
            case.a_contains,
            case.b_contains
        );
    }
    for case in &oracle.noise {
        assert!(
            !contains_ci(&trophy, &case.text_contains),
            "noise text appeared in live trophy: {}",
            case.text_contains
        );
    }
    Ok(())
}
