//! The Soqrates engine: starts and grades sessions.

use serde::{Deserialize, Serialize};

use crate::Result;
use crate::model::*;
use crate::ports::{Clock, Examiner, StateStore};

/// Whether a failed quiz blocks (D8, D9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Mode {
    /// Report the result, never block.
    #[default]
    Advisory,
    /// A failed quiz blocks the commit/push or fails the check.
    Blocking,
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

    /// Concepts whose review is due for `author`, most overdue first (D6).
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

    /// Grade `answers` for the session `key`, update the author's cards and
    /// level, persist the profile and remove the session (D5, D6, D7).
    pub async fn submit(&self, key: &SessionKey, answers: &[Answer]) -> Result<SessionOutcome> {
        let _ = (key, answers);
        todo!("ADR-0001 D5/D6/D7")
    }
}
