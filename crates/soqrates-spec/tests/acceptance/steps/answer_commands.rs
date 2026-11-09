//! Steps for `answer_commands.feature` and `answer_fields.feature` (ADR-0004
//! D8, D9, D11): answer comments as text, read with the forgiving answer
//! command, hints for mistyped commands, answers typed into the question
//! comment and the acknowledgement of submitted answers, on the fake forge
//! (ADR-0001 D12).

use cucumber::gherkin::Step;
use cucumber::{given, then, when};
use soqrates_core::{QuestionBody, Session};

use super::{author, correctness};
use crate::fakes::{CORRECT_TEXT, FakeExaminer, Reaction, Submitter, WRONG_TEXT};
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
    let on = |submitter: Submitter, reaction: Reaction| format!("{submitter:?}: {reaction:?}");
    match row {
        "👀 on the answer comment" => on(Submitter::Comment(that_comment(w)), Reaction::Eyes),
        "✅ on the answer comment" => on(Submitter::Comment(that_comment(w)), Reaction::Done),
        "👀 on the question comment" => on(Submitter::QuestionComment, Reaction::Eyes),
        "✅ on the question comment" => on(Submitter::QuestionComment, Reaction::Done),
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

fn reactions_on(w: &SpecWorld, submitter: Submitter) -> Vec<Reaction> {
    w.scm
        .state
        .lock()
        .unwrap()
        .reactions
        .iter()
        .filter(|(s, _)| *s == submitter)
        .map(|(_, r)| *r)
        .collect()
}

/// The answer comment (the latest comment) or the question comment.
fn submitter(w: &SpecWorld, which: &str) -> Submitter {
    match which {
        "answer" => Submitter::Comment(that_comment(w)),
        "question" => Submitter::QuestionComment,
        other => panic!("expected answer or question, got {other:?}"),
    }
}

#[then(regex = r"^the (answer|question) comment shows only ✅$")]
fn only_done(w: &mut SpecWorld, which: String) {
    assert_eq!(reactions_on(w, submitter(w, &which)), vec![Reaction::Done]);
}

#[then(regex = r"^the (answer|question) comment shows no reaction$")]
fn no_reaction(w: &mut SpecWorld, which: String) {
    assert_eq!(reactions_on(w, submitter(w, &which)), vec![]);
}

// ---------------------------------------------------------------------------
// Answering inside the question comment (ADR-0004 D11)

/// `by` edits the question comment of the latest session: ticks options and
/// types open answers into the answer fields, as the answer key says is
/// correct or wrong, and ticks "Grade my answers" or not.
#[given(
    regex = r#"^(the author|"[^"]+") types (correct|wrong) answers into the question comment (and ticks|without ticking) "Grade my answers"$"#
)]
#[when(
    regex = r#"^(the author|"[^"]+") types (correct|wrong) answers into the question comment (and ticks|without ticking) "Grade my answers"$"#
)]
fn types_into_question_comment(w: &mut SpecWorld, who: String, how: String, done: String) {
    let session = latest_posted(w);
    let by = match who.as_str() {
        "the author" => session.author().clone(),
        quoted => author(quoted.trim_matches('"')),
    };
    let answers = session
        .questions()
        .iter()
        .map(|q| FakeExaminer::answer(q, correctness(&how)))
        .collect();
    w.scm
        .edit_question_comment(session.key(), &by, answers, done == "and ticks");
}

#[given(
    regex = r#"^the author comments "([^"]*)" answering only question (\d+) (correctly|wrongly)$"#
)]
#[when(
    regex = r#"^the author comments "([^"]*)" answering only question (\d+) (correctly|wrongly)$"#
)]
fn comments_one(w: &mut SpecWorld, command: String, n: usize, how: String) {
    let session = latest_posted(w);
    let full = answer_comment(&session, &command, correctness(&how));
    // The command line, then only the line of question `n`.
    let mut lines = full.lines();
    let mut body = lines.next().unwrap_or_default().to_string();
    let line = lines
        .find(|l| l.starts_with(&format!("{n}. ")))
        .unwrap_or_else(|| panic!("no question {n}"));
    body.push('\n');
    body.push_str(line);
    let pr = w.current_pr().clone();
    w.scm.comment(&pr, session.key(), session.author(), &body);
}

#[then("every posted question is an open question")]
fn all_open(w: &mut SpecWorld) {
    let session = latest_posted(w);
    assert!(!session.questions().is_empty(), "no questions were posted");
    for q in session.questions() {
        assert!(
            matches!(q.body, QuestionBody::Open { .. }),
            "not an open question: {q:?}"
        );
    }
}

#[then(regex = r"^the posted result for question (\d+) is (correct|wrong)$")]
fn result_n(w: &mut SpecWorld, n: usize, how: String) {
    let state = w.scm.state.lock().unwrap();
    let (_, _, outcome) = state.results.last().expect("no results posted");
    let r = outcome
        .results
        .get(n - 1)
        .unwrap_or_else(|| panic!("no result for question {n}"));
    assert_eq!(r.is_correct(), correctness(&how), "result: {r:?}");
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
