use std::path::Path;

use serde_json::Value;
use tempfile::TempDir;

use super::adapter::{CLAUDE_MCP_PATH, CODEX_CONFIG_PATH, CURSOR_MCP_PATH};
use super::{install, uninstall, ChangeAction, ClientTarget, AGENT_GUIDE_PATH, MCP_MANIFEST_PATH};

#[test]
fn install_all_is_idempotent_and_uninstall_preserves_user_content() {
    let dir = TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join(AGENT_GUIDE_PATH), "# User guide\n").expect("user guide");
    std::fs::create_dir_all(dir.path().join(".codex")).expect("codex dir");
    std::fs::write(dir.path().join(CODEX_CONFIG_PATH), "model = \"example\"\n")
        .expect("codex config");
    std::fs::write(
        dir.path().join(CLAUDE_MCP_PATH),
        b"{\"user\":true,\"mcpServers\":{}}",
    )
    .expect("claude config");

    install(dir.path(), "demo", &[ClientTarget::All], false).expect("install");
    let first = fingerprint_files(dir.path());
    let second = install(dir.path(), "demo", &[ClientTarget::All], false).expect("reinstall");
    assert!(second
        .changes
        .iter()
        .all(|change| change.action == ChangeAction::Unchanged));
    assert_eq!(fingerprint_files(dir.path()), first);

    uninstall(dir.path(), &[], false).expect("uninstall");
    assert!(std::fs::read_to_string(dir.path().join(AGENT_GUIDE_PATH))
        .expect("guide")
        .contains("# User guide"));
    assert!(std::fs::read_to_string(dir.path().join(CODEX_CONFIG_PATH))
        .expect("codex")
        .contains("model = \"example\""));
    let claude: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join(CLAUDE_MCP_PATH)).expect("claude"))
            .expect("valid json");
    assert_eq!(claude["user"], true);
    assert!(claude["mcpServers"].get("texo").is_none());
    assert!(dir.path().join(".texo/config.toml").is_file());
}

#[test]
fn dry_run_is_write_free_on_a_fresh_root() {
    let dir = TempDir::new().expect("tempdir");
    let report = install(dir.path(), "demo", &[ClientTarget::All], true).expect("preview");
    assert!(report.dry_run);
    assert!(report
        .changes
        .iter()
        .all(|change| change.action == ChangeAction::Created));
    assert!(fingerprint_files(dir.path()).is_empty());
}

#[test]
fn conflicting_adapter_fails_before_any_write() {
    let dir = TempDir::new().expect("tempdir");
    let conflict = br#"{"mcpServers":{"texo":{"command":"other"}}}"#;
    std::fs::write(dir.path().join(CLAUDE_MCP_PATH), conflict).expect("conflict");
    let before = fingerprint_files(dir.path());

    let error =
        install(dir.path(), "demo", &[ClientTarget::All], false).expect_err("conflict must fail");

    assert!(error.to_string().contains("not managed"));
    assert_eq!(fingerprint_files(dir.path()), before);
}

#[test]
fn uninstall_refuses_an_unmanaged_texo_entry() {
    let dir = TempDir::new().expect("tempdir");
    let conflict = br#"{"mcpServers":{"texo":{"command":"other"}}}"#;
    std::fs::write(dir.path().join(CLAUDE_MCP_PATH), conflict).expect("conflict");

    let error = uninstall(dir.path(), &[], false).expect_err("unmanaged entry must survive");

    assert!(error.to_string().contains("not managed"));
    assert_eq!(
        std::fs::read(dir.path().join(CLAUDE_MCP_PATH)).expect("preserved"),
        conflict
    );
}

#[test]
fn uninstall_deletes_only_empty_files_created_by_texo() {
    let created = TempDir::new().expect("created root");
    install(created.path(), "demo", &[ClientTarget::All], false).expect("install created");
    uninstall(created.path(), &[], false).expect("uninstall created");
    for relative in [
        CLAUDE_MCP_PATH,
        CURSOR_MCP_PATH,
        CODEX_CONFIG_PATH,
        AGENT_GUIDE_PATH,
    ] {
        assert!(
            !created.path().join(relative).exists(),
            "{relative} removed"
        );
    }
    assert!(created.path().join(".texo/config.toml").is_file());

    let existing = TempDir::new().expect("existing root");
    std::fs::create_dir_all(existing.path().join(".cursor")).expect("cursor dir");
    std::fs::create_dir_all(existing.path().join(".codex")).expect("codex dir");
    std::fs::write(existing.path().join(CLAUDE_MCP_PATH), "{}\n").expect("claude");
    std::fs::write(existing.path().join(CURSOR_MCP_PATH), "{}\n").expect("cursor");
    std::fs::write(existing.path().join(CODEX_CONFIG_PATH), "").expect("codex");
    std::fs::write(existing.path().join(AGENT_GUIDE_PATH), "").expect("guide");
    install(existing.path(), "demo", &[ClientTarget::All], false).expect("install existing");
    uninstall(existing.path(), &[], false).expect("uninstall existing");
    for relative in [
        CLAUDE_MCP_PATH,
        CURSOR_MCP_PATH,
        CODEX_CONFIG_PATH,
        AGENT_GUIDE_PATH,
    ] {
        assert!(
            existing.path().join(relative).is_file(),
            "{relative} preserved"
        );
    }
}

