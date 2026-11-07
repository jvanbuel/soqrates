//! Configuration, time and engine-level session steps shared by all features.

use chrono::Duration;
use cucumber::gherkin::Step;
use cucumber::{given, when};
use soqrates_core::{Config, Diff};

use super::{answer_table, author, correctness, grade, level, mode, quoted};
use crate::fakes::FakeExaminer;
use crate::world::SpecWorld;

#[given(regex = r"^the question budget is (\d+)$")]
fn budget(w: &mut SpecWorld, n: usize) {
    w.app.config.max_questions = n;
}

#[given(regex = r"^the pass threshold is ([0-9.]+)$")]
fn threshold(w: &mut SpecWorld, t: f64) {
    w.app.config.pass_threshold = t;
}

#[given(regex = r"^Soqrates runs in (Advisory|Blocking) mode$")]
fn run_mode(w: &mut SpecWorld, m: String) {
    w.app.config.mode = mode(&m);
}

#[given("the default configuration")]
fn default_config(w: &mut SpecWorld) {
    w.app.config = Config::default();
}

#[given(regex = r"^correct answers are graded (\w+)$")]
fn correct_grade(w: &mut SpecWorld, g: String) {
    let g = grade(&g);
    assert!(g.is_correct(), "a correct answer cannot be graded {g:?}");
    w.examiner.set_correct_grade(g);
}

#[given(regex = r#"^"([^"]+)" is a known (\w+) author$"#)]
async fn known_author(w: &mut SpecWorld, id: String, lvl: String) {
    w.seed_level(&author(&id), level(&lvl)).await;
}

#[given(regex = r"^(\d+) days? pass(?:es)?$")]
#[when(regex = r"^(\d+) days? pass(?:es)?$")]
fn days_pass(w: &mut SpecWorld, days: i64) {
    w.clock.advance(Duration::days(days));
}

#[given(regex = r#"^"([^"]+)" (?:has answered|answers) questions about (.+) (correctly|wrongly)$"#)]
#[when(regex = r#"^"([^"]+)" (?:has answered|answers) questions about (.+) (correctly|wrongly)$"#)]
async fn answered_about(w: &mut SpecWorld, id: String, list: String, how: String) {
    let session = w.start_touching(&author(&id), &quoted(&list)).await;
    w.submit_all(&session, correctness(&how)).await;
}

#[given(regex = r#"^"([^"]+)" starts a session for a change touching (.+)$"#)]
#[when(regex = r#"^"([^"]+)" starts a session for a change touching (.+)$"#)]
async fn starts_session(w: &mut SpecWorld, id: String, list: String) {
    w.start_touching(&author(&id), &quoted(&list)).await;
}

#[when(regex = r#"^"([^"]+)" starts a session for an empty change$"#)]
async fn starts_empty_session(w: &mut SpecWorld, id: String) {
    w.start(&author(&id), &Diff(String::new())).await;
}

#[given(regex = r#"^"([^"]+)" answers the session as follows:$"#)]
#[when(regex = r#"^"([^"]+)" answers the session as follows:$"#)]
async fn answers_as_follows(w: &mut SpecWorld, id: String, step: &Step) {
    let key = answer_table(step);
    let session = w.session.clone().expect("no session started");
    assert_eq!(
        *session.author(),
        author(&id),
        "the session belongs to someone else"
    );
    let answers: Vec<_> = session
        .questions()
        .iter()
        .map(|q| {
            let correct = *key
                .get(&q.concept)
                .unwrap_or_else(|| panic!("no answer given for concept {:?}", q.concept));
            FakeExaminer::answer(q, correct)
        })
        .collect();
    let outcome = w.app.submit(session.key(), &answers).await.expect("submit");
    w.outcome = Some(outcome);
}

#[when(regex = r#"^"([^"]+)" answers every question (correctly|wrongly)$"#)]
async fn answers_every_question(w: &mut SpecWorld, id: String, how: String) {
    let session = w.session.clone().expect("no session started");
    assert_eq!(
        *session.author(),
        author(&id),
        "the session belongs to someone else"
    );
    w.submit_all(&session, correctness(&how)).await;
}

#[when(regex = r#"^"([^"]+)" answers (\d+) sessions of (\d+) new concepts (correctly|wrongly)$"#)]
async fn answers_many_sessions(w: &mut SpecWorld, id: String, n: usize, per: usize, how: String) {
    let who = author(&id);
    w.app.config.max_questions = w.app.config.max_questions.max(per);
    for s in 0..n {
        let list: Vec<String> = (0..per).map(|c| format!("drill.s{s}.c{c}")).collect();
        let session = w.start_touching(&who, &list).await;
        assert_eq!(
            session.questions().len(),
            per,
            "session {s} should ask about its {per} new concepts"
        );
        w.submit_all(&session, correctness(&how)).await;
    }
}
