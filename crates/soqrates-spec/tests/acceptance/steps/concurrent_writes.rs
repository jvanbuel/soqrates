//! Steps for `concurrent_writes.feature` (ADR-0002 D6): two runs that write
//! one record at the same moment.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use cucumber::{then, when};
use soqrates_core::ci::{run_check, session_key};
use soqrates_core::engine::SessionState;
use soqrates_core::ports::{Kind, NoCodebase, StateStore};
use soqrates_core::{Round, SessionKey};

use super::{author, quoted};
use crate::fakes::{AnyStore, FakeCheckout, Faults, Meeting, diff_touching};
use crate::world::SpecWorld;

/// Make the first two swaps of `kind` collide, until [`heal`].
fn meet_on(w: &mut SpecWorld, kind: Kind) -> Arc<Meeting> {
    let meet = Arc::new(Meeting::default());
    let store = std::mem::replace(&mut w.app.store, AnyStore::memory());
    w.app.store = store.into_faulty(Faults {
        only: None,
        allowed: 0,
        failing: 0,
        meet: Some(meet.clone()),
        meet_on: kind,
    });
    meet
}

/// Take the collisions out again; check that exactly one write lost.
fn heal(w: &mut SpecWorld, meet: &Meeting) {
    let store = std::mem::replace(&mut w.app.store, AnyStore::memory());
    w.app.store = store.into_healthy();
    assert_eq!(
        meet.refused.load(Ordering::SeqCst),
        1,
        "exactly one of the two writes found the record changed"
    );
}

#[when(
    regex = r#"^"([^"]+)" starts sessions for changes touching "([^"]+)" and "([^"]+)" at the same moment$"#
)]
async fn sessions_at_once(w: &mut SpecWorld, id: String, a: String, b: String) {
    let who = author(&id);
    // The learner exists already: the race is about the rounds.
    w.app.learner_of(&who).await.expect("learner_of");
    let (ka, kb) = (w.next_local_key(&who), w.next_local_key(&who));
    let (da, db) = (diff_touching(&[a]), diff_touching(&[b]));
    let meet = meet_on(w, Kind::Seen);
    let (ra, rb) = tokio::join!(
        w.app.start_session(ka, &who, &da, &NoCodebase),
        w.app.start_session(kb, &who, &db, &NoCodebase),
    );
    heal(w, &meet);
    w.racing = vec![ra.expect("first session"), rb.expect("second session")];
}

#[then(
    regex = r#"^what "([^"]+)" was shown holds the bank questions of both sessions, from different rounds$"#
)]
async fn shown_both(w: &mut SpecWorld, id: String) {
    let learner = w.app.learner_of(&author(&id)).await.expect("learner_of");
    let seen = w
        .app
        .store
        .load_seen(&learner)
        .await
        .expect("load_seen")
        .expect("nothing recorded as shown");
    let rounds: Vec<u64> = w
        .racing
        .iter()
        .flat_map(|s| s.questions())
        .map(|q| {
            let banked = q.banked.as_ref().expect("the question was banked");
            seen.last_seen(&banked.id)
                .unwrap_or_else(|| panic!("{:?} is not recorded as shown", q.concept))
        })
        .collect();
    assert_eq!(rounds.len(), 2, "one question per session");
    assert_ne!(rounds[0], rounds[1], "both rounds got the same number");
}

#[when(
    regex = r#"^rounds for heads "([^"]+)" and "([^"]+)" of the pull request are recorded at the same moment$"#
)]
async fn rounds_at_once(w: &mut SpecWorld, h1: String, h2: String) {
    let pr = w.current_pr().clone();
    let by = w.scm.pull_request_now(&pr).author;
    let now = chrono::Utc::now();
    let round = |head: &str| {
        Round::own(
            head.to_string(),
            SessionKey::pr(&pr.repo, pr.number, head),
            Vec::new(),
            now,
        )
    };
    let (r1, r2) = (round(&h1), round(&h2));
    let meet = meet_on(w, Kind::Rounds);
    let (a, b) = tokio::join!(
        w.app.record_round(&pr, &by, r1),
        w.app.record_round(&pr, &by, r2),
    );
    heal(w, &meet);
    a.expect("first round");
    b.expect("second round");
}

#[then(regex = r#"^the pull request has rounds for heads (.+)$"#)]
async fn has_rounds(w: &mut SpecWorld, list: String) {
    let pr = w.current_pr().clone();
    let by = w.scm.pull_request_now(&pr).author;
    let rounds = w.app.rounds(&pr, &by).await.expect("rounds");
    let mut heads: Vec<String> = rounds.rounds().iter().map(|r| r.head.clone()).collect();
    heads.sort();
    let mut want = quoted(&list);
    want.sort();
    assert_eq!(heads, want);
}

#[when("the Soqrates check runs twice at the same moment")]
async fn check_runs_twice(w: &mut SpecWorld) {
    let pr = w.current_pr().clone();
    let meet = meet_on(w, Kind::Session);
    let (a, b) = tokio::join!(
        run_check(&w.app, &w.scm, &FakeCheckout, &pr),
        run_check(&w.app, &w.scm, &FakeCheckout, &pr),
    );
    heal(w, &meet);
    let (a, b) = (a.expect("first run"), b.expect("second run"));
    assert_eq!(a, b, "both runs report the same status");
    w.status = Some(a);
}

#[then("the pending session stored for the current head is the one posted")]
async fn stored_is_posted(w: &mut SpecWorld) {
    let pr = w.current_pr().clone();
    let posted = w.scm.posted_on(&pr);
    let key = session_key(&w.scm.pull_request_now(&pr));
    let stored = w
        .app
        .pending_session(&key)
        .await
        .expect("pending_session")
        .expect("no pending session");
    assert_eq!(posted, vec![stored]);
}

#[then(regex = r#"^opening session (\d+) of "([^"]+)" again opens nothing, and it stays graded$"#)]
async fn reopen_graded(w: &mut SpecWorld, n: usize, id: String) {
    let who = author(&id);
    let key = SessionKey(format!("local:{}:{n}", who.0));
    let opened = w
        .app
        .open_session(key.clone(), &who, Vec::new(), 0)
        .await
        .expect("open_session");
    assert!(opened.is_none(), "a graded session was opened again");
    assert!(matches!(
        w.app.session_state(&key).await.expect("session_state"),
        SessionState::Graded(_)
    ));
}
