//! Steps for `ci_check.feature` (ADR-0001 D8, D4), driven through the
//! `ScmProvider` port with a fake forge (D12).

use cucumber::gherkin::Step;
use cucumber::{given, then, when};
use soqrates_core::ci::{run_check, session_key};
use soqrates_core::ports::StateStore;
use soqrates_core::{ConceptId, PrRef, PullRequest, Session};

use super::{answer_table, author, check_status, concepts, correctness, quoted};
use crate::fakes::{AnyStore, FakeExaminer, Faults, diff_touching};
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

#[given(
    regex = r##"^a pull request "([^"#]+)#(\d+)" by "([^"]+)" at head "([^"]+)" touching (.+)$"##
)]
fn pull_request(
    w: &mut SpecWorld,
    repo: String,
    number: u64,
    id: String,
    head: String,
    list: String,
) {
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

#[when("the author's correct answers are submitted twice at the same time")]
async fn submitted_twice_concurrently(w: &mut SpecWorld) {
    let s = latest_posted(w);
    let answers: Vec<_> = s
        .questions()
        .iter()
        .map(|q| FakeExaminer::answer(q, true))
        .collect();
    let (a, b) = tokio::join!(
        w.app.submit(s.key(), &answers),
        w.app.submit(s.key(), &answers)
    );
    let ok = [a.is_ok(), b.is_ok()].iter().filter(|x| **x).count();
    assert_eq!(ok, 1, "exactly one submission is graded: {a:?} / {b:?}");
    let lost = if a.is_err() { a } else { b };
    assert!(
        matches!(lost, Err(soqrates_core::Error::NotFound(_))),
        "the other submission finds no session: {lost:?}"
    );
}

#[given("posting the questions fails once")]
fn posting_fails_once(w: &mut SpecWorld) {
    w.scm.state.lock().unwrap().fail_next_post = true;
}

/// Inject `faults` into the learner store.
fn inject(w: &mut SpecWorld, faults: Faults) {
    let store = std::mem::replace(&mut w.app.store, AnyStore::memory());
    w.app.store = store.into_faulty(faults);
}

#[given(
    regex = r"^(writing the grading journal|saving the learner profile|removing the pending session|recording the graded session|removing the grading journal) (fails once|keeps failing)$"
)]
fn write_fails(w: &mut SpecWorld, what: String, how: String) {
    let method = match what.as_str() {
        "writing the grading journal" => "begin_grading",
        "saving the learner profile" => "save_profile",
        "removing the pending session" => "delete_session",
        "recording the graded session" => "save_graded",
        _ => "end_grading",
    };
    let failing = if how == "fails once" { 1 } else { usize::MAX };
    inject(
        w,
        Faults {
            only: Some(method.to_string()),
            allowed: 0,
            failing,
        },
    );
}

#[given(regex = r"^the process stops after (\d+) writes? to the store$")]
fn stops_after(w: &mut SpecWorld, n: usize) {
    inject(
        w,
        Faults {
            only: None,
            allowed: n,
            failing: usize::MAX,
        },
    );
}

#[when("the store works again")]
fn store_works_again(w: &mut SpecWorld) {
    let store = std::mem::replace(&mut w.app.store, AnyStore::memory());
    w.app.store = store.into_healthy();
}

#[then(regex = r"^the session for the current head is (pending|being graded|graded)$")]
async fn session_state(w: &mut SpecWorld, want: String) {
    let pr = w.scm.pull_request_now(w.current_pr());
    let key = session_key(&pr);
    let store = w.app.store.inner();
    let pending = store.load_session(&key).await.expect("load_session");
    let journal = store.load_grading(&key).await.expect("load_grading");
    let graded = store.load_graded(&key).await.expect("load_graded");
    let state = match (&pending, &journal, &graded) {
        (Some(_), None, None) => "pending",
        (_, Some(_), None) => "being graded",
        (None, _, Some(_)) => "graded",
        _ => panic!("inconsistent state: {pending:?} / {journal:?} / {graded:?}"),
    };
    assert_eq!(state, want);
}

