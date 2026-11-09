//! Steps for `domain_learner_profile.feature` and `domain_session.feature`
//! (ADR-0002 D3-D8).
//!
//! Most steps exercise the aggregates and the policy directly: synchronous,
//! no engine. Only the steps about the engine boundary (D6 `NotFound`, D7
//! `SessionOutcome::events`) go through `Soqrates::submit`.

// @adr-0002-d3 @adr-0002-d4 @adr-0002-d5 @adr-0002-d6 @adr-0002-d7 @adr-0002-d8

use std::collections::HashMap;

use cucumber::gherkin::Step;
use cucumber::{given, then, when};
use soqrates_core::policy::{DEMOTE_AFTER, PROMOTE_AFTER, format_for, next_level};
use soqrates_core::ports::Clock;
use soqrates_core::{
    Answer, AnswerBody, AuthorId, ConceptId, ConceptScope, DomainEvent, Error, Grade, GradedAnswer,
    Identity, LearnerEvent, LearnerId, LearnerProfile, Question, QuestionBody, QuestionFormat,
    QuestionId, QuestionKind, Session, SessionEvent, SessionKey, SessionOutcome, SessionResult,
};

use super::{author, concepts, format, grade, level};
use crate::fakes::FakeExaminer;
use crate::world::SpecWorld;

// ---------------------------------------------------------------------------
// helpers

fn profile(w: &SpecWorld) -> &LearnerProfile {
    w.domain
        .profile
        .as_ref()
        .expect("no learner profile in this scenario")
}

fn record(w: &mut SpecWorld, concept: ConceptId, fmt: QuestionFormat, g: Grade) {
    let now = w.clock.now();
    let p = w
        .domain
        .profile
        .as_mut()
        .expect("no learner profile in this scenario");
    let via = via_of(p);
    let events = p.record(&concept, fmt, g, now, &via);
    w.domain.all_events.extend(events.iter().cloned());
    w.domain.last_events = events;
}

/// In these scenarios the learner ID has the same text as the one identity
/// every review comes through, e.g. `github:1001` (ADR-0003 D3).
fn learner(id: &str) -> LearnerId {
    LearnerId(id.to_string())
}

fn via_of(p: &LearnerProfile) -> Identity {
    Identity::parse(&p.learner().0).expect("the learner ID of these scenarios is an identity")
}

/// The learner behind `who`, as the engine resolves it (ADR-0003 D1).
async fn learner_of(w: &SpecWorld, who: &AuthorId) -> LearnerId {
    w.app.learner_of(who).await.expect("learner_of")
}

fn event_name(e: &LearnerEvent) -> &'static str {
    match e {
        LearnerEvent::ReviewRecorded { .. } => "ReviewRecorded",
        LearnerEvent::ReviewScheduled { .. } => "ReviewScheduled",
        LearnerEvent::LevelChanged { .. } => "LevelChanged",
    }
}

fn domain_event_name(e: &DomainEvent) -> &'static str {
    match e {
        DomainEvent::Learner(l) => event_name(l),
        DomainEvent::Session(SessionEvent::SessionGraded { .. }) => "SessionGraded",
    }
}

/// `"Good, Again"` or `"multiple-choice, open"` -> trimmed items.
fn plain_list(list: &str) -> Vec<String> {
    list.split(',').map(|s| s.trim().to_string()).collect()
}

/// A multiple-choice question about `concept`, with the id the
/// [`FakeExaminer`] would give it. Option 0 is correct.
fn mc_question(concept: &ConceptId) -> Question {
    Question {
        id: FakeExaminer::question_id(concept, QuestionFormat::MultipleChoice),
        concept: concept.clone(),
        scope: ConceptScope::General,
        kind: QuestionKind::Scenario,
        body: QuestionBody::MultipleChoice {
            prompt: format!("Which statement about {} is true?", concept.0),
            code: None,
            options: vec!["right".into(), "wrong".into(), "also wrong".into()],
            options_language: None,
            correct: 0,
            reasons: Vec::new(),
        },
        explanation: FakeExaminer::explanation(concept),
        checked: false,
        banked: None,
    }
}

