use super::*;

fn test_role(role: ModelRole, model: &str) -> ResolvedRole {
    resolve_role(
        role,
        &RoleOverrides {
            api_key: Some("test-key".to_string()),
            model: Some(model.to_string()),
            ..RoleOverrides::default()
        },
        None,
    )
}

// --- request-body construction ---

#[test]
fn embeddings_request_has_model_and_input_array() {
    let body = build_embeddings_request("google/gemini-embedding-2", &["hello", "world"]);
    assert_eq!(body["model"], "google/gemini-embedding-2");
    assert_eq!(body["input"], json!(["hello", "world"]));
}

#[test]
fn default_provider_fingerprints_remain_byte_stable() {
    let relate_role = test_role(ModelRole::Relate, "relation/model");
    let relate_client = OpenRouterClient::from_role(&relate_role).expect("client");
    let relater = OpenRouterRelater {
        client: relate_client,
        role: relate_role,
    };
    assert_eq!(
        relater.fingerprint(),
        "openrouter:relation/model|relation-v2"
    );

    let propose_role = test_role(ModelRole::Propose, "propose/model");
    let propose_client = OpenRouterClient::from_role(&propose_role).expect("client");
    let proposer = OpenRouterProposer {
        client: propose_client,
        role: propose_role,
    };
    assert_eq!(
        proposer.fingerprint(),
        "openrouter:propose/model|propose-v3"
    );
}

// --- embeddings parsing ---

#[test]
fn embeddings_response_parses_in_request_order() {
    let value = json!({
        "data": [
            { "index": 1, "embedding": [0.1, 0.2] },
            { "index": 0, "embedding": [0.3, 0.4] }
        ]
    });
    let vectors = parse_embeddings_response(&value, 2).expect("parse");
    // Sorted by `index`: row 0 first.
    assert_eq!(vectors, vec![vec![0.3, 0.4], vec![0.1, 0.2]]);
}

#[test]
fn embeddings_response_without_index_keeps_array_order() {
    let value = json!({
        "data": [
            { "embedding": [1.0] },
            { "embedding": [2.0] }
        ]
    });
    let vectors = parse_embeddings_response(&value, 2).expect("parse");
    assert_eq!(vectors, vec![vec![1.0], vec![2.0]]);
}

#[test]
fn embeddings_response_wrong_count_is_error() {
    let value = json!({ "data": [ { "embedding": [1.0] } ] });
    let err = parse_embeddings_response(&value, 2).expect_err("count mismatch");
    assert!(matches!(err, BackendError::UnexpectedResponse { .. }));
}

#[test]
fn embeddings_response_empty_vector_is_error() {
    let value = json!({ "data": [ { "embedding": [] } ] });
    let err = parse_embeddings_response(&value, 1).expect_err("empty vector");
    assert!(matches!(err, BackendError::UnexpectedResponse { .. }));
}

#[test]
fn embeddings_response_bad_shape_is_parse_error() {
    let value = json!({ "data": [ { "embedding": "not-a-vector" } ] });
    let err = parse_embeddings_response(&value, 1).expect_err("parse error");
    assert!(matches!(err, BackendError::Parse { .. }));
}

/// Helper that wraps a judge content string in a chat-completions envelope.
fn chat_envelope(content: &str) -> Value {
    json!({ "choices": [ { "message": { "role": "assistant", "content": content } } ] })
}

fn chat_envelope_with_reason(content: &str, finish_reason: &str) -> Value {
    json!({
        "choices": [{
            "finish_reason": finish_reason,
            "message": { "role": "assistant", "content": content }
        }]
    })
}

