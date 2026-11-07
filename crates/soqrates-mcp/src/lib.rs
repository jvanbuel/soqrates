//! MCP server exposing the Soqrates core through `rmcp` (ADR-0001 D10).

/// Names of the MCP tools this server exposes. ADR-0001 D10 fixes this set.
pub fn tool_names() -> Vec<&'static str> {
    vec![
        "start_session",
        "submit_answers",
        "get_learner_profile",
        "due_reviews",
    ]
}

/// Serve MCP over stdio.
pub async fn serve_stdio() -> soqrates_core::Result<()> {
    todo!("ADR-0001 D10")
}
