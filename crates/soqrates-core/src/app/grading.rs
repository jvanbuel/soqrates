//! Grading: the session's life after it is asked, grading its answers
//! through a journal written ahead, and the queries for where it is
//! (ADR-0002 D6).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::app::engine::Soqrates;
use crate::app::events::DomainEvent;
use crate::app::ports::{Clock, Codebase, Examiner, StateStore};
use crate::app::stores::{Access, ProfileLocation, Recorded, grading_mark, load_or_new};
use crate::assessment::{Answer, AnswerBody, ConceptScope, Grade, QuestionBody};
use crate::delivery::{
    GradedSession, GradingJournal, JournaledReview, QuestionResult, Session, StoreId,
};
use crate::ids::{ConceptId, QuestionId, SessionKey};
use crate::learner::{GradedAnswer, LearnerEvent, Level};
use crate::{Error, Result};

/// The result of grading a session, as returned by [`Soqrates::submit`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionOutcome {
    /// One grade per question, in question order.
    pub grades: Vec<(QuestionId, Grade)>,
    pub passed: bool,
    pub level_after: Level,
    /// What happened, in order (ADR-0002 D7).
    pub events: Vec<DomainEvent>,
    /// Where the general reviews were stored (ADR-0003 D4).
    pub profile_location: ProfileLocation,
    /// Per-question feedback, in question order (ADR-0004 D1).
    pub results: Vec<QuestionResult>,
}

impl SessionOutcome {
    /// Replace what was worked out at grading time by what `recorded`
    /// says happened: the level of the profile in `primary`, the review
    /// events and the next review dates. The grades and the session's own
    /// event stay.
    fn correct_by(&mut self, recorded: Recorded, primary: &StoreId) {
        let concluded = self.events.pop();
        let count = recorded.by_review.keys().max().map_or(0, |last| last + 1);
        self.events = recorded.events_in_order(count);
        self.events.extend(concluded);
        if let Some(level) = recorded.level_of(primary) {
            self.level_after = level;
        }
        for r in &mut self.results {
            r.next_review = next_review_of(&self.events, &r.concept);
        }
    }
}

/// Where a session is in its life (ADR-0002 D6).
#[derive(Debug, Clone)]
pub enum SessionState {
    /// No such session, or one that was discarded.
    Absent,
    /// Asked, not graded yet.
    Pending(Session),
    /// Its answers were graded but not all results are recorded yet.
    Grading,
    /// Graded.
    Graded(GradedSession),
}

impl<S: StateStore, E: Examiner, C: Clock> Soqrates<S, E, C> {
    /// Remove the pending session `key` without grading it, e.g. when its
    /// questions could not be posted.
    pub async fn discard_session(&self, key: &SessionKey) -> Result<()> {
        let stores = self.stores().best_effort().await;
        if let Some((_, store)) = stores.find_session(key).await? {
            store.delete_session(key).await?;
        }
        Ok(())
    }

    /// Where the session `key` is in its life (ADR-0002 D6). The one query
    /// for it: the lookups run in the order a grading moves through, pending,
    /// then being graded, then graded, so a grading by another run is never
    /// missed and no new session is opened for its key.
    pub async fn session_state(&self, key: &SessionKey) -> Result<SessionState> {
        let stores = self.stores().best_effort().await;
        if let Some((session, _)) = stores.find_session(key).await? {
            return Ok(SessionState::Pending(session));
        }
        // The journal is looked up before the graded record, never after:
        // a grading that finishes in between is then found as graded. A
        // journal left behind next to a graded record only needs removing,
        // so the record wins.
        let grading = stores.grading_recorded(key).await?;
        Ok(match stores.graded_record(key).await? {
            Some(graded) => SessionState::Graded(graded),
            None if grading => SessionState::Grading,
            None => SessionState::Absent,
        })
    }

    /// The record of the graded session `key`, if it was graded.
    pub async fn graded(&self, key: &SessionKey) -> Result<Option<GradedSession>> {
        // Best effort: a grading that can't be finished yet is reported as
        // being graded (ADR-0002 D6).
        let stores = self.stores().best_effort().await;
        stores.graded_record(key).await
    }