fn open_question(concept: &ConceptId) -> Question {
    Question {
        id: FakeExaminer::question_id(concept, QuestionFormat::Open),
        concept: concept.clone(),
        scope: ConceptScope::General,
        kind: QuestionKind::Scenario,
        body: QuestionBody::Open {
            prompt: format!("Explain {} in this change.", concept.0),
            code: None,
            rubric: format!("Covers {}", concept.0),
        },
        explanation: FakeExaminer::explanation(concept),
        checked: false,
        banked: None,
    }
}

fn session_key(who: &AuthorId) -> SessionKey {
    SessionKey(format!("domain:{}", who.0))
}

fn open_session(w: &mut SpecWorld, who: &str, max_questions: usize) {
    let who = author(who);
    let questions = w.domain.questions.clone();
    let opened = Session::open(
        session_key(&who),
        who,
        questions,
        max_questions,
        w.clock.now(),
    );
    w.domain.opened = Some(opened);
}

fn opened(w: &SpecWorld) -> &Session {
    match w.domain.opened.as_ref().expect("no session was opened") {
        Ok(s) => s,
        Err(e) => panic!("the session was not opened: {e:?}"),
    }
}

fn conclude(w: &mut SpecWorld, grades: Vec<(QuestionId, Grade)>, threshold: f64) {
    let session = match w.domain.opened.take().expect("no session was opened") {
        Ok(s) => s,
        Err(e) => panic!("the session was not opened: {e:?}"),
    };
    let result = session.conclude(grades, threshold, w.clock.now());
    w.domain.concluded = Some(result);
}

fn concluded(w: &SpecWorld) -> &SessionResult {
    w.domain
        .concluded
        .as_ref()
        .expect("the session was not concluded")
}

fn grades_by_concept(step: &Step) -> HashMap<ConceptId, Grade> {
    let table = step.table.as_ref().expect("this step needs a data table");
    let mut rows = table.rows.iter();
    assert_eq!(
        rows.next().expect("table header"),
        &["concept", "grade"],
        "table columns must be | concept | grade |"
    );
    rows.map(|r| (ConceptId(r[0].clone()), grade(&r[1])))
        .collect()
}

fn outcome(w: &SpecWorld) -> &SessionOutcome {
    w.outcome.as_ref().expect("no session outcome")
}

fn engine_session(w: &SpecWorld) -> Session {
    w.session.clone().expect("no session started")
}

async fn submit_expecting_error(w: &mut SpecWorld, key: &SessionKey, answers: &[Answer]) {
    match w
        .app
        .submit(key, answers, &soqrates_core::ports::NoCodebase)
        .await
    {
        Ok(o) => panic!("the submission was accepted: {o:?}"),
        Err(e) => w.domain.submit_error = Some(e),
    }
}

// ---------------------------------------------------------------------------
// D3, D4: the learner profile

#[given(regex = r#"^a new learner profile for "([^"]+)"$"#)]
fn new_profile(w: &mut SpecWorld, id: String) {
    w.domain.profile = Some(LearnerProfile::new(learner(&id)));
}

#[given(regex = r#"^a learner profile for "([^"]+)" starting at (\w+)$"#)]
fn profile_starting_at(w: &mut SpecWorld, id: String, lvl: String) {
    w.domain.profile = Some(LearnerProfile::starting_at(learner(&id), level(&lvl)));
}

#[given(
    regex = r#"^a review of "([^"]+)" graded (\w+) is recorded(?: for an? (multiple-choice|open) question)?$"#
)]
#[when(
    regex = r#"^a review of "([^"]+)" graded (\w+) is recorded(?: for an? (multiple-choice|open) question)?$"#
)]
fn review_recorded(w: &mut SpecWorld, concept: String, g: String, fmt: String) {
    let fmt = if fmt.is_empty() {
        QuestionFormat::MultipleChoice
    } else {
        format(&fmt)
    };
    record(w, ConceptId(concept), fmt, grade(&g));
}

#[given(regex = r"^(\d+) reviews? graded (\w+) (?:is|are) recorded$")]
#[when(regex = r"^(\d+) reviews? graded (\w+) (?:is|are) recorded$")]
fn reviews_recorded(w: &mut SpecWorld, n: usize, g: String) {
    let g = grade(&g);
    for _ in 0..n {
        w.domain.generated += 1;
        let concept = ConceptId(format!("drill.c{}", w.domain.generated));
        record(w, concept, QuestionFormat::MultipleChoice, g);
    }
}

