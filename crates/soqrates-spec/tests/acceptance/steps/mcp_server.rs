//! Steps for `mcp_server.feature` (ADR-0001 D10).

use std::collections::BTreeSet;

use cucumber::gherkin::Step;
use cucumber::{then, when};
use serde_json::json;
use soqrates_core::AuthorId;

use crate::world::SpecWorld;

#[when("I list the MCP tools")]
fn list_tools(w: &mut SpecWorld) {
    w.tools = soqrates_mcp::tool_names()
        .into_iter()
        .map(str::to_string)
        .collect();
}

#[then("the MCP tools are exactly:")]
fn tools_exactly(w: &mut SpecWorld, step: &Step) {
    let table = step.table.as_ref().expect("this step needs a data table");
    let want: BTreeSet<String> = table.rows.iter().skip(1).map(|r| r[0].clone()).collect();
    let have: BTreeSet<String> = w.tools.iter().cloned().collect();
    assert_eq!(have, want);
}

#[then("no MCP tool is listed twice")]
fn no_duplicates(w: &mut SpecWorld) {
    let unique: BTreeSet<_> = w.tools.iter().collect();
    assert_eq!(unique.len(), w.tools.len(), "tools: {:?}", w.tools);
}

#[then(regex = r#"^the MCP server exposes the tool "([^"]+)"$"#)]
fn exposes(w: &mut SpecWorld, tool: String) {
    assert!(w.tools.contains(&tool), "{tool} missing from {:?}", w.tools);
}

// G1 (resolved): tools are called in-process with `SoqratesMcp::call_tool`
// against the world's engine. See API_GAPS.md.

#[when(regex = r#"^an MCP client calls "([^"]+)" for "([^"]+)"$"#)]
async fn mcp_call(w: &mut SpecWorld, tool: String, id: String) {
    let result = w.call_mcp(&tool, json!({ "author": id })).await;
    w.mcp_result = Some(result);
}

#[then(regex = r#"^the MCP result matches the engine's profile of "([^"]+)"$"#)]
async fn mcp_matches(w: &mut SpecWorld, id: String) {
    let result = match w.mcp_result.take().expect("no MCP call in this scenario") {
        Ok(v) => v,
        Err(e) => panic!("MCP call failed: {e}"),
    };
    let profile = w.profile(&AuthorId(id)).await;
    let cards: Vec<_> = profile
        .cards()
        .iter()
        .map(|(concept, card)| {
            json!({
                "concept": concept.0,
                "due": card.due().to_rfc3339(),
                "reps": card.reps(),
            })
        })
        .collect();
    assert!(!cards.is_empty(), "the engine's profile has no reviews");
    let want = json!({
        "level": serde_json::to_value(profile.level()).unwrap(),
        "cards": cards,
        "reviews": profile.history().len(),
    });
    assert_eq!(result, want);
}
