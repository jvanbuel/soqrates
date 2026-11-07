//! The Soqrates engine: orchestrates the aggregates (load, call, save,
//! emit events). Holds no domain rules (ADR-0002 D3).

use serde::{Deserialize, Serialize};

use crate::Result;
use crate::app::events::DomainEvent;
use crate::app::ports::{Clock, Examiner, StateStore};
use crate::assessment::{Answer, Diff, Grade};
use crate::delivery::{Mode, Session};
use crate::ids::{AuthorId, ConceptId, QuestionId, SessionKey};
use crate::learner::{LearnerProfile, Level};

/// The result of grading a session, as returned by [`Soqrates::submit`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionOutcome {
    /// One grade per question, in question order.
    pub grades: Vec<(QuestionId, Grade)>,
    pub passed: bool,
    pub level_after: Level,
    /// What happened, in order (ADR-0002 D7).
    pub events: Vec<DomainEvent>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    /// Upper bound on questions per session, due reviews included.
    pub max_questions: usize,
    /// Fraction of correct answers needed to pass, in `0.0..=1.0`.
    pub pass_threshold: f64,
    pub mode: Mode,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_questions: 3,
            pass_threshold: 0.6,
            mode: Mode::Advisory,
        }
    }
}

pub struct Soqrates<S, E, C> {
    pub store: S,
    pub examiner: E,
    pub clock: C,
    pub config: Config,
}

impl<S: StateStore, E: Examiner, C: Clock> Soqrates<S, E, C> {
    pub fn new(store: S, examiner: E, clock: C, config: Config) -> Self {
        Self {
            store,
            examiner,
            clock,
            config,
        }
    }

    /// The author's profile, or a fresh `Novice` profile if unknown (D5, D7).
    pub async fn profile(&self, author: &AuthorId) -> Result<LearnerProfile> {
        let _ = author;
        todo!("ADR-0001 D5")
    }

    /// Concepts whose review is due for `author` (ADR-0001 D6, ADR-0002 D8).
    pub async fn due_reviews(&self, author: &AuthorId) -> Result<Vec<ConceptId>> {
        let _ = author;
        todo!("ADR-0001 D6")
    }

    /// Create and persist a session for `author` about `diff`: due reviews
    /// first, then new concepts from the diff, in the format the author's
    /// level calls for, at most `config.max_questions` questions (D6, D7).
    pub async fn start_session(
        &self,
        key: SessionKey,
        author: &AuthorId,
        diff: &Diff,
    ) -> Result<Session> {
        let _ = (key, author, diff);
        todo!("ADR-0001 D6/D7")
    }

    /// Grade `answers` for the session `key`, record each review on the
    /// author's profile, persist it and remove the session. Fails with
    /// `NotFound` for an unknown or already graded session and `Invalid`
    /// for answers that don't fit the session (ADR-0002 D6).
    pub async fn submit(&self, key: &SessionKey, answers: &[Answer]) -> Result<SessionOutcome> {
        let _ = (key, answers);
        todo!("ADR-0001 D5/D6/D7")
    }
}
