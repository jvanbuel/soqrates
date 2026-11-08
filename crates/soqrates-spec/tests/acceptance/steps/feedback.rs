//! Steps for features/feedback.feature (ADR-0004 D1).

use cucumber::{given, then, when};
use soqrates_core::{AnswerBody, ConceptId, QuestionResult};

use super::{author, correctness};
use crate::fakes::FakeExaminer;
use crate::world::SpecWorld;

fn result<'a>(w: &'a SpecWorld, concept: &str) -> &'a QuestionResult {
    w.outcome
        .as_ref()
        .expect("no session was graded")
        .results
        .iter()
        .find(|r| r.concept.0 == concept)
        .unwrap_or_else(|| panic!("no result for {concept}"))
}

#[given("the examiner writes multiple-choice questions without option reasons")]
fn without_reasons(w: &mut SpecWorld) {
    w.examiner.write_without_reasons();
}

#[when(regex = r#"^"([^"]+)" answers only the question about "([^"]+)" (correctly|wrongly)$"#)]
async fn answers_only(w: &mut SpecWorld, id: String, concept: String, how: String) {
    let session = w.session.clone().expect("no session started");
    assert_eq!(*session.author(), author(&id));
    let answers: Vec<_> = session
        .questions()
        .iter()
        .filter(|q| q.concept.0 == concept)
        .map(|q| FakeExaminer::answer(q, correctness(&how)))
        .collect();
    assert_eq!(answers.len(), 1, "no question about {concept}");
    let outcome = w
        .app
        .submit(session.key(), &answers, &soqrates_core::ports::NoCodebase)
        .await
        .expect("submit");
    w.outcome = Some(outcome);
}

#[then("there is one result per question, in question order")]
fn one_result_per_question(w: &mut SpecWorld) {
    let session = w.session.as_ref().expect("no session started");
    let asked: Vec<_> = session.questions().iter().map(|q| q.id.clone()).collect();
    let results: Vec<_> = w
        .outcome
        .as_ref()
        .expect("no session was graded")
        .results
        .iter()
        .map(|r| r.question.clone())
        .collect();
    assert_eq!(results, asked);
}

#[then(regex = r#"^the result for "([^"]+)" is (correct|wrong)$"#)]
fn result_verdict(w: &mut SpecWorld, concept: String, verdict: String) {
    assert_eq!(
        result(w, &concept).is_correct(),
        verdict == "correct",
        "{:?}",
        result(w, &concept)
    );
}

#[then(regex = r#"^the result for "([^"]+)" shows the author's answer$"#)]
fn shows_answer(w: &mut SpecWorld, concept: String) {
    let r = result(w, &concept);
    let session = w.session.as_ref().unwrap();
    let q = session
        .questions()
        .iter()
        .find(|q| q.concept.0 == concept)
        .unwrap();
    let correct = r.is_correct();
    assert_eq!(
        r.answer.as_ref(),
        Some(&FakeExaminer::answer(q, correct).body)
    );
}

#[then(regex = r#"^the result for "([^"]+)" names the correct option$"#)]
fn names_correct_option(w: &mut SpecWorld, concept: String) {
    let r = result(w, &concept);
    // The fake examiner always makes option 0 correct.
    assert_eq!(r.correct_option, Some(0));
    assert!(!r.options.is_empty(), "the options are listed");
}

#[then(regex = r#"^the result for "([^"]+)" names no correct option$"#)]
fn names_no_correct_option(w: &mut SpecWorld, concept: String) {
    let r = result(w, &concept);
    assert_eq!(r.correct_option, None);
    assert!(r.options.is_empty());
    assert!(matches!(r.answer, Some(AnswerBody::Text(_))));
}

#[then(regex = r#"^the result for "([^"]+)" explains the answer with the question's explanation$"#)]
fn explains_with_explanation(w: &mut SpecWorld, concept: String) {
    assert_eq!(
        result(w, &concept).feedback,
        FakeExaminer::explanation(&ConceptId(concept.clone()))
    );
}

#[then(
    regex = r#"^the result for "([^"]+)" says why the picked option is wrong, then gives the explanation$"#
)]
fn explains_the_wrong_pick(w: &mut SpecWorld, concept: String) {
    let r = result(w, &concept);
    let Some(AnswerBody::Choice(picked)) = r.answer else {
        panic!("not a choice: {:?}", r.answer);
    };
    assert_ne!(Some(picked), r.correct_option);
    let concept = ConceptId(concept);
    assert_eq!(
        r.feedback,
        format!(
            "{} {}",
            FakeExaminer::reason(&concept, &r.options[picked]),
            FakeExaminer::explanation(&concept)
        )
    );
}

#[then(
    regex = r#"^the result for "([^"]+)" carries the examiner's feedback for a (correct|wrong) answer$"#
)]
fn grader_feedback(w: &mut SpecWorld, concept: String, verdict: String) {
    assert_eq!(
        result(w, &concept).feedback,
        FakeExaminer::open_feedback(&ConceptId(concept.clone()), verdict == "correct")
    );
}

#[then(regex = r#"^the result for "([^"]+)" says it was not answered and explains the answer$"#)]
fn not_answered(w: &mut SpecWorld, concept: String) {
    let r = result(w, &concept);
    assert_eq!(r.answer, None);
    assert!(r.feedback.starts_with("Not answered."), "{}", r.feedback);
    assert!(
        r.feedback
            .contains(&FakeExaminer::explanation(&ConceptId(concept.clone()))),
        "{}",
        r.feedback
    );
}

#[then(regex = r#"^the result for "([^"]+)" is due again when its review card is due$"#)]
async fn next_review(w: &mut SpecWorld, concept: String) {
    let session_author = w.session.as_ref().unwrap().author().clone();
    let profile = w.profile(&session_author).await;
    let due = profile
        .card(&ConceptId(concept.clone()))
        .expect("a review card")
        .due();
    assert_eq!(result(w, &concept).next_review, Some(due));
}

#[then(regex = r"^the results were posted on the pull request (\d+) times?$")]
fn results_posted(w: &mut SpecWorld, n: usize) {
    let pr = w.current_pr().clone();
    let count = w
        .scm
        .state
        .lock()
        .unwrap()
        .results
        .iter()
        .filter(|(p, _, _)| *p == pr)
        .count();
    assert_eq!(count, n);
}

#[then("the posted results are for the current head")]
fn results_for_head(w: &mut SpecWorld) {
    let pr = w.current_pr().clone();
    let pull = w.scm.pull_request_now(&pr);
    let key = soqrates_core::ci::session_key(&pull);
    let state = w.scm.state.lock().unwrap();
    let (_, posted_key, outcome) = state.results.last().expect("no results posted");
    assert_eq!(*posted_key, key);
    assert_eq!(outcome.results.len(), 2);
}