#[then(regex = r#"^the learner profile belongs to "([^"]+)"$"#)]
fn profile_belongs_to(w: &mut SpecWorld, id: String) {
    assert_eq!(*profile(w).learner(), learner(&id));
}

#[then(regex = r"^the learner profile is at level (\w+)$")]
fn profile_level(w: &mut SpecWorld, lvl: String) {
    assert_eq!(profile(w).level(), level(&lvl), "level of the profile");
}

#[then(regex = r"^the learner profile has (\d+) review cards?$")]
fn profile_card_count(w: &mut SpecWorld, n: usize) {
    let cards = profile(w).cards();
    assert_eq!(cards.len(), n, "review cards: {:?}", cards.keys());
}

#[then(regex = r#"^the learner profile has a review card for "([^"]+)" reviewed (\d+) times?$"#)]
fn profile_card_reps(w: &mut SpecWorld, concept: String, reps: u32) {
    let card = profile(w)
        .card(&ConceptId(concept.clone()))
        .unwrap_or_else(|| panic!("no review card for {concept}"));
    assert_eq!(card.reps(), reps, "reviews of {concept}");
}

#[then(regex = r#"^the review card for "([^"]+)" is due later than now$"#)]
fn card_due_later(w: &mut SpecWorld, concept: String) {
    let now = w.clock.now();
    let card = profile(w)
        .card(&ConceptId(concept.clone()))
        .unwrap_or_else(|| panic!("no review card for {concept}"));
    assert!(card.due() > now, "{concept} is due at {}", card.due());
    assert!(!card.is_due(now), "{concept} is already due");
}

#[then(regex = r"^the learner profile has (\d+) reviews? in its history$")]
fn profile_history_len(w: &mut SpecWorld, n: usize) {
    assert_eq!(profile(w).history().len(), n, "history");
}

#[then(regex = r"^the learner profile has (\d+) reviews? since the last level change$")]
fn reviews_since_change(w: &mut SpecWorld, n: usize) {
    assert_eq!(
        profile(w).reviews_since_level_change().len(),
        n,
        "reviews since the last level change"
    );
}

#[then(
    regex = r#"^the last review in the history is of "([^"]+)", graded (\w+), for an? (multiple-choice|open) question, recorded now$"#
)]
fn last_review(w: &mut SpecWorld, concept: String, g: String, fmt: String) {
    let want = GradedAnswer {
        concept: ConceptId(concept),
        format: format(&fmt),
        grade: grade(&g),
        at: w.clock.now(),
        via: via_of(profile(w)),
    };
    assert_eq!(profile(w).history().last(), Some(&want), "last review");
}

#[then(
    regex = r"^promotion takes (\d+) consecutive correct reviews and demotion takes (\d+) consecutive incorrect reviews$"
)]
fn thresholds(_w: &mut SpecWorld, promote: usize, demote: usize) {
    assert_eq!(PROMOTE_AFTER, promote, "PROMOTE_AFTER");
    assert_eq!(DEMOTE_AFTER, demote, "DEMOTE_AFTER");
}

#[then(regex = r"^the next level of an? (\w+) author after (.+) is (\w+)$")]
fn policy_next_level(w: &mut SpecWorld, current: String, reviews: String, expected: String) {
    let start = w.clock.now();
    let mut recent = Vec::new();
    for item in plain_list(&reviews) {
        let (n, g) = item
            .split_once(' ')
            .unwrap_or_else(|| panic!("expected '<count> <grade>', got {item:?}"));
        let n: usize = n.parse().expect("count");
        let g = grade(g);
        for _ in 0..n {
            let i = recent.len();
            recent.push(GradedAnswer {
                concept: ConceptId(format!("drill.c{i}")),
                format: QuestionFormat::MultipleChoice,
                grade: g,
                at: start + chrono::Duration::minutes(i as i64),
                via: Identity::parse("github:1001").expect("identity"),
            });
        }
    }
    assert_eq!(
        next_level(level(&current), &recent),
        level(&expected),
        "next level of a {current} author after {reviews}"
    );
}

// ---------------------------------------------------------------------------
// D5: format rule

#[then(
    regex = r"^questions for an? (\w+) author with open questions (on|off) are (multiple-choice|open)$"
)]
fn format_for_level(_w: &mut SpecWorld, lvl: String, open: String, fmt: String) {
    assert_eq!(
        format_for(level(&lvl), open == "on"),
        format(&fmt),
        "format for a {lvl} author with open questions {open}"
    );
}

