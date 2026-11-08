//! Steps for `answer_commands.feature` (ADR-0004 D8, D9): answer comments as
//! text, read with the forgiving answer command, hints for mistyped commands
//! and the acknowledgement of submitted answers, on the fake forge (ADR-0001
//! D12).

use cucumber::gherkin::Step;
use cucumber::{given, then, when};
use soqrates_core::{QuestionBody, Session};

use super::correctness;
use crate::fakes::{CORRECT_TEXT, Reaction, Submitter, WRONG_TEXT};
use crate::world::SpecWorld;

fn latest_posted(w: &SpecWorld) -> Session {
    w.scm
        .posted_on(w.current_pr())
        .pop()
        .expect("no questions were posted on the pull request")
}

/// `command`, then one numbered line per question of `session`, answered as
/// the answer key says is correct or wrong.
fn answer_comment(session: &Session, command: &str, correct: bool) -> String {
    let mut body = command.to_string();
    for (i, q) in session.questions().iter().enumerate() {
        let text = match (&q.body, correct) {
            // The fake examiner's option a is always the correct one.
            (QuestionBody::MultipleChoice { .. }, true) => "a",
            (QuestionBody::MultipleChoice { .. }, false) => "b",
            (QuestionBody::Open { .. }, true) => CORRECT_TEXT,
            (QuestionBody::Open { .. }, false) => WRONG_TEXT,
        };
        body.push_str(&format!("\n{}. {text}", i + 1));
    }
    body
}

/// The id of the latest comment posted in the scenario.
fn that_comment(w: &SpecWorld) -> u64 {
    w.scm
        .state
        .lock()
        .unwrap()
        .comments
        .last()
        .expect("no comment was posted")
        .id
}

#[given(regex = r#"^Soqrates posts as the app "([^"]+)"$"#)]
fn posts_as_app(w: &mut SpecWorld, slug: String) {
    w.scm.state.lock().unwrap().app_slug = Some(slug);
}

#[given(regex = r#"^the author comments "([^"]*)" with (correct|wrong) answers$"#)]
#[when(regex = r#"^the author comments "([^"]*)" with (correct|wrong) answers$"#)]
fn comments(w: &mut SpecWorld, command: String, how: String) {
    let session = latest_posted(w);
    let body = answer_comment(&session, &command, correctness(&how));
    let pr = w.current_pr().clone();
    w.scm.comment(&pr, session.key(), session.author(), &body);
}

#[when(regex = r#"^the author edits that comment to "([^"]*)" with (correct|wrong) answers$"#)]
fn edits(w: &mut SpecWorld, command: String, how: String) {
    let session = latest_posted(w);
    let body = answer_comment(&session, &command, correctness(&how));
    let id = that_comment(w);
    w.scm.edit_comment(id, &body);
}

#[then(regex = r"^(\d+) hints? (?:was|were) posted$")]
fn hints_posted(w: &mut SpecWorld, n: usize) {
    assert_eq!(w.scm.state.lock().unwrap().hints.len(), n);
}

#[then(regex = r"^the posted results have (\d+) correct answers?$")]
fn results_correct(w: &mut SpecWorld, n: usize) {
    let state = w.scm.state.lock().unwrap();
    let (_, _, outcome) = state.results.last().expect("no results posted");
    let correct = outcome.results.iter().filter(|r| r.is_correct()).count();
    assert_eq!(correct, n, "results: {:?}", outcome.results);
}

/// A row of "the author saw, in order" as the start of a timeline entry.
fn timeline_entry(w: &SpecWorld, row: &str) -> String {
    let comment = Submitter::Comment(that_comment(w));
    match row {
        "👀 on the answer comment" => format!("{comment:?}: {:?}", Reaction::Eyes),
        "✅ on the answer comment" => format!("{comment:?}: {:?}", Reaction::Done),
        other => other.to_string(),
    }
}

#[then("the author saw, in order:")]
fn saw_in_order(w: &mut SpecWorld, step: &Step) {
    let table = step.table.as_ref().expect("this step needs a data table");
    let expected: Vec<String> = table
        .rows
        .iter()
        .skip(1)
        .map(|r| timeline_entry(w, &r[0]))
        .collect();
    let timeline = w.scm.state.lock().unwrap().timeline.clone();
    let mut entries = timeline.iter();
    for want in &expected {
        assert!(
            entries.any(|e| e.starts_with(want.as_str())),
            "expected {want:?}, in order {expected:?}, in the timeline {timeline:?}"
        );
    }
}

fn reactions_on_that_comment(w: &SpecWorld) -> Vec<Reaction> {
    let comment = Submitter::Comment(that_comment(w));
    w.scm
        .state
        .lock()
        .unwrap()
        .reactions
        .iter()
        .filter(|(s, _)| *s == comment)
        .map(|(_, r)| *r)
        .collect()
}

#[then("the answer comment shows only ✅")]
fn only_done(w: &mut SpecWorld) {
    assert_eq!(reactions_on_that_comment(w), vec![Reaction::Done]);
}

#[then("the answer comment shows no reaction")]
fn no_reaction(w: &mut SpecWorld) {
    assert_eq!(reactions_on_that_comment(w), vec![]);
}

#[then(regex = r#"^the check never said "([^"]+)"$"#)]
fn never_said(w: &mut SpecWorld, summary: String) {
    let state = w.scm.state.lock().unwrap();
    assert!(
        !state.summaries.iter().any(|(_, s)| *s == summary),
        "summaries: {:?}",
        state.summaries
    );
}
