//! Steps for `follow_ups.feature` (ADR-0010), and the follow-up steps of
//! `local_hooks.feature`.

use cucumber::{given, then};
use soqrates_core::ci::session_key;
use soqrates_core::ports::StateStore;
use soqrates_core::{QuestionFormat, Session, SessionKey};

use super::{author, grade};
use crate::world::SpecWorld;

/// The key of the first round for `head` of the current pull request.
fn key_at(w: &SpecWorld, head: &str) -> SessionKey {
    let mut pr = w.scm.pull_request_now(w.current_pr());
    pr.head_sha = head.to_string();
    session_key(&pr)
}

fn posted(w: &SpecWorld, key: &SessionKey) -> Session {
    w.scm
        .posted_on(w.current_pr())
        .into_iter()
        .find(|s| s.key() == key)
        .unwrap_or_else(|| panic!("no questions were posted for {key:?}"))
}

#[given(regex = r"^Soqrates asks (?:at most (\d+)|no) follow-up rounds?$")]
fn max_follow_ups(w: &mut SpecWorld, n: String) {
    w.app.config.max_follow_ups = if n.is_empty() { 0 } else { n.parse().unwrap() };
}

#[then(regex = r#"^the latest posted session is follow-up round (\d+) of head "([^"]+)"$"#)]
fn latest_is_follow_up(w: &mut SpecWorld, round: u32, head: String) {
    let latest = w
        .scm
        .posted_on(w.current_pr())
        .pop()
        .expect("no questions were posted");
    assert_eq!(*latest.key(), key_at(w, &head).follow_up(round));
    let follow_up = latest.follow_up().expect("the session is a follow-up");
    assert_eq!(follow_up.round, round);
}

#[then(
    "the posted questions are new multiple-choice questions, written knowing the wrong answers they follow"
)]
fn new_questions_after_wrong_answers(w: &mut SpecWorld) {
    let latest = w
        .scm
        .posted_on(w.current_pr())
        .pop()
        .expect("no questions were posted");
    let (first, round) = latest.key().round();
    assert!(round >= 2, "the latest posted session is not a follow-up");
    let earlier_key = if round == 2 {
        first
    } else {
        first.follow_up(round - 1)
    };
    let earlier = posted(w, &earlier_key);
    let replies = w.scm.state.lock().unwrap().replies.clone();
    let answers = replies.get(&earlier_key).cloned().unwrap_or_default();
    let written = w.examiner.follow_ups();
    for q in latest.questions() {
        assert_eq!(q.format(), QuestionFormat::MultipleChoice, "{q:?}");
        let before = earlier
            .questions()
            .iter()
            .find(|e| e.concept == q.concept)
            .unwrap_or_else(|| panic!("{:?} was not asked before", q.concept));
        assert_ne!(q.id, before.id, "the same question");
        assert_ne!(q.body.prompt(), before.body.prompt(), "the same prompt");
        let answer = answers.iter().find(|a| a.question == before.id).cloned();
        assert!(
            written.contains(&(before.clone(), answer)),
            "the follow-up about {:?} was written without the question and the wrong answer",
            q.concept
        );
    }
}

#[then(regex = r#"^follow-up round (\d+) of head "([^"]+)" was marked superseded (\d+) times?$"#)]
fn follow_up_superseded(w: &mut SpecWorld, round: u32, head: String, n: usize) {
    let key = key_at(w, &head).follow_up(round);
    let marked = w
        .scm
        .superseded_on(w.current_pr())
        .into_iter()
        .filter(|(k, ..)| *k == key)
        .count();
    assert_eq!(marked, n);
}

#[then(regex = r#"^no pending session is stored for follow-up round (\d+) of head "([^"]+)"$"#)]
async fn no_pending_follow_up(w: &mut SpecWorld, round: u32, head: String) {
    let key = key_at(w, &head).follow_up(round);
    let stored = w.app.store.load_session(&key).await.expect("load_session");
    assert!(stored.is_none(), "follow-up {key:?} still pending");
}

#[then(regex = r#"^the latest review of "([^"]+)" by "([^"]+)" is graded (\w+)$"#)]
async fn latest_review(w: &mut SpecWorld, concept: String, id: String, g: String) {
    let p = w.profile(&author(&id)).await;
    let review = p
        .history()
        .iter()
        .rev()
        .find(|r| r.concept.0 == concept)
        .unwrap_or_else(|| panic!("no review of {concept:?}"));
    assert_eq!(review.grade, grade(&g));
}

// --- local hooks (ADR-0010 D7) --------------------------------------------

#[given(
    regex = r"^the author will answer the first (\d+) questions wrongly, then correctly, at the terminal$"
)]
fn wrongly_then_correctly(w: &mut SpecWorld, n: usize) {
    w.prompter.answer_correctly = false;
    w.prompter.correct_from = Some(n);
}

#[then(regex = r"^(\d+) results were reported to the author, the last one (passed|failed)$")]
fn reported_rounds(w: &mut SpecWorld, n: usize, how: String) {
    let run = w.hook_runs.last().expect("no hook has run");
    assert_eq!(run.reported.len(), n, "reports: {:?}", run.reported);
    assert_eq!(run.reported[n - 1].passed, how == "passed");
}
