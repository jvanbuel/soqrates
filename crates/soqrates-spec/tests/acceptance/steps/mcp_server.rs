//! Steps for `mcp_server.feature` (ADR-0001 D10).

use std::collections::BTreeSet;

use cucumber::gherkin::Step;
use cucumber::{then, when};

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

// Blocked: soqrates-mcp has no in-process way to call a tool against a given
// engine. See API_GAPS.md (G1).

#[when(regex = r#"^an MCP client calls "([^"]+)" for "([^"]+)"$"#)]
fn mcp_call(_w: &mut SpecWorld, tool: String, id: String) {
    panic!(
        "blocked by API gap G1 (see crates/soqrates-spec/API_GAPS.md): \
         cannot call MCP tool {tool:?} for {id:?} in-process"
    );
}

#[then(regex = r#"^the MCP result matches the engine's profile of "([^"]+)"$"#)]
fn mcp_matches(_w: &mut SpecWorld, id: String) {
    panic!("blocked by API gap G1 (see crates/soqrates-spec/API_GAPS.md): profile of {id:?}");
}
