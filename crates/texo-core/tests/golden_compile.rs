//! Golden snapshot for compile output bundle.

mod support;

use insta::assert_json_snapshot;
use serde::Serialize;
use support::{copy_sample_sources, ingest_sample_sources, setup_demo_journal, temp_workspace};
use texo_core::{compile_out, FIXTURE_OBSERVED_AT_MS};

#[derive(Serialize)]
struct CompileGolden {
    file_names: Vec<String>,
    onboarding_first_line: String,
}

#[test]
fn compile_demo_snapshot() {
    let dir = temp_workspace();
    copy_sample_sources(dir.path());
    setup_demo_journal(dir.path());
    ingest_sample_sources(dir.path());

    let out = dir.path().join("public");
    let output = compile_out(dir.path(), &out, FIXTURE_OBSERVED_AT_MS).expect("compile");

    let file_names: Vec<String> = output.files.iter().map(|(name, _)| name.clone()).collect();
    let onboarding_first_line = output
        .files
        .iter()
        .find(|(name, _)| name == "onboarding.generated.md")
        .map(|(_, content)| content.lines().next().unwrap_or("").to_string())
        .unwrap_or_default();

    assert_json_snapshot!(
        "compile_demo",
        CompileGolden {
            file_names,
            onboarding_first_line,
        }
    );
}
