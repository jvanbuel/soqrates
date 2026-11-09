//! Steps for `question_bank.feature` (ADR-0009): reusing questions from a
//! question bank instead of writing every question from scratch.

// @adr-0009-d2 @adr-0009-d3 @adr-0009-d4 @adr-0009-d5

use chrono::Duration;
use cucumber::{given, then, when};
use soqrates_core::ports::{Clock, StateStore};
use soqrates_core::{
    BankedQuestion, ConceptId, ConceptScope, Level, Question, QuestionBody, QuestionFormat,
    QuestionKind,
};

use super::author;
use super::learner_identity::org_store;
use crate::fakes::{AnyStore, FakeExaminer};
use crate::world::SpecWorld;

/// The prompt of the `n`th question a scenario put in the bank.
fn stored_prompt(n: usize, concept: &str) -> String {
    format!("Stored question {n}: what happens to {concept} here?")
}

/// What a question asks, to tell whether two questions are the same.
fn content(q: &Question) -> (QuestionKind, QuestionBody, String) {
    (q.kind, q.body.clone(), q.explanation.clone())
}

fn session_question(w: &SpecWorld, concept: &str) -> Question {
    let session = w.session.as_ref().expect("no session");
    session
        .questions()
        .iter()
        .find(|q| q.concept.0 == concept)
        .unwrap_or_else(|| panic!("no question about {concept}: {session:?}"))
        .clone()
}

async fn bank_size(store: &AnyStore, concept: &str) -> usize {
    store
        .bank_questions(&ConceptId(concept.into()))
        .await
        .expect("bank_questions")
        .len()
}

fn shared_bank(w: &SpecWorld) -> &AnyStore {
    w.app.shared_bank.as_ref().expect("no shared question bank")
}

// ---------------------------------------------------------------------------
// Given

/// Bank `n` checked, general questions about `concept` that a novice's
/// brief accepts, in the store of a deployment without organisations.
#[given(regex = r#"^the question bank holds (\d+) questions? about "([^"]+)"$"#)]
async fn bank_holds(w: &mut SpecWorld, n: usize, concept: String) {
    let id = ConceptId(concept.clone());
    for i in 1..=n {
        let question = Question {
            id: FakeExaminer::question_id(&id, QuestionFormat::MultipleChoice),
            concept: id.clone(),
            scope: ConceptScope::General,
            kind: QuestionKind::PredictOutcome,
            body: QuestionBody::MultipleChoice {
                prompt: stored_prompt(i, &concept),
                code: Some(FakeExaminer::general_snippet(&id)),
                options: vec!["it compiles".into(), "it doesn't".into()],
                options_language: None,
                correct: 0,
                reasons: Vec::new(),
            },
            explanation: FakeExaminer::explanation(&id),
            checked: true,
            banked: None,
        };
        // Banked a second apart, in order.
        let at = w.clock.now() - Duration::seconds((n - i + 1) as i64);
        let banked = BankedQuestion::new(&question, Level::Novice, at).expect("bankable");
        w.app
            .store
            .save_bank_question(&banked)
            .await
            .expect("save_bank_question");
    }
}

#[given(regex = r"^a learner is shown a bank question at most once within (\d+) rounds$")]
fn reask_after(w: &mut SpecWorld, rounds: u64) {
    w.app.config.reask_after_rounds = rounds;
}

#[given("the deployment shares one question bank across organisations")]
fn shares_a_bank(w: &mut SpecWorld) {
    w.app.shared_bank = Some(AnyStore::memory());
}

// ---------------------------------------------------------------------------
// When

/// `n` rounds about `concept`, each answered wrongly and followed by 30
/// days, so that the concept is due again in the next.
#[when(regex = r#"^"([^"]+)" has (\d+) rounds about "([^"]+)"$"#)]
async fn has_rounds(w: &mut SpecWorld, id: String, n: usize, concept: String) {
    for _ in 0..n {
        let session = w
            .start_touching(&author(&id), std::slice::from_ref(&concept))
            .await;
        w.bank_rounds.push(session_question(w, &concept));
        w.submit_all(&session, false).await;
        w.clock.advance(Duration::days(30));
    }
}

