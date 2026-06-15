//! External extractor command integration.

mod support;

use std::path::PathBuf;

use support::{copy_sample_sources, temp_workspace};
use texo_core::{
    extract_via_cmd, init_workspace, source::markdown::MarkdownDocument, SourceId, TexoRootConfig,
    FIXTURE_OBSERVED_AT_MS,
};

#[test]
fn extract_via_cmd_emits_claims() {
    let dir = temp_workspace();
    copy_sample_sources(dir.path());
    init_workspace(dir.path(), "demo").expect("init");

    let config_path = dir.path().join(".texo/config.toml");
    let mut root = TexoRootConfig::load(&config_path).expect("load");
    let mut demo = root.workspaces.get("demo").cloned().expect("demo entry");
    demo.extractor_cmd = Some(format!(
        "python3 {}",
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../scripts/extract-identity.py")
            .display()
    ));
    root.workspaces.insert("demo".to_string(), demo);
    root.save(&config_path).expect("save");

    let doc = MarkdownDocument::from_path(
        &dir.path().join("sample_sources/meeting_notes.md"),
        dir.path(),
    )
    .expect("doc");
    let source_id = SourceId::try_from(doc.source_id.as_str()).expect("source id");
    let cmd = root
        .workspaces
        .get("demo")
        .and_then(|w| w.extractor_cmd.clone())
        .expect("cmd");

    let claims = extract_via_cmd(
        &cmd,
        &doc,
        &source_id,
        "demo",
        FIXTURE_OBSERVED_AT_MS,
        dir.path(),
    )
    .expect("extract");
    assert!(!claims.is_empty());
    assert!(claims[0].payload.extractor_kind.starts_with("cmd:"));
}
