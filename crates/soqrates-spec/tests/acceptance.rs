//! BDD acceptance tests for Soqrates (ADR-0001 D5-D10, D12, D15).
//!
//! Scenarios live in `crates/soqrates-spec/features/`. The run is a ratchet
//! (ADR-0001 D15):
//!
//! - By default only scenarios *without* `@wip` run, and any failing,
//!   skipped or undefined step fails the build.
//! - With `SOQRATES_SPEC_ALL=1` every scenario runs, which shows the red/green
//!   state of the `@wip` backlog.
//!
//! All ports are played by hermetic fakes (ADR-0001 D12, `// @adr-0001-d12`).

#[path = "acceptance/fakes.rs"]
mod fakes;
#[path = "acceptance/steps/mod.rs"]
mod steps;
#[path = "acceptance/world.rs"]
mod world;

use cucumber::World as _;

use world::SpecWorld;

const WIP: &str = "wip";

fn run_all() -> bool {
    std::env::var("SOQRATES_SPEC_ALL").is_ok_and(|v| v == "1")
}

#[tokio::main]
async fn main() {
    let all = run_all();
    SpecWorld::cucumber()
        .fail_on_skipped()
        .filter_run_and_exit(
            concat!(env!("CARGO_MANIFEST_DIR"), "/features"),
            move |feature, rule, scenario| {
                let wip = feature
                    .tags
                    .iter()
                    .chain(rule.iter().flat_map(|r| r.tags.iter()))
                    .chain(scenario.tags.iter())
                    .any(|t| t == WIP);
                all || !wip
            },
        )
        .await;
}