#[test]
fn positive_length_finish_is_truncated_but_unknown_reason_is_preserved() {
    let truncated = parse_relation_response(&chat_envelope_with_reason("", "length"), 4096)
        .expect_err("positive truncation");
    assert!(matches!(
        truncated,
        BackendError::Truncated {
            finish_reason,
            max_tokens: 4096,
            ..
        } if finish_reason == "length"
    ));

    let unknown = parse_relation_response(&chat_envelope_with_reason("", "weird_variant"), 4096)
        .expect_err("unknown finish reason is malformed, not truncation");
    assert!(matches!(unknown, BackendError::UnexpectedResponse { .. }));
    assert!(unknown.to_string().contains("weird_variant"));
}
// --- claim-relation parsing + label mapping ---

#[test]
fn relation_request_carries_prompt_and_strict_format() {
    let role = test_role(ModelRole::Relate, "nvidia/nemotron-3-ultra-550b-a55b");
    let body = build_relation_request(&role, "older X", "newer Y");
    assert_eq!(body["model"], "nvidia/nemotron-3-ultra-550b-a55b");
    assert_eq!(body["temperature"], json!(0.0));
    assert_eq!(
        body["response_format"]["json_schema"]["name"],
        "claim_relation"
    );
    assert_eq!(body["max_tokens"], 4096);
    let messages = body["messages"].as_array().expect("messages array");
    assert_eq!(messages.len(), 2);
    let user = messages[1]["content"].as_str().expect("user content");
    assert!(user.contains("older X"));
    assert!(user.contains("newer Y"));
    // Order must be conveyed: older is labeled first, newer second.
    assert!(user.find("older X") < user.find("newer Y"));
}

#[test]
fn provider_capability_suppresses_unsupported_response_format() {
    let mut role = test_role(ModelRole::Relate, "provider/model");
    role.profile.strict_json_schema_ok = false;
    let body = build_relation_request(&role, "older", "newer");
    assert!(body.get("response_format").is_none());
}

#[test]
fn relation_label_strings_map_to_claim_relation() {
    assert_eq!(
        parse_relation_label("SUPERSEDES"),
        Some(ClaimRelation::Supersedes)
    );
    assert_eq!(
        parse_relation_label(" conflict "),
        Some(ClaimRelation::Conflict)
    );
    assert_eq!(
        parse_relation_label("duplicate"),
        Some(ClaimRelation::Duplicate)
    );
    assert_eq!(
        parse_relation_label("unrelated"),
        Some(ClaimRelation::Unrelated)
    );
    assert_eq!(parse_relation_label("nonsense"), None);
}

#[test]
fn relation_response_parses_clean_json() {
    let value = chat_envelope("{\"relation\": \"supersedes\", \"score\": 0.88}");
    let verdict = parse_relation_response(&value, 4096).expect("parse");
    assert_eq!(verdict.relation, ClaimRelation::Supersedes);
    assert!((verdict.score - 0.88).abs() < 1e-6);
}

#[test]
fn relation_response_tolerates_fences_and_clamps_score() {
    let value = chat_envelope("```json\n{\"relation\": \"conflict\", \"score\": 1.4}\n```");
    let verdict = parse_relation_response(&value, 4096).expect("parse");
    assert_eq!(verdict.relation, ClaimRelation::Conflict);
    assert!((verdict.score - 1.0).abs() < 1e-6);
}

#[test]
fn relation_response_unknown_label_is_error() {
    let value = chat_envelope("{\"relation\": \"maybe\", \"score\": 0.5}");
    let err = parse_relation_response(&value, 4096).expect_err("unknown relation");
    assert!(matches!(err, BackendError::UnexpectedResponse { .. }));
}

#[test]
fn relation_response_missing_score_is_parse_error() {
    let value = chat_envelope("{\"relation\": \"unrelated\"}");
    let err = parse_relation_response(&value, 4096).expect_err("missing score");
    assert!(matches!(err, BackendError::Parse { .. }));
}

// --- proposer parsing ---