// ---------------------------------------------------------------------------
// Then

#[then(regex = r#"^the question about "([^"]+)" is stored question (\d+) from the question bank$"#)]
fn is_stored_question(w: &mut SpecWorld, concept: String, n: usize) {
    let q = session_question(w, &concept);
    assert_eq!(q.body.prompt(), stored_prompt(n, &concept), "{q:?}");
    assert!(q.banked.is_some(), "{q:?}");
}

#[then(regex = r#"^the question about "([^"]+)" comes from the question bank$"#)]
fn comes_from_the_bank(w: &mut SpecWorld, concept: String) {
    let q = session_question(w, &concept);
    assert!(q.banked.is_some(), "{q:?}");
}

#[then(regex = r#"^no question was written about "([^"]+)"$"#)]
fn none_written(w: &mut SpecWorld, concept: String) {
    written(w, 0, concept);
}

#[then(regex = r#"^(\d+) questions? (?:was|were) written about "([^"]+)"$"#)]
fn written(w: &mut SpecWorld, n: usize, concept: String) {
    let writes = w.examiner.writes();
    let about = writes.iter().filter(|c| c.0 == concept).count();
    assert_eq!(about, n, "questions written: {writes:?}");
}

#[then(regex = r#"^the question bank now holds (\d+) questions? about "([^"]+)"$"#)]
async fn bank_now_holds(w: &mut SpecWorld, n: usize, concept: String) {
    assert_eq!(bank_size(&w.app.store, &concept).await, n);
}

#[then(regex = r#"^"([^"]+)" was never shown the same question twice within (\d+) rounds$"#)]
fn never_twice(w: &mut SpecWorld, _id: String, gap: usize) {
    let rounds = &w.bank_rounds;
    assert!(
        rounds.len() > gap,
        "too few rounds to tell: {}",
        rounds.len()
    );
    for (i, a) in rounds.iter().enumerate() {
        for (j, b) in rounds.iter().enumerate().skip(i + 1).take(gap - 1) {
            assert_ne!(
                content(a),
                content(b),
                "rounds {} and {} asked the same question",
                i + 1,
                j + 1
            );
        }
    }
}

#[then(regex = r"^(\d+) of those rounds asked a question from the question bank$")]
fn rounds_from_bank(w: &mut SpecWorld, n: usize) {
    let from_bank = w
        .bank_rounds
        .iter()
        .filter(|q| q.body.prompt().starts_with("Stored question"))
        .count();
    assert_eq!(from_bank, n, "{:?}", w.bank_rounds);
}

#[then(regex = r#"^the shared question bank holds no question about "([^"]+)"$"#)]
async fn shared_bank_holds_none(w: &mut SpecWorld, concept: String) {
    assert_eq!(bank_size(shared_bank(w), &concept).await, 0);
}

#[then(regex = r#"^the shared question bank holds (\d+) questions? about "([^"]+)"$"#)]
async fn shared_bank_holds(w: &mut SpecWorld, n: usize, concept: String) {
    assert_eq!(bank_size(shared_bank(w), &concept).await, n);
}

#[then(
    regex = r#"^the question bank of organisation "([^"]+)" holds (\d+) questions? about "([^"]+)"$"#
)]
async fn org_bank_holds(w: &mut SpecWorld, org: String, n: usize, concept: String) {
    assert_eq!(bank_size(org_store(w, &org), &concept).await, n);
}

#[then(regex = r#"^the learner store holds no question about "([^"]+)"$"#)]
async fn learner_store_holds_none(w: &mut SpecWorld, concept: String) {
    assert_eq!(bank_size(&w.app.store, &concept).await, 0);
}
