//! Ignored live Helios evaluation gate.

#[test]
#[ignore = "requires TEXO_LLM_API_KEY and live semantic orchestration"]
fn helios_live_eval_is_key_gated() {
    let Ok(key) = std::env::var("TEXO_LLM_API_KEY") else {
        return;
    };
    assert!(!key.trim().is_empty());
}