#[then(regex = r#"^the check says "([^"]+)"$"#)]
fn check_says(w: &mut SpecWorld, text: String) {
    let summary = w
        .scm
        .last_summary_on(w.current_pr())
        .expect("no check output");
    assert!(summary.contains(&text), "check output: {summary:?}");
}

#[when("the Soqrates check runs and fails")]
async fn check_runs_and_fails(w: &mut SpecWorld) {
    let pr = w.current_pr().clone();
    assert!(
        run_check(&w.app, &w.scm, &pr).await.is_err(),
        "expected the check run to fail"
    );
}

#[given(regex = r"^the author replies with (correct|wrong) answers$")]
fn replies(w: &mut SpecWorld, how: String) {
    let s = latest_posted(w);
    let answers = s
        .questions()
        .iter()
        .map(|q| FakeExaminer::answer(q, correctness(&how)))
        .collect();
    w.scm.reply(s.key(), answers);
}

#[given("the author replies with answers:")]
fn replies_table(w: &mut SpecWorld, step: &Step) {
    let key = answer_table(step);
    let s = latest_posted(w);
    let answers = s
        .questions()
        .iter()
        .map(|q| {
            let correct = *key
                .get(&q.concept)
                .unwrap_or_else(|| panic!("no answer given for concept {:?}", q.concept));
            FakeExaminer::answer(q, correct)
        })
        .collect();
    w.scm.reply(s.key(), answers);
}

#[given(regex = r#"^the author pushes head "([^"]+)" touching (.+)$"#)]
#[when(regex = r#"^the author pushes head "([^"]+)" touching (.+)$"#)]
fn pushes(w: &mut SpecWorld, head: String, list: String) {
    let pr = w.current_pr().clone();
    w.scm.push(&pr, &head, touching(&list));
}

#[given(regex = r#"^the author force-pushes head "([^"]+)" touching (.+)$"#)]
#[when(regex = r#"^the author force-pushes head "([^"]+)" touching (.+)$"#)]
fn force_pushes(w: &mut SpecWorld, head: String, list: String) {
    let mut pr = w.scm.pull_request_now(w.current_pr());
    pr.head_sha = head;
    pr.diff = touching(&list);
    w.scm.open(pr);
}

/// A diff touching the quoted concepts of `list`, or `nothing`.
fn touching(list: &str) -> soqrates_core::Diff {
    if list == "nothing" {
        diff_touching(&[])
    } else {
        diff_touching(&quoted(list))
    }
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
        .questions()
        .iter()
        .map(|q| q.concept.clone())
        .collect();
    assert_eq!(asked, concepts(&list));
}

#[then(
    regex = r#"^the latest posted session starts with the questions posted for head "([^"]+)"$"#
)]
fn starts_with_carried(w: &mut SpecWorld, head: String) {
    let posted = w.scm.posted_on(w.current_pr());
    let earlier = posted
        .iter()
        .find(|s| s.key().0.ends_with(&format!("@{head}")))
        .unwrap_or_else(|| panic!("no questions were posted for head {head}"));
    let latest = posted.last().unwrap();
    let carried = earlier.questions();
    assert!(
        latest.questions().starts_with(carried),
        "the questions for {head} are not asked again unchanged, first"
    );
}

#[then("the latest posted session is for the current head")]
fn posted_for_head(w: &mut SpecWorld) {
    let pr = w.scm.pull_request_now(w.current_pr());
    let s = latest_posted(w);
    assert_eq!(*s.key(), session_key(&pr));
    assert_eq!(*s.author(), pr.author);
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

#[then(
    regex = r#"^the session key for head "([^"]+)" differs from the session key for head "([^"]+)"$"#
)]
fn key_per_head(w: &mut SpecWorld, a: String, b: String) {
    let pr = w.scm.pull_request_now(w.current_pr());
    let at = |head: &str| PullRequest {
        head_sha: head.to_string(),
        ..pr.clone()
    };
    assert_ne!(session_key(&at(&a)), session_key(&at(&b)));
}