#[then("open questions are off in the default configuration")]
fn open_off_by_default(_w: &mut SpecWorld) {
    assert!(!soqrates_core::Config::default().open_questions);
}

// ---------------------------------------------------------------------------
// D7: learner events

#[then(regex = r"^the last recorded review emitted exactly (.+)$")]
fn last_events_exactly(w: &mut SpecWorld, list: String) {
    let mut got: Vec<&str> = w.domain.last_events.iter().map(event_name).collect();
    let mut want = plain_list(&list);
    got.sort_unstable();
    want.sort_unstable();
    assert_eq!(got, want, "events: {:?}", w.domain.last_events);
}

#[then("no recorded review emitted LevelChanged")]
fn no_level_changed(w: &mut SpecWorld) {
    assert!(
        !w.domain.all_events.is_empty(),
        "no review was recorded in this scenario"
    );
    let changes: Vec<_> = w
        .domain
        .all_events
        .iter()
        .filter(|e| matches!(e, LearnerEvent::LevelChanged { .. }))
        .collect();
    assert!(changes.is_empty(), "LevelChanged events: {changes:?}");
}

#[then(
    regex = r#"^the ReviewRecorded event names "([^"]+)" and the review of "([^"]+)" graded (\w+)$"#
)]
fn review_recorded_event(w: &mut SpecWorld, id: String, concept: String, g: String) {
    let (who, review) = w
        .domain
        .last_events
        .iter()
        .find_map(|e| match e {
            LearnerEvent::ReviewRecorded { learner, review } => Some((learner, review)),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no ReviewRecorded in {:?}", w.domain.last_events));
    assert_eq!(*who, learner(&id));
    assert_eq!(review.concept, ConceptId(concept));
    assert_eq!(review.grade, grade(&g));
    assert_eq!(review.at, w.clock.now());
    assert_eq!(
        profile(w).history().last(),
        Some(review),
        "the recorded review is the last one in the history"
    );
}

#[then(regex = r#"^the ReviewScheduled event for "([^"]+)" is due when its review card is due$"#)]
fn review_scheduled_event(w: &mut SpecWorld, concept: String) {
    let concept = ConceptId(concept);
    let p = profile(w);
    let (who, c, due) = w
        .domain
        .last_events
        .iter()
        .find_map(|e| match e {
            LearnerEvent::ReviewScheduled {
                learner,
                concept,
                due,
            } => Some((learner, concept, *due)),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no ReviewScheduled in {:?}", w.domain.last_events));
    assert_eq!(who, p.learner());
    assert_eq!(*c, concept);
    let card = p
        .card(&concept)
        .unwrap_or_else(|| panic!("no review card for {concept:?}"));
    assert_eq!(due, card.due(), "due date in ReviewScheduled");
}

#[then(regex = r#"^the LevelChanged event names "([^"]+)" and goes from (\w+) to (\w+)$"#)]
fn level_changed_event(w: &mut SpecWorld, id: String, from: String, to: String) {
    let want = LearnerEvent::LevelChanged {
        learner: learner(&id),
        from: level(&from),
        to: level(&to),
        at: w.clock.now(),
    };
    assert!(
        w.domain.last_events.contains(&want),
        "expected {want:?} in {:?}",
        w.domain.last_events
    );
    assert_eq!(profile(w).level(), level(&to), "level after the change");
}

// ---------------------------------------------------------------------------
// D8: due reviews

#[then("the learner profile has no due reviews")]
fn no_due_reviews(w: &mut SpecWorld) {
    let due = profile(w).due_reviews(w.clock.now());
    assert!(due.is_empty(), "due reviews: {due:?}");
}

#[then(regex = r"^the due reviews are (.+), in that order$")]
fn due_reviews_in_order(w: &mut SpecWorld, list: String) {
    let due = profile(w).due_reviews(w.clock.now());
    assert_eq!(due, concepts(&list), "due reviews");
}

// ---------------------------------------------------------------------------
// D3, D6: the session aggregate

#[given(regex = r"^multiple-choice questions about (.+)$")]
fn mc_questions(w: &mut SpecWorld, list: String) {
    w.domain.questions = concepts(&list).iter().map(mc_question).collect();
}

#[given(regex = r#"^a multiple-choice and an open question about "([^"]+)"$"#)]
fn two_formats_one_concept(w: &mut SpecWorld, concept: String) {
    let c = ConceptId(concept);
    w.domain.questions = vec![mc_question(&c), open_question(&c)];
}

#[given("no questions")]
fn no_questions(w: &mut SpecWorld) {
    w.domain.questions.clear();
}

#[when(regex = r#"^a session for "([^"]+)" is opened with at most (\d+) questions$"#)]
fn session_opened_with_budget(w: &mut SpecWorld, id: String, max: usize) {
    open_session(w, &id, max);
}

#[when(
    regex = r#"^the engine opens a session for "([^"]+)" whose first (\d+) questions are carried$"#
)]
async fn engine_opens_session(w: &mut SpecWorld, id: String, carried: usize) {
    let who = author(&id);
    let questions = w.domain.questions.clone();
    let opened = w
        .app
        .open_session(session_key(&who), &who, questions, carried)
        .await;
    w.domain.opened = Some(opened);
}

#[given(regex = r#"^a session for "([^"]+)" opened with (?:questions about (.+)|no questions)$"#)]
fn session_opened(w: &mut SpecWorld, id: String, list: String) {
    w.domain.questions = if list.is_empty() {
        Vec::new()
    } else {
        concepts(&list).iter().map(mc_question).collect()
    };
    let max = w.domain.questions.len().max(1);
    open_session(w, &id, max);
    opened(w);
}

#[then("the session is opened")]
fn session_is_opened(w: &mut SpecWorld) {
    opened(w);
}

#[then("opening the session fails as invalid")]
fn opening_fails(w: &mut SpecWorld) {
    match w.domain.opened.as_ref().expect("no session was opened") {
        Err(Error::Invalid(_)) => {}
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[then(regex = r#"^the session belongs to "([^"]+)" and asks about (.+), in that order$"#)]
fn session_belongs_and_asks(w: &mut SpecWorld, id: String, list: String) {
    let now = w.clock.now();
    let s = opened(w);
    let who = author(&id);
    assert_eq!(*s.author(), who);
    assert_eq!(*s.key(), session_key(&who));
    assert_eq!(s.created_at(), now);
    let asked: Vec<ConceptId> = s.questions().iter().map(|q| q.concept.clone()).collect();
    assert_eq!(asked, concepts(&list), "concepts asked");
    assert_eq!(s.questions(), w.domain.questions.as_slice(), "questions");
}

#[when(regex = r"^the answers to the questions about (.+) are validated$")]
fn answers_validated(w: &mut SpecWorld, list: String) {
    let answers: Vec<Answer> = concepts(&list)
        .iter()
        .map(|c| Answer {
            question: FakeExaminer::question_id(c, QuestionFormat::MultipleChoice),
            body: AnswerBody::Choice(0),
        })
        .collect();
    let result = opened(w).validate_answers(&answers);
    w.domain.validated = Some(result);
}

#[then(regex = r"^the answers are (accepted|rejected as invalid)$")]
fn answers_verdict(w: &mut SpecWorld, verdict: String) {
    let result = w.domain.validated.as_ref().expect("no answers validated");
    match (verdict.as_str(), result) {
        ("accepted", Ok(())) | ("rejected as invalid", Err(Error::Invalid(_))) => {}
        (v, r) => panic!("expected the answers to be {v}, got {r:?}"),
    }
}

#[when(regex = r"^the session is concluded with pass threshold ([0-9.]+) and grades:$")]
fn concluded_with_grades(w: &mut SpecWorld, threshold: f64, step: &Step) {
    let by_concept = grades_by_concept(step);
    let grades: Vec<(QuestionId, Grade)> = opened(w)
        .questions()
        .iter()
        .filter_map(|q| by_concept.get(&q.concept).map(|g| (q.id.clone(), *g)))
        .collect();
    assert_eq!(
        grades.len(),
        by_concept.len(),
        "every graded concept must be in the session"
    );
    conclude(w, grades, threshold);
}

#[when(regex = r"^the session is concluded with pass threshold ([0-9.]+) and no grades$")]
fn concluded_without_grades(w: &mut SpecWorld, threshold: f64) {
    conclude(w, Vec::new(), threshold);
}

#[when(
    regex = r"^the session is concluded with pass threshold ([0-9.]+), (\d+) questions graded (\w+) and the rest graded Again$"
)]
fn concluded_with_n_correct(w: &mut SpecWorld, threshold: f64, n: usize, g: String) {
    let g = grade(&g);
    let questions = opened(w).questions().to_vec();
    assert!(
        n <= questions.len(),
        "the session has {} questions",
        questions.len()
    );
    let grades = questions
        .iter()
        .enumerate()
        .map(|(i, q)| (q.id.clone(), if i < n { g } else { Grade::Again }))
        .collect();
    conclude(w, grades, threshold);
}

#[then("the session grades are, in question order:")]
fn session_grades(w: &mut SpecWorld, step: &Step) {
    let table = step.table.as_ref().expect("this step needs a data table");
    let mut rows = table.rows.iter();
    assert_eq!(rows.next().expect("table header"), &["concept", "grade"]);
    let want: Vec<(QuestionId, Grade)> = rows
        .map(|r| {
            (
                FakeExaminer::question_id(&ConceptId(r[0].clone()), QuestionFormat::MultipleChoice),
                grade(&r[1]),
            )
        })
        .collect();
    assert_eq!(concluded(w).grades, want, "grades");
}

#[then(regex = r"^the session (passes|fails)$")]
fn session_passes(w: &mut SpecWorld, how: String) {
    let r = concluded(w);
    assert_eq!(r.passed, how == "passes", "result: {r:?}");
    let SessionEvent::SessionGraded { passed, .. } = &r.event;
    assert_eq!(*passed, r.passed, "SessionGraded agrees with the result");
}

#[then(regex = r"^the SessionGraded event reports (\d+) correct out of (\d+) and (passed|failed)$")]
fn session_graded_event(w: &mut SpecWorld, correct: usize, total: usize, passed: String) {
    let now = w.clock.now();
    let r = concluded(w);
    let SessionEvent::SessionGraded {
        key,
        author: who,
        correct: c,
        total: t,
        passed: p,
        at,
    } = &r.event;
    assert_eq!((*c, *t, *p), (correct, total, passed == "passed"), "{r:?}");
    assert_eq!(*p, r.passed, "SessionGraded agrees with the result");
    assert_eq!(*key, session_key(who), "session key");
    assert_eq!(*who, author("github:1001"), "author");
    assert_eq!(*at, now, "graded at");
}

// ---------------------------------------------------------------------------
// D6: the engine boundary

#[when(regex = r#"^"([^"]+)" submits the same answers to that session again$"#)]
async fn submits_again(w: &mut SpecWorld, id: String) {
    let session = engine_session(w);
    assert_eq!(*session.author(), author(&id));
    assert!(
        w.outcome.is_some(),
        "the session was not graded the first time"
    );
    let answers: Vec<_> = session
        .questions()
        .iter()
        .map(|q| FakeExaminer::answer(q, true))
        .collect();
    submit_expecting_error(w, session.key(), &answers).await;
}

#[when(regex = r#"^answers are submitted to the unknown session "([^"]+)"$"#)]
async fn submits_to_unknown(w: &mut SpecWorld, key: String) {
    submit_expecting_error(w, &SessionKey(key), &[]).await;
}

#[when(regex = r#"^"([^"]+)" submits an answer to a question that is not in the session$"#)]
async fn submits_foreign_answer(w: &mut SpecWorld, id: String) {
    let session = engine_session(w);
    assert_eq!(*session.author(), author(&id));
    let answers = vec![Answer {
        question: QuestionId("not.in.this.session#mc".into()),
        body: AnswerBody::Choice(0),
    }];
    submit_expecting_error(w, session.key(), &answers).await;
}

#[when(regex = r#"^"([^"]+)" submits two answers to the first question of the session$"#)]
async fn submits_two_answers(w: &mut SpecWorld, id: String) {
    let session = engine_session(w);
    assert_eq!(*session.author(), author(&id));
    let first = session
        .questions()
        .first()
        .expect("the session asks nothing");
    let answers = vec![
        FakeExaminer::answer(first, true),
        FakeExaminer::answer(first, false),
    ];
    submit_expecting_error(w, session.key(), &answers).await;
}

#[then(regex = r"^the submission fails as (not found|invalid)$")]
fn submission_fails(w: &mut SpecWorld, how: String) {
    let e = w
        .domain
        .submit_error
        .as_ref()
        .expect("no failed submission");
    match (how.as_str(), e) {
        ("not found", Error::NotFound(_)) | ("invalid", Error::Invalid(_)) => {}
        (h, e) => panic!("expected {h}, got {e:?}"),
    }
}

// ---------------------------------------------------------------------------
// D7: SessionOutcome::events

#[then(
    regex = r"^the outcome events hold (\d+) (ReviewRecorded|ReviewScheduled|LevelChanged|SessionGraded) events?$"
)]
fn outcome_event_count(w: &mut SpecWorld, n: usize, name: String) {
    let events = &outcome(w).events;
    let got = events
        .iter()
        .filter(|e| domain_event_name(e) == name)
        .count();
    assert_eq!(got, n, "{name} events in {events:?}");
}

#[then(regex = r"^the outcome events record a review of each of (.+)$")]
async fn outcome_reviews_each(w: &mut SpecWorld, list: String) {
    let session = engine_session(w);
    let who = learner_of(w, session.author()).await;
    let events = &outcome(w).events;
    for c in concepts(&list) {
        let recorded = events
            .iter()
            .filter(|e| {
                matches!(e, DomainEvent::Learner(LearnerEvent::ReviewRecorded { learner, review })
                    if *learner == who && review.concept == c)
            })
            .count();
        let scheduled = events
            .iter()
            .filter(|e| {
                matches!(e, DomainEvent::Learner(LearnerEvent::ReviewScheduled { learner, concept, .. })
                    if *learner == who && *concept == c)
            })
            .count();
        assert_eq!(
            (recorded, scheduled),
            (1, 1),
            "ReviewRecorded and ReviewScheduled for {c:?} in {events:?}"
        );
    }
}

#[then(
    regex = r#"^the ReviewScheduled events in the outcome match the review cards of "([^"]+)"$"#
)]
async fn outcome_scheduled_match_cards(w: &mut SpecWorld, id: String) {
    let p = w.profile(&author(&id)).await;
    let events = outcome(w).events.clone();
    let mut seen = 0;
    for e in &events {
        if let DomainEvent::Learner(LearnerEvent::ReviewScheduled { concept, due, .. }) = e {
            let card = p
                .card(concept)
                .unwrap_or_else(|| panic!("no review card for {concept:?}"));
            assert_eq!(*due, card.due(), "due date of {concept:?}");
            seen += 1;
        }
    }
    assert!(seen > 0, "no ReviewScheduled events in {events:?}");
}

