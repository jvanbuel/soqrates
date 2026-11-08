//! Steps for `local_hooks.feature` (ADR-0001 D9).

use cucumber::{given, then, when};
use soqrates_core::hook::run_hook;
use soqrates_core::{ConceptId, Diff, HookDecision};

use super::{author, concepts, correctness, hook_kind, quoted};
use crate::fakes::diff_touching;
use crate::world::{HookRun, SpecWorld};

async fn run(w: &mut SpecWorld, id: &str, hook: &str, diff: Diff) {
    let kind = hook_kind(hook);
    w.prompter.asked.clear();
    w.prompter.reported.clear();
    let decision = run_hook(
        &w.app,
        &mut w.prompter,
        kind,
        &author(id),
        &diff,
        &crate::fakes::FakeCodebase(format!("what the {hook} hook is about")),
    )
    .await
    .expect("run_hook");
    w.hook_runs.push(HookRun {
        kind,
        decision,
        asked: std::mem::take(&mut w.prompter.asked),
        reported: std::mem::take(&mut w.prompter.reported),
    });
}

fn last_run(w: &SpecWorld) -> &HookRun {
    w.hook_runs.last().expect("no hook has run")
}

#[given(regex = r"^the author will answer (correctly|wrongly) at the terminal$")]
fn will_answer(w: &mut SpecWorld, how: String) {
    w.prompter.answer_correctly = correctness(&how);
}

#[given(
    regex = r#"^"([^"]+)" (?:runs|ran) the (pre-commit|pre-push) hook on a change touching (.+)$"#
)]
#[when(
    regex = r#"^"([^"]+)" (?:runs|ran) the (pre-commit|pre-push) hook on a change touching (.+)$"#
)]
async fn runs_hook(w: &mut SpecWorld, id: String, hook: String, list: String) {
    run(w, &id, &hook, diff_touching(&quoted(&list))).await;
}

#[when(
    regex = r#"^"([^"]+)" runs the (pre-commit|pre-push) hook on (an empty change|a change with only whitespace)$"#
)]
async fn runs_hook_empty(w: &mut SpecWorld, id: String, hook: String, change: String) {
    let diff = match change.as_str() {
        "an empty change" => Diff(String::new()),
        _ => Diff(" \n\t\n  \n".to_string()),
    };
    assert!(diff.is_empty());
    run(w, &id, &hook, diff).await;
}

#[then(regex = r"^the hook (allows|blocks) it$")]
fn decision(w: &mut SpecWorld, d: String) {
    let want = if d == "allows" {
        HookDecision::Allow
    } else {
        HookDecision::Block
    };
    let r = last_run(w);
    assert_eq!(r.decision, want, "{:?} hook decision", r.kind);
}

#[then(regex = r"^the author was asked (\d+) questions? at the terminal$")]
fn asked_n(w: &mut SpecWorld, n: usize) {
    assert_eq!(last_run(w).asked.len(), n, "asked: {:?}", last_run(w).asked);
}

#[then(regex = r"^the author was asked about (.+), in that order$")]
fn asked_about(w: &mut SpecWorld, list: String) {
    let asked: Vec<ConceptId> = last_run(w)
        .asked
        .iter()
        .map(|q| q.concept.clone())
        .collect();
    assert_eq!(asked, concepts(&list));
}

#[then(regex = r"^a (passed|failed) result was reported to the author$")]
fn reported(w: &mut SpecWorld, how: String) {
    let r = last_run(w);
    assert_eq!(r.reported.len(), 1, "reports: {:?}", r.reported);
    assert_eq!(r.reported[0].passed, how == "passed");
}

fn last_two(w: &SpecWorld) -> (&HookRun, &HookRun) {
    match w.hook_runs.as_slice() {
        [.., a, b] => (a, b),
        _ => panic!("two hook runs are needed"),
    }
}

#[then("both hooks made the same decision")]
fn same_decision(w: &mut SpecWorld) {
    let (a, b) = last_two(w);
    assert_ne!(a.kind, b.kind, "compare two different hooks");
    assert_eq!(a.decision, b.decision);
}

#[then("both hooks asked the same questions")]
fn same_questions(w: &mut SpecWorld) {
    let (a, b) = last_two(w);
    assert!(!a.asked.is_empty(), "nothing was asked");
    assert_eq!(a.asked, b.asked);
}
