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
    stale_line: Vec<TextCase>,
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

struct TrophySections {
    current: String,
    stale: String,
    conflicts: String,
    all: String,
}

#[derive(Clone, Copy)]
enum TrophyBucket {
    Current,
    Stale,
    Conflicts,
    Other,
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

fn run_relate_to_completion(bin: &Path, root: &Path, cache_root: &Path) -> TestResult {
    let mut cursor = 0_u64;
    for _ in 0..100 {
        let cursor_arg = cursor.to_string();
        let output = Command::new(bin)
            .arg("--root")
            .arg(root)
            .args([
                "relate",
                "--json",
                "--pair-budget",
                "100000",
                "--pair-cursor",
                cursor_arg.as_str(),
            ])
            .env("TEXO_EXTRACT_CACHE", cache_root.join("extract"))
            .env("TEXO_RELATE_CACHE", cache_root.join("relate"))
            .output()?;
        assert!(
            matches!(output.status.code(), Some(0 | 2)),
            "relate failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let value: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        if value.get("outcome").and_then(serde_json::Value::as_str) == Some("complete") {
            return Ok(());
        }
        let next = value
            .get("next_candidate_cursor")
            .and_then(serde_json::Value::as_u64)
            .ok_or("partial relate output omitted next_candidate_cursor")?;
        if next == cursor {
            return Err("relate cursor made no progress".into());
        }
        cursor = next;
    }
    Err("relate did not complete within 100 bounded pages".into())
}

fn contains_ci(haystack: &str, needle: &str) -> bool {
    haystack
        .to_ascii_lowercase()
        .contains(&needle.to_ascii_lowercase())
}

fn trophy_sections(all: String) -> TrophySections {
    let mut current = String::new();
    let mut stale = String::new();
    let mut conflicts = String::new();
    let mut selected = TrophyBucket::Other;
    for line in all.lines() {
        if let Some(title) = line.strip_prefix("## ") {
            selected = if title.starts_with("Current") {
                TrophyBucket::Current
            } else if title.starts_with("Stale") {
                TrophyBucket::Stale
            } else if title.starts_with("Conflicts") {
                TrophyBucket::Conflicts
            } else {
                TrophyBucket::Other
            };
            continue;
        }
        let section = match selected {
            TrophyBucket::Current => Some(&mut current),
            TrophyBucket::Stale => Some(&mut stale),
            TrophyBucket::Conflicts => Some(&mut conflicts),
            TrophyBucket::Other => None,
        };
        if let Some(section) = section {
            section.push_str(line);
            section.push('\n');
        }
    }
    TrophySections {
        current,
        stale,
        conflicts,
        all,
    }
}

#[test]
#[ignore = "live model e2e; requires TEXO_LLM_API_KEY"]
fn helios_live_e2e_reaches_oracle() -> TestResult {
    let key = std::env::var("TEXO_LLM_API_KEY")
        .map_err(|_| "explicit Helios live gate requires TEXO_LLM_API_KEY")?;
    if key.trim().is_empty() {
        return Err("explicit Helios live gate requires a non-empty TEXO_LLM_API_KEY".into());
    }

    let bin = assert_cmd::cargo::cargo_bin("texo");
    let dir = TempDir::new()?;
    let root = dir.path();
    // The explicit live gate owns an empty, per-run cache. A checked-in or
    // developer cache must never turn this behavior proof into replay-only
    // validation.
    let cache_root = root.join(".texo/live-cache");
    write_config(root, &bin)?;
    let docs = repo_path("examples/helios/docs");
    let docs_arg = docs.to_string_lossy().into_owned();

    run_texo(&bin, root, &["ingest", docs_arg.as_str()], &cache_root)?;
    run_relate_to_completion(&bin, root, &cache_root)?;
    run_texo(
        &bin,
        root,
        &["compile", "--out", "public/helios"],
        &cache_root,
    )?;

    let trophy = trophy_sections(std::fs::read_to_string(
        root.join("public/helios/onboarding.generated.md"),
    )?);
    let oracle = load_oracle()?;
    for case in &oracle.current_claim {
        assert!(
            contains_ci(&trophy.current, &case.text_contains),
            "current oracle text missing: {}",
            case.text_contains
        );
    }
    for case in &oracle.superseded_claim {
        assert!(
            contains_ci(&trophy.stale, &case.text_contains),
            "superseded oracle text missing: {}",
            case.text_contains
        );
    }
    for case in &oracle.conflict {
        assert!(
            contains_ci(&trophy.conflicts, &case.a_contains)
                && contains_ci(&trophy.conflicts, &case.b_contains),
            "conflict oracle text missing: {} <> {}",
            case.a_contains,
            case.b_contains
        );
    }
    for case in &oracle.stale_line {
        assert!(
            contains_ci(&trophy.stale, &case.text_contains),
            "stale-line oracle text missing from stale section: {}",
            case.text_contains
        );
    }
    for case in &oracle.noise {
        assert!(
            !contains_ci(&trophy.all, &case.text_contains),
            "noise text appeared in live trophy: {}",
            case.text_contains
        );
    }
    Ok(())
}
