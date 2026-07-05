//! Ignored live Helios evaluation placeholder.

#[test]
#[ignore = "requires OPENROUTER_API_KEY and WO-4 semantic orchestration"]
fn helios_live_eval_is_key_gated() {
    let Ok(key) = std::env::var("OPENROUTER_API_KEY") else {
        return;
    };
    assert!(!key.trim().is_empty());
}