    /// The pending (not yet graded) session `key`, if any.
    pub async fn pending_session(&self, key: &SessionKey) -> Result<Option<Session>> {
        Ok(match self.session_state(key).await? {
            SessionState::Pending(session) => Some(session),
            _ => None,
        })
    }

    /// Whether the session `key` is being graded: its answers were graded,
    /// but not all of the results are recorded yet (ADR-0002 D6).
    pub async fn being_graded(&self, key: &SessionKey) -> Result<bool> {
        let stores = self.stores().best_effort().await;
        stores.grading_recorded(key).await
    }

    /// Grade `answers` for the session `key`, record each review on the
    /// author's profile, persist it and remove the session. Fails with
    /// `NotFound` for an unknown or already graded session and `Invalid`
    /// for answers that don't fit the session (ADR-0002 D6).
    ///
    /// Grading is written ahead: claiming the session and journaling the
    /// reviews to record are one atomic step, then the journal is applied.
    /// If applying fails, the journal stays and a later operation finishes
    /// it.
    ///
    /// The grader may read `code`, the codebase the session was asked
    /// about (ADR-0008 D4).
    pub async fn submit(
        &self,
        key: &SessionKey,
        answers: &[Answer],
        code: &impl Codebase,
    ) -> Result<SessionOutcome> {
        let stores = self.stores().strict().await?;
        let not_found = || Error::NotFound(format!("session {:?}", key.0));
        let (session, session_store) = stores.find_session(key).await?.ok_or_else(not_found)?;
        let (journal, mut outcome, primary) = self.grade(&stores, session, answers, code).await?;
        // Of several concurrent submissions, exactly one claims the session.
        if !session_store.begin_grading(&journal).await? {
            return Err(not_found());
        }
        if let Some(recorded) = stores.apply(session_store, &journal).await? {
            // Applying recorded the reviews on the profiles as they are now,
            // which may differ from the ones grading saw if another session
            // of the learner was graded meanwhile: report what happened.
            outcome.correct_by(recorded, &primary);
        }
        Ok(outcome)
    }

    /// Grade `answers` to `session`: the journal of everything grading
    /// writes, the outcome as the profiles look now, and the store of the
    /// general reviews. Writes no reviews.
    async fn grade(
        &self,
        stores: &Access<'_, S, C>,
        session: Session,
        answers: &[Answer],
        code: &impl Codebase,
    ) -> Result<(GradingJournal, SessionOutcome, StoreId)> {
        let now = self.clock.now();
        session.validate_answers(answers)?;

        let mut grades = Vec::new();
        let mut feedback: Vec<(QuestionId, String)> = Vec::new();
        for q in session.questions() {
            if let Some(a) = answers.iter().find(|a| a.question == q.id) {
                let grading = self.examiner.grade(q, a, code).await?;
                grades.push((q.id.clone(), grading.grade));
                feedback.push((q.id.clone(), grading.feedback));
            }
        }
        let claimed = session.clone();
        let key = session.key().clone();
        let author = session.author().clone();
        let questions = session.questions().to_vec();
        let result = session.conclude(grades, self.config.pass_threshold, now);

        let place = stores.place(&author).await?;
        let identity = author.identity()?;
        let primary = load_or_new(place.primary, &place.learner).await?;
        let project = match place.project {
            Some(store) => Some(load_or_new(store, &place.learner).await?),
            None => None,
        };
        let reviews: Vec<JournaledReview> = questions
            .iter()
            .zip(&result.grades)
            .map(|(q, (_, grade))| {
                let store = match (&place.project_id, q.scope) {
                    (Some(id), ConceptScope::Project) => id.clone(),
                    _ => place.primary_id.clone(),
                };
                JournaledReview {
                    store,
                    learner: place.learner.clone(),
                    review: GradedAnswer {
                        concept: q.concept.clone(),
                        format: q.format(),
                        grade: *grade,
                        at: now,
                        via: identity.clone(),
                    },
                }
            })
            .collect();
        let graded = GradedSession {
            key: key.clone(),
            author,
            correct: result.grades.iter().filter(|(_, g)| g.is_correct()).count(),
            total: result.grades.len(),
            passed: result.passed,
            at: now,
        };
        let journal = GradingJournal {
            session: claimed,
            graded,
            reviews,
            profiles: Vec::new(),
        };

        // What the reviews will do to the profiles as they are now; the
        // profiles are not saved, applying the journal does that.
        let mut profiles = vec![(place.primary_id.clone(), primary)];
        if let (Some(id), Some(p)) = (place.project_id.clone(), project) {
            profiles.push((id, p));
        }
        let mut recorded = Recorded::default();
        for (id, profile) in profiles {
            let indices = journal.review_indices(&id);
            let reviews: Vec<GradedAnswer> = indices
                .iter()
                .map(|&i| journal.reviews[i].review.clone())
                .collect();
            let next = profile
                .with_session_reviews(&grading_mark(&key), &reviews)
                .map_or(profile, |(next, events)| {
                    recorded.add(&indices, events);
                    next
                });
            recorded.levels.push((id, next.level()));
        }
        let mut events = recorded.events_in_order(journal.reviews.len());
        events.push(result.event.into());
        let results = question_results(&questions, answers, &result.grades, &feedback, &events);
        let level_after = recorded.level_of(&place.primary_id).unwrap_or_default();
        let outcome = SessionOutcome {
            grades: result.grades,
            passed: result.passed,
            level_after,
            events,
            profile_location: place.location,
            results,
        };
        Ok((journal, outcome, place.primary_id))
    }
}