#[then(
    regex = r##"^the session key for pull request "([^"#]+)#(\d+)" differs from the one for "([^"#]+)#(\d+)" at the same head$"##
)]
fn key_per_pr(w: &mut SpecWorld, repo_a: String, a: u64, repo_b: String, b: u64) {
    let pr = w.scm.pull_request_now(w.current_pr());
    let on = |repo: &str, n: u64| PullRequest {
        pr: pr_ref(repo, n),
        ..pr.clone()
    };
    assert_ne!(session_key(&on(&repo_a, a)), session_key(&on(&repo_b, b)));
}

// --- superseded question comments (ADR-0004 D7) ---------------------------

fn key_at(w: &SpecWorld, head: &str) -> soqrates_core::SessionKey {
    let mut pr = w.scm.pull_request_now(w.current_pr());
    pr.head_sha = head.to_string();
    session_key(&pr)
}

#[given("freezing superseded question comments fails once")]
fn supersede_fails_once(w: &mut SpecWorld) {
    w.scm.state.lock().unwrap().fail_next_supersede = true;
}

#[when(regex = r#"^the author answers the questions for head "([^"]+)" correctly$"#)]
fn answers_old_head(w: &mut SpecWorld, head: String) {
    let key = key_at(w, &head);
    let s = w
        .scm
        .posted_on(w.current_pr())
        .into_iter()
        .find(|s| *s.key() == key)
        .unwrap_or_else(|| panic!("no questions were posted for head {head:?}"));
    let answers = s
        .questions()
        .iter()
        .map(|q| FakeExaminer::answer(q, true))
        .collect();
    w.scm.reply(&key, answers);
}

#[then(regex = r#"^the question comment for head "([^"]+)" was marked superseded (\d+) times?$"#)]
fn superseded_times(w: &mut SpecWorld, head: String, n: usize) {
    let key = key_at(w, &head);
    let marked = w
        .scm
        .superseded_on(w.current_pr())
        .into_iter()
        .filter(|(k, ..)| *k == key)
        .count();
    assert_eq!(marked, n);
}

#[then("it points to the question comment for the current head and shows its own questions")]
fn superseded_points_to_current(w: &mut SpecWorld) {
    let pr = w.scm.pull_request_now(w.current_pr());
    let (old, shown, newer) = w
        .scm
        .superseded_on(w.current_pr())
        .pop()
        .expect("no question comment was marked superseded");
    assert_eq!(newer, session_key(&pr));
    let posted = w
        .scm
        .posted_on(w.current_pr())
        .into_iter()
        .find(|s| *s.key() == old)
        .expect("the superseded session was posted");
    let shown = shown.expect("the superseded session's questions are shown");
    assert_eq!(shown.questions(), posted.questions());
}

#[then("no question comment was marked superseded")]
fn none_superseded(w: &mut SpecWorld) {
    assert_eq!(w.scm.superseded_on(w.current_pr()), vec![]);
}

#[then(regex = r#"^(a|no) pending session is stored for head "([^"]+)"$"#)]
async fn pending_at_head(w: &mut SpecWorld, which: String, head: String) {
    let key = key_at(w, &head);
    let stored = w
        .app
        .store
        .load_session(&key)
        .await
        .expect("load_session")
        .is_some();
    assert_eq!(stored, which == "a", "pending session for head {head:?}");
}

#[then("the latest posted session is for the current head and asks nothing")]
fn posted_empty_for_head(w: &mut SpecWorld) {
    posted_for_head(w);
    assert_eq!(latest_posted(w).questions(), &[]);
}

// --- rounds (ADR-0002 D9) -------------------------------------------------

#[then(regex = r#"^the change was compared from head "([^"]+)" to head "([^"]+)"$"#)]
fn compared(w: &mut SpecWorld, from: String, to: String) {
    let state = w.scm.state.lock().unwrap();
    let (_, f, t) = state
        .compared
        .iter()
        .rev()
        .find(|(p, ..)| p == w.current_pr())
        .expect("the change was never compared");
    assert_eq!((f, t), (&from, &to));
}

#[then(regex = r"^the change was compared (\d+) times?$")]
fn compared_times(w: &mut SpecWorld, n: usize) {
    let state = w.scm.state.lock().unwrap();
    let times = state
        .compared
        .iter()
        .filter(|(p, ..)| p == w.current_pr())
        .count();
    assert_eq!(times, n);
}
