//! Steps for `grading_journal.feature` (ADR-0002 D6).

use std::sync::Arc;
use std::sync::atomic::Ordering;

use cucumber::{given, then, when};
use soqrates_core::ports::{Clock, NoCodebase, StateStore};
use soqrates_core::{GradedSession, GradingJournal, LearnerProfile, StoreId};

use super::author;
use crate::fakes::{AnyStore, FakeExaminer, Faults, Meeting};
use crate::world::SpecWorld;

#[given(regex = r#"^"([^"]+)" has two pending sessions, about "([^"]+)" and about "([^"]+)"$"#)]
async fn two_pending_sessions(w: &mut SpecWorld, id: String, first: String, second: String) {
    let a = w.start_touching(&author(&id), &[first]).await;
    let b = w.start_touching(&author(&id), &[second]).await;
    assert!(!a.questions().is_empty() && !b.questions().is_empty());
    w.racing = vec![a, b];
}

#[when("both sessions are graded at the same moment")]
async fn graded_together(w: &mut SpecWorld) {
    let meet = Arc::new(Meeting::default());
    let store = std::mem::replace(&mut w.app.store, AnyStore::memory());
    w.app.store = store.into_faulty(Faults {
        only: None,
        allowed: 0,
        failing: 0,
        meet: Some(meet.clone()),
    });
    let answers = |s: &soqrates_core::Session| -> Vec<_> {
        s.questions()
            .iter()
            .map(|q| FakeExaminer::answer(q, true))
            .collect()
    };
    let (a, b) = (w.racing[0].clone(), w.racing[1].clone());
    let (answers_a, answers_b) = (answers(&a), answers(&b));
    let (ra, rb) = tokio::join!(
        w.app.submit(a.key(), &answers_a, &NoCodebase),
        w.app.submit(b.key(), &answers_b, &NoCodebase)
    );
    let (ra, rb) = (ra.expect("first submit"), rb.expect("second submit"));
    let store = std::mem::replace(&mut w.app.store, AnyStore::memory());
    w.app.store = store.into_healthy();
    assert_eq!(
        meet.refused.load(Ordering::SeqCst),
        1,
        "exactly one of the two saves found the profile changed"
    );
    // Each outcome reports its own reviews, scheduled.
    for r in ra.results.iter().chain(&rb.results) {
        assert!(
            r.next_review.is_some(),
            "no next review for {:?}",
            r.concept
        );
    }
    w.outcome = Some(rb);
}

#[when(regex = r#"^"([^"]+)" submits correct answers and the process stops$"#)]
async fn submits_and_stops(w: &mut SpecWorld, id: String) {
    let session = w.session.clone().expect("no session started");
    assert_eq!(*session.author(), author(&id));
    let answers: Vec<_> = session
        .questions()
        .iter()
        .map(|q| FakeExaminer::answer(q, true))
        .collect();
    let result = w.app.submit(session.key(), &answers, &NoCodebase).await;
    assert!(result.is_err(), "the store should have stopped: {result:?}");
}

#[then("the grading journal of the session is stored")]
async fn journal_stored(w: &mut SpecWorld) {
    let key = w
        .session
        .as_ref()
        .expect("no session started")
        .key()
        .clone();
    let journal = w.app.store.inner().load_grading(&key).await.expect("load");
    assert!(journal.is_some(), "no grading journal for {key:?}");
}

#[then("no grading journal is stored")]
async fn no_journal(w: &mut SpecWorld) {
    let journals = w.app.store.inner().gradings().await.expect("gradings");
    assert!(journals.is_empty(), "{journals:?}");
}

#[when(regex = r"^Soqrates tries to finish the grading (\d+) times$")]
async fn tries_to_finish(w: &mut SpecWorld, times: usize) {
    let session = w.session.clone().expect("no session started");
    for _ in 0..times {
        // Every operation first applies the journals left behind.
        let result = w.app.learner_of(session.author()).await;
        assert!(result.is_err(), "the store should still fail: {result:?}");
    }
}

#[then(regex = r#"^the stored profile of "([^"]+)" has (\d+) graded answers$"#)]
async fn stored_history(w: &mut SpecWorld, id: String, n: usize) {
    let store = w.app.store.inner();
    let learner = store
        .learner_of(&soqrates_core::Identity::parse(&id).expect("identity"))
        .await
        .expect("learner_of")
        .expect("a linked learner");
    let profile = store
        .load_profile(&learner)
        .await
        .expect("load_profile")
        .expect("a stored profile");
    assert_eq!(profile.history().len(), n, "{:?}", profile.history());
}

#[when(regex = r#"^Soqrates finds a grading journal in the old format for "([^"]+)"$"#)]
async fn old_format_journal(w: &mut SpecWorld, id: String) {
    let session = w.session.clone().expect("no session started");
    assert_eq!(*session.author(), author(&id));
    let learner = w.app.learner_of(&author(&id)).await.expect("learner_of");
    // The profile as the old version saved it: the whole thing, reviews on it.
    let mut profile = LearnerProfile::new(learner);
    let now = w.clock.now();
    let via = soqrates_core::Identity::parse(&id).expect("identity");
    for q in session.questions() {
        profile.record(
            &q.concept,
            q.format(),
            soqrates_core::Grade::Good,
            now,
            &via,
        );
    }
    let journal = GradingJournal {
        graded: GradedSession {
            key: session.key().clone(),
            author: session.author().clone(),
            correct: session.questions().len(),
            total: session.questions().len(),
            passed: true,
            at: now,
        },
        session,
        reviews: Vec::new(),
        profiles: vec![(StoreId::LearnerStore, profile)],
    };
    // As stored by the old version, which had no `reviews`.
    let mut doc = serde_json::to_value(&journal).expect("serialise");
    doc.as_object_mut().expect("object").remove("reviews");
    let journal: GradingJournal = serde_json::from_value(doc).expect("old journal");
    assert!(journal.reviews.is_empty() && !journal.profiles.is_empty());
    let claimed = w
        .app
        .store
        .inner()
        .begin_grading(&journal)
        .await
        .expect("begin_grading");
    assert!(claimed, "the session was pending");
}
