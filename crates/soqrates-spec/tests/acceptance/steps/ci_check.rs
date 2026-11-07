//! Steps for `ci_check.feature` (ADR-0001 D8, D4), driven through the
//! `ScmProvider` port with a fake forge (D12).

use cucumber::gherkin::Step;
use cucumber::{given, then, when};
use soqrates_core::ci::{run_check, session_key};
use soqrates_core::ports::StateStore;
use soqrates_core::{ConceptId, PrRef, PullRequest, Session};

use super::{answer_table, author, check_status, concepts, correctness, quoted};
use crate::fakes::{FakeExaminer, diff_touching};
use crate::world::SpecWorld;

fn pr_ref(repo: &str, number: u64) -> PrRef {
    PrRef {
        repo: repo.to_string(),
        number,
    }
}

fn latest_posted(w: &SpecWorld) -> Session {
    w.scm
        .posted_on(w.current_pr())
        .pop()
        .expect("no questions were posted on the pull request")
}

#[given(regex = r##"^a pull request "([^"#]+)#(\d+)" by "([^"]+)" at head "([^"]+)" touching (.+)$"##)]
fn pull_request(w: &mut SpecWorld, repo: String, number: u64, id: String, head: String, list: String) {
    let pr = pr_ref(&repo, number);
    w.scm.open(PullRequest {
        pr: pr.clone(),
        author: author(&id),
        head_sha: head,
        diff: diff_touching(&quoted(&list)),
    });
    w.pr = Some(pr);
}

#[given(regex = r"^the Soqrates check (?:has run|runs)$")]
#[when(regex = r"^the Soqrates check (?:has run|runs)$")]
async fn check_runs(w: &mut SpecWorld) {
    let pr = w.current_pr().clone();
    let status = run_check(&w.app, &w.scm, &pr).await.expect("run_check");
    w.status = Some(status);
}

#[given(regex = r"^the author replies with (correct|wrong) answers$")]
fn replies(w: &mut SpecWorld, how: String) {
    let s = latest_posted(w);
    let answers = s
        .questions
        .iter()
        .map(|q| FakeExaminer::answer(q, correctness(&how)))
        .collect();
    w.scm.reply(&s.key, answers);
}

#[given("the author replies with answers:")]
fn replies_table(w: &mut SpecWorld, step: &Step) {
    let key = answer_table(step);
    let s = latest_posted(w);
    let answers = s
        .questions
        .iter()
        .map(|q| {
            let correct = *key
                .get(&q.concept)
                .unwrap_or_else(|| panic!("no answer given for concept {:?}", q.concept));
            FakeExaminer::answer(q, correct)
        })
        .collect();
    w.scm.reply(&s.key, answers);
}

#[when(regex = r#"^the author pushes head "([^"]+)" touching (.+)$"#)]
fn pushes(w: &mut SpecWorld, head: String, list: String) {
    let mut pr = w.scm.pull_request_now(w.current_pr());
    pr.head_sha = head;
    pr.diff = diff_touching(&quoted(&list));
    w.scm.open(pr);
}

#[then(regex = r"^the check is (\w+)$")]
fn check_is(w: &mut SpecWorld, s: String) {
    let want = check_status(&s);
    assert_eq!(w.status, Some(want), "status returned by run_check");
    assert_eq!(
        w.scm.last_check_on(w.current_pr()),
        Some(want),
        "status set on the pull request"
    );
}

#[then(regex = r"^the questions were posted (\d+) times?$")]
fn posted_times(w: &mut SpecWorld, n: usize) {
    assert_eq!(w.scm.posted_on(w.current_pr()).len(), n);
}

#[then(regex = r"^the posted questions are about (.+)$")]
fn posted_about(w: &mut SpecWorld, list: String) {
    let asked: Vec<ConceptId> = latest_posted(w)
        .questions
        .iter()
        .map(|q| q.concept.clone())
        .collect();
    assert_eq!(asked, concepts(&list));
}

#[then("the latest posted session is for the current head")]
fn posted_for_head(w: &mut SpecWorld) {
    let pr = w.scm.pull_request_now(w.current_pr());
    let s = latest_posted(w);
    assert_eq!(s.key, session_key(&pr));
    assert_eq!(s.author, pr.author);
}

async fn pending_for_head(w: &SpecWorld) -> Option<Session> {
    let pr = w.scm.pull_request_now(w.current_pr());
    w.app
        .store
        .load_session(&session_key(&pr))
        .await
        .expect("load_session")
}

#[then("a pending session is stored for the current head")]
async fn pending_stored(w: &mut SpecWorld) {
    assert!(pending_for_head(w).await.is_some(), "no pending session");
}

#[then("no pending session is stored for the current head")]
async fn pending_gone(w: &mut SpecWorld) {
    assert!(pending_for_head(w).await.is_none(), "session still pending");
}

#[then("the session key is the same each time it is derived for the current head")]
fn key_stable(w: &mut SpecWorld) {
    let pr = w.scm.pull_request_now(w.current_pr());
    assert_eq!(session_key(&pr), session_key(&pr.clone()));
}

#[then(regex = r#"^the session key for head "([^"]+)" differs from the session key for head "([^"]+)"$"#)]
fn key_per_head(w: &mut SpecWorld, a: String, b: String) {
    let pr = w.scm.pull_request_now(w.current_pr());
    let at = |head: &str| PullRequest {
        head_sha: head.to_string(),
        ..pr.clone()
    };
    assert_ne!(session_key(&at(&a)), session_key(&at(&b)));
}

#[then(regex = r##"^the session key for pull request "([^"#]+)#(\d+)" differs from the one for "([^"#]+)#(\d+)" at the same head$"##)]
fn key_per_pr(w: &mut SpecWorld, repo_a: String, a: u64, repo_b: String, b: u64) {
    let pr = w.scm.pull_request_now(w.current_pr());
    let on = |repo: &str, n: u64| PullRequest {
        pr: pr_ref(repo, n),
        ..pr.clone()
    };
    assert_ne!(session_key(&on(&repo_a, a)), session_key(&on(&repo_b, b)));
}