#[then(regex = r"^the outcome events hold a LevelChanged event from (\w+) to (\w+)$")]
async fn outcome_level_changed(w: &mut SpecWorld, from: String, to: String) {
    let (from, to) = (level(&from), level(&to));
    let session = engine_session(w);
    let who = learner_of(w, session.author()).await;
    let o = outcome(w);
    let found = o.events.iter().any(|e| {
        matches!(e, DomainEvent::Learner(LearnerEvent::LevelChanged { learner, from: f, to: t, .. })
            if *learner == who && *f == from && *t == to)
    });
    assert!(
        found,
        "no LevelChanged {from:?} -> {to:?} in {:?}",
        o.events
    );
    assert_eq!(o.level_after, to, "level after the session");
}

#[then(
    regex = r"^the outcome events hold exactly one SessionGraded event, reporting (\d+) correct out of (\d+) and (passed|failed)$"
)]
fn outcome_session_graded(w: &mut SpecWorld, correct: usize, total: usize, passed: String) {
    let o = outcome(w);
    let session = engine_session(w);
    let graded: Vec<&SessionEvent> = o
        .events
        .iter()
        .filter_map(|e| match e {
            DomainEvent::Session(s) => Some(s),
            DomainEvent::Learner(_) => None,
        })
        .collect();
    assert_eq!(graded.len(), 1, "SessionGraded events in {:?}", o.events);
    let SessionEvent::SessionGraded {
        key,
        author: who,
        correct: c,
        total: t,
        passed: p,
        ..
    } = graded[0];
    assert_eq!(key, session.key(), "session key");
    assert_eq!(who, session.author(), "author");
    assert_eq!((*c, *t, *p), (correct, total, passed == "passed"));
    assert_eq!(*p, o.passed, "SessionGraded agrees with the outcome");
}