#[test]
fn targeted_uninstall_keeps_shared_and_other_client_entries() {
    let dir = TempDir::new().expect("tempdir");
    install(dir.path(), "demo", &[ClientTarget::All], false).expect("install");

    let report = uninstall(dir.path(), &[ClientTarget::Claude], false).expect("uninstall");

    assert_eq!(report.clients, vec![ClientTarget::Claude]);
    assert!(!dir.path().join(CLAUDE_MCP_PATH).exists());
    assert!(dir.path().join(CURSOR_MCP_PATH).is_file());
    assert!(dir.path().join(CODEX_CONFIG_PATH).is_file());
    assert!(dir.path().join(MCP_MANIFEST_PATH).is_file());
    assert!(dir.path().join(crate::hooks::HOOKS_MANIFEST_PATH).is_file());
    assert!(dir.path().join(AGENT_GUIDE_PATH).is_file());
}

#[test]
fn json_merge_preserves_user_key_order() {
    let dir = TempDir::new().expect("tempdir");
    std::fs::write(
        dir.path().join(CLAUDE_MCP_PATH),
        r#"{"zeta":1,"alpha":2,"mcpServers":{"other":{"command":"other"}}}"#,
    )
    .expect("config");

    install(dir.path(), "demo", &[ClientTarget::Claude], false).expect("install");

    let merged = std::fs::read_to_string(dir.path().join(CLAUDE_MCP_PATH)).expect("merged");
    let zeta = merged.find("\"zeta\"").expect("zeta");
    let alpha = merged.find("\"alpha\"").expect("alpha");
    let servers = merged.find("\"mcpServers\"").expect("servers");
    assert!(zeta < alpha && alpha < servers);
}

#[test]
fn uninstall_conflict_is_detected_before_any_removal() {
    let dir = TempDir::new().expect("tempdir");
    install(dir.path(), "demo", &[ClientTarget::All], false).expect("install");
    std::fs::write(
        dir.path().join(CURSOR_MCP_PATH),
        r#"{"mcpServers":{"texo":{"command":"other"}}}"#,
    )
    .expect("conflict");
    let before = fingerprint_files(dir.path());

    let error = uninstall(dir.path(), &[], false).expect_err("conflict");

    assert!(error.to_string().contains("not managed"));
    assert_eq!(fingerprint_files(dir.path()), before);
}

#[cfg(unix)]
#[test]
fn install_refuses_symlinked_client_paths_and_preserves_permissions() {
    use std::os::unix::fs::{symlink, PermissionsExt as _};

    let linked = TempDir::new().expect("linked root");
    let outside = TempDir::new().expect("outside");
    symlink(outside.path(), linked.path().join(".cursor")).expect("symlink");
    let before = fingerprint_files(outside.path());
    let error =
        install(linked.path(), "demo", &[ClientTarget::All], false).expect_err("symlink must fail");
    assert!(error.to_string().contains("symbolic link"));
    assert_eq!(fingerprint_files(outside.path()), before);
    assert!(!linked.path().join(".texo").exists());

    let permissions = TempDir::new().expect("permissions root");
    std::fs::write(permissions.path().join(CLAUDE_MCP_PATH), "{}\n").expect("config");
    let mut mode = std::fs::metadata(permissions.path().join(CLAUDE_MCP_PATH))
        .expect("metadata")
        .permissions();
    mode.set_mode(0o600);
    std::fs::set_permissions(permissions.path().join(CLAUDE_MCP_PATH), mode).expect("permissions");
    install(permissions.path(), "demo", &[ClientTarget::Claude], false).expect("install");
    assert_eq!(
        std::fs::metadata(permissions.path().join(CLAUDE_MCP_PATH))
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

fn fingerprint_files(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut rows = walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| {
            (
                entry
                    .path()
                    .strip_prefix(root)
                    .expect("relative")
                    .to_string_lossy()
                    .to_string(),
                std::fs::read(entry.path()).expect("read"),
            )
        })
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| left.0.cmp(&right.0));
    rows
}
