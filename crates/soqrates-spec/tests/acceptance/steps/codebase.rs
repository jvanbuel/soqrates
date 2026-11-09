//! Steps for `codebase.feature` (ADR-0008): which code each examiner step
//! read, as recorded by the fake examiner.

use cucumber::{then, when};
use serde_json::json;

use super::quoted;
use crate::fakes::{ExaminerStep, diff_touching};
use crate::world::SpecWorld;

fn step(what: &str) -> ExaminerStep {
    match what {
        "find the concepts" => ExaminerStep::Extract,
        "write the questions" => ExaminerStep::Write,
        "grade the answers" => ExaminerStep::Grade,
        other => panic!("unknown examiner step {other:?}"),
    }
}

fn read_by(w: &SpecWorld, what: &str) -> Vec<String> {
    let want = step(what);
    w.examiner
        .reads()
        .into_iter()
        .filter(|(s, _)| *s == want)
        .map(|(_, text)| text)
        .collect()
}

#[then(
    regex = r#"^the examiner read "([^"]+)" to (find the concepts|write the questions|grade the answers)$"#
)]
fn read(w: &mut SpecWorld, text: String, what: String) {
    let reads = read_by(w, &what);
    assert!(
        reads.contains(&text),
        "the examiner did not read {text:?} to {what}; it read {reads:?}"
    );
}

#[then(
    regex = r#"^the examiner last read "([^"]+)" to (find the concepts|write the questions|grade the answers)$"#
)]
fn last_read(w: &mut SpecWorld, text: String, what: String) {
    let reads = read_by(w, &what);
    assert_eq!(reads.last(), Some(&text), "it read {reads:?}");
}

#[then(
    regex = r#"^the examiner never read "([^"]+)" to (find the concepts|write the questions|grade the answers)$"#
)]
fn never_read(w: &mut SpecWorld, text: String, what: String) {
    let reads = read_by(w, &what);
    assert!(!reads.contains(&text), "it read {reads:?}");
}

#[then("the examiner read no code")]
fn read_nothing(w: &mut SpecWorld) {
    assert_eq!(w.examiner.reads(), Vec::new());
}

#[when(
    regex = r#"^an MCP client starts and answers a session for "([^"]+)" on a change touching (.+)$"#
)]
async fn mcp_session(w: &mut SpecWorld, id: String, list: String) {
    let diff = diff_touching(&quoted(&list));
    let started = w
        .call_mcp("start_session", json!({ "author": id, "diff": diff.0 }))
        .await
        .expect("start_session");
    let questions = started["questions"].as_array().expect("questions");
    assert!(!questions.is_empty(), "no questions were asked");
    let answers: Vec<_> = questions
        .iter()
        .map(|q| match q["format"].as_str() {
            Some("MultipleChoice") => json!({ "question": q["id"], "choice": 0 }),
            _ => json!({ "question": q["id"], "text": "correct" }),
        })
        .collect();
    w.call_mcp(
        "submit_answers",
        json!({ "session_key": started["session_key"], "answers": answers }),
    )
    .await
    .expect("submit_answers");
}