/// When `concept` is next due, by the last review scheduled in `events`.
fn next_review_of(events: &[DomainEvent], concept: &ConceptId) -> Option<DateTime<Utc>> {
    events.iter().rev().find_map(|e| match e {
        DomainEvent::Learner(LearnerEvent::ReviewScheduled {
            concept: c, due, ..
        }) if c == concept => Some(*due),
        _ => None,
    })
}

/// Per-question results for the author (ADR-0004 D1): the answer, the
/// correct option, feedback, and when the concept is next due.
fn question_results(
    questions: &[crate::assessment::Question],
    answers: &[Answer],
    grades: &[(QuestionId, Grade)],
    feedback: &[(QuestionId, String)],
    events: &[DomainEvent],
) -> Vec<QuestionResult> {
    questions
        .iter()
        .zip(grades)
        .map(|(q, (_, grade))| {
            let (prompt, options, correct_option) = match &q.body {
                QuestionBody::MultipleChoice {
                    prompt,
                    options,
                    correct,
                    ..
                } => (prompt.clone(), options.clone(), Some(*correct)),
                QuestionBody::Open { prompt, .. } => (prompt.clone(), Vec::new(), None),
            };
            let answer: Option<AnswerBody> = answers
                .iter()
                .find(|a| a.question == q.id)
                .map(|a| a.body.clone());
            let feedback = feedback
                .iter()
                .find(|(id, _)| *id == q.id)
                .map(|(_, f)| f.clone())
                .filter(|f| !f.trim().is_empty())
                .unwrap_or_else(|| match (&answer, correct_option) {
                    (None, _) => format!("Not answered. {}", q.explanation),
                    // A wrong option first gets its own reason, so the author
                    // learns why their pick was wrong, then why the correct
                    // one is right.
                    (Some(AnswerBody::Choice(picked)), Some(correct)) if *picked != correct => {
                        match q.body.reason(*picked) {
                            Some(why) => format!("{why} {}", q.explanation),
                            None => q.explanation.clone(),
                        }
                    }
                    _ => q.explanation.clone(),
                });
            let next_review = next_review_of(events, &q.concept);
            QuestionResult {
                question: q.id.clone(),
                concept: q.concept.clone(),
                prompt,
                options,
                correct_option,
                answer,
                grade: *grade,
                feedback,
                next_review,
            }
        })
        .collect()
}
