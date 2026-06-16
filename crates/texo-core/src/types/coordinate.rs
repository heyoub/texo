//! BatPak coordinate builders for texo entity streams.

/// Coordinate scope for a workspace.
pub fn scope_for_workspace(workspace_id: &str) -> String {
    format!("workspace:{workspace_id}")
}

/// Entity string for a source stream.
pub fn entity_for_source(source_id: &str) -> String {
    format!("source:{source_id}")
}

/// Entity string for a claim stream.
pub fn entity_for_claim(claim_id: &str) -> String {
    format!("claim:{claim_id}")
}

/// Entity string for a conflict stream.
pub fn entity_for_conflict(conflict_id: &str) -> String {
    format!("conflict:{conflict_id}")
}

/// Entity string for a projection stream.
pub fn entity_for_projection(name: &str) -> String {
    format!("projection:{name}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_and_entity_builders_use_the_expected_prefixes() {
        // These prefixes are load-bearing: replay scopes events by
        // `workspace:{id}` and BatPak entity streams are keyed by these strings.
        // A drift here would silently split or merge event streams.
        assert_eq!(scope_for_workspace("demo"), "workspace:demo");
        assert_eq!(entity_for_source("src_abc"), "source:src_abc");
        assert_eq!(entity_for_claim("claim_abc"), "claim:claim_abc");
        assert_eq!(entity_for_conflict("conflict_abc"), "conflict:conflict_abc");
        assert_eq!(entity_for_projection("onboarding"), "projection:onboarding");
    }
}