#[test]
fn propose_request_includes_heading_context_and_schema() {
    let role = test_role(ModelRole::Propose, "anthropic/claude-opus-4.8");
    let body = build_propose_request(
        &role,
        "Deploys moved to Tuesday.",
        &["Operations".to_owned(), "Deploys".to_owned()],
    );
    assert_eq!(body["max_tokens"], 2048);
    let user = body["messages"][1]["content"].as_str().expect("user");
    assert!(user.contains("Operations > Deploys"), "heading path joined");
    assert!(user.contains("Deploys moved to Tuesday."));
}

#[test]
fn propose_request_without_headings_has_no_section_line() {
    let role = test_role(ModelRole::Propose, "m");
    let body = build_propose_request(&role, "Some span.", &[]);
    let user = body["messages"][1]["content"].as_str().expect("user");
    assert!(!user.contains("Section:"));
    assert!(user.starts_with("Span:"));
}

#[test]
fn propose_response_parses_claims_and_rescales_confidence() {
    let value = chat_envelope(
        "{\"claims\":[{\"text\":\"Deploys moved to Tuesday.\",\"subject\":\"deploys\",\"predicate\":\"scheduled\",\"object\":\"Tuesday\",\"confidence\":90}]}",
    );
    let claims = parse_propose_response(&value, 2048).expect("parse");
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].text, "Deploys moved to Tuesday.");
    assert_eq!(claims[0].subject, "deploys");
    assert_eq!(claims[0].confidence_ppm, 900_000);
}

#[test]
fn propose_response_drops_blank_text_and_clamps_confidence() {
    let value = chat_envelope(
        "{\"claims\":[{\"text\":\"   \",\"subject\":\"\",\"predicate\":\"\",\"object\":\"\",\"confidence\":50},{\"text\":\"Real claim.\",\"subject\":\"x\",\"predicate\":\"y\",\"object\":\"z\",\"confidence\":170}]}",
    );
    let claims = parse_propose_response(&value, 2048).expect("parse");
    assert_eq!(claims.len(), 1, "blank-text claim dropped");
    assert_eq!(claims[0].text, "Real claim.");
    assert_eq!(
        claims[0].confidence_ppm, 1_000_000,
        "over-100 confidence clamped"
    );
}

#[test]
fn propose_response_empty_list_is_ok() {
    let value = chat_envelope("{\"claims\":[]}");
    let claims = parse_propose_response(&value, 2048).expect("parse");
    assert!(claims.is_empty());
}

#[test]
fn propose_response_tolerates_fences() {
    let value = chat_envelope(
        "```json\n{\"claims\":[{\"text\":\"A.\",\"subject\":\"a\",\"predicate\":\"b\",\"object\":\"c\",\"confidence\":40}]}\n```",
    );
    let claims = parse_propose_response(&value, 2048).expect("parse");
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].confidence_ppm, 400_000);
}

#[test]
fn propose_response_missing_claims_key_is_parse_error() {
    let value = chat_envelope("{\"items\":[]}");
    let err = parse_propose_response(&value, 2048).expect_err("missing claims");
    assert!(matches!(err, BackendError::Parse { .. }));
}

/// PROVES: the chat-completions response parsers are TOTAL functions on
/// arbitrary model output — they return `Ok` or a typed `Err`, never panic —
/// and when `Ok`, scores are clamped to `[0,1]` and confidence to `0..=1e6`.
/// The cargo-fuzz substitute for the judge/proposer parsers.
mod robustness {
    use super::super::{parse_propose_response, parse_relation_response};
    use super::chat_envelope;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]

        #[test]
        fn relation_parser_is_total(content in any::<String>()) {
            if let Ok(v) = parse_relation_response(&chat_envelope(&content), 4096) {
                prop_assert!((0.0..=1.0).contains(&v.score));
            }
        }

        #[test]
        fn propose_parser_is_total(content in any::<String>()) {
            if let Ok(claims) = parse_propose_response(&chat_envelope(&content), 2048) {
                for c in claims {
                    prop_assert!(c.confidence_ppm <= 1_000_000);
                    prop_assert!(!c.text.trim().is_empty());
                }
            }
        }
    }
}
