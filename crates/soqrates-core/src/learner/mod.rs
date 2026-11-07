//! Learner context: what Soqrates knows about each author (ADR-0002).
//! Uses the shared kernel and `assessment`.

pub mod policy;
pub mod schedule;

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::assessment::{Grade, QuestionFormat};
use crate::ids::{AuthorId, ConceptId};
pub use schedule::ReviewCard;

/// Knowledge level of an author (ADR-0001 D7, ADR-0002 D4).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub enum Level {
    #[default]
    Novice,
    Intermediate,
    Expert,
}

/// A review: one graded answer recorded on a profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GradedAnswer {
    pub concept: ConceptId,
    pub format: QuestionFormat,
    pub grade: Grade,
    pub at: DateTime<Utc>,
}

/// Events emitted by [`LearnerProfile`] (ADR-0002 D7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LearnerEvent {
    ReviewRecorded {
        author: AuthorId,
        review: GradedAnswer,
    },
    ReviewScheduled {
        author: AuthorId,
        concept: ConceptId,
        due: DateTime<Utc>,
    },
    LevelChanged {
        author: AuthorId,
        from: Level,
        to: Level,
        at: DateTime<Utc>,
    },
}

/// Everything Soqrates knows about one author. An aggregate (ADR-0002 D3):
/// the fields are private and change only through [`LearnerProfile::record`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LearnerProfile {
    author: AuthorId,
    level: Level,
    /// One FSRS card per concept the author has been asked about.
    cards: BTreeMap<ConceptId, ReviewCard>,
    /// All reviews, oldest first.
    history: Vec<GradedAnswer>,
    /// Index into `history` of the first review since the last level change.
    level_since: usize,
}

impl LearnerProfile {
    /// The profile of an author Soqrates has never seen: `Novice` (ADR-0002 D4).
    pub fn new(author: AuthorId) -> Self {
        Self::starting_at(author, Level::Novice)
    }

    /// A new profile for an author who declares a starting level (ADR-0002 D4).
    pub fn starting_at(author: AuthorId, level: Level) -> Self {
        Self {
            author,
            level,
            cards: BTreeMap::new(),
            history: Vec::new(),
            level_since: 0,
        }
    }

    pub fn author(&self) -> &AuthorId {
        &self.author
    }

    pub fn level(&self) -> Level {
        self.level
    }

    pub fn cards(&self) -> &BTreeMap<ConceptId, ReviewCard> {
        &self.cards
    }

    pub fn card(&self, concept: &ConceptId) -> Option<&ReviewCard> {
        self.cards.get(concept)
    }

    /// All reviews, oldest first.
    pub fn history(&self) -> &[GradedAnswer] {
        &self.history
    }

    /// Reviews since the last level change, oldest first.
    pub fn reviews_since_level_change(&self) -> &[GradedAnswer] {
        &self.history[self.level_since..]
    }

    /// Concepts whose review card is due at `now`: earliest due first, ties
    /// broken by concept ID in ascending order (ADR-0002 D8).
    pub fn due_reviews(&self, now: DateTime<Utc>) -> Vec<ConceptId> {
        let _ = now;
        todo!("ADR-0002 D8")
    }

    /// Record a review: reschedule the concept's card (creating it if new),
    /// append to the history and re-evaluate the level (ADR-0002 D3, D4, D7).
    /// Returns `ReviewRecorded`, `ReviewScheduled` and, if the level changed,
    /// `LevelChanged`.
    pub fn record(
        &mut self,
        concept: &ConceptId,
        format: QuestionFormat,
        grade: Grade,
        at: DateTime<Utc>,
    ) -> Vec<LearnerEvent> {
        let _ = (concept, format, grade, at);
        todo!("ADR-0002 D3/D4/D7")
    }
}
