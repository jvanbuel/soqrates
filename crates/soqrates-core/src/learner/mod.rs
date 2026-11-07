//! Learner context: what Soqrates knows about each author (ADR-0002).
//! Uses the shared kernel and `assessment`.

pub mod identity;
pub mod policy;
pub mod schedule;

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::assessment::{Grade, QuestionFormat};
use crate::ids::{ConceptId, Identity, LearnerId};
pub use identity::{IdentityLink, LINK_CODE_TTL, LinkCode, LinkRejection};
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
    /// The identity the review came through (ADR-0003 D3).
    pub via: Identity,
}

/// Events emitted by [`LearnerProfile`] (ADR-0002 D7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LearnerEvent {
    ReviewRecorded {
        learner: LearnerId,
        review: GradedAnswer,
    },
    ReviewScheduled {
        learner: LearnerId,
        concept: ConceptId,
        due: DateTime<Utc>,
    },
    LevelChanged {
        learner: LearnerId,
        from: Level,
        to: Level,
        at: DateTime<Utc>,
    },
}

/// Everything Soqrates knows about one learner. An aggregate (ADR-0002 D3):
/// the fields are private and change only through [`LearnerProfile::record`]
/// (and, across learners, by replaying reviews: ADR-0003 D3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LearnerProfile {
    learner: LearnerId,
    /// The level the learner started at (ADR-0002 D4); replays start here.
    starting_level: Level,
    level: Level,
    /// One FSRS card per concept the author has been asked about.
    cards: BTreeMap<ConceptId, ReviewCard>,
    /// All reviews, oldest first.
    history: Vec<GradedAnswer>,
    /// Index into `history` of the first review since the last level change.
    level_since: usize,
}

impl LearnerProfile {
    /// The profile of a learner Soqrates has never seen: `Novice` (ADR-0002 D4).
    pub fn new(learner: LearnerId) -> Self {
        Self::starting_at(learner, Level::Novice)
    }

    /// A new profile for a learner who declares a starting level (ADR-0002 D4).
    pub fn starting_at(learner: LearnerId, level: Level) -> Self {
        Self {
            learner,
            starting_level: level,
            level,
            cards: BTreeMap::new(),
            history: Vec::new(),
            level_since: 0,
        }
    }

    pub fn learner(&self) -> &LearnerId {
        &self.learner
    }

    pub fn starting_level(&self) -> Level {
        self.starting_level
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
        let mut due: Vec<_> = self
            .cards
            .iter()
            .filter(|(_, card)| card.is_due(now))
            .map(|(concept, card)| (card.due(), concept.clone()))
            .collect();
        due.sort();
        due.into_iter().map(|(_, concept)| concept).collect()
    }

    /// Record a review: reschedule the concept's card (creating it if new),
    /// append to the history and re-evaluate the level (ADR-0002 D3, D4, D7).
    /// Returns `ReviewRecorded`, `ReviewScheduled` and, if the level changed,
    /// `LevelChanged`. `via` is the identity the review came through.
    pub fn record(
        &mut self,
        concept: &ConceptId,
        format: QuestionFormat,
        grade: Grade,
        at: DateTime<Utc>,
        via: &Identity,
    ) -> Vec<LearnerEvent> {
        let card = self
            .cards
            .entry(concept.clone())
            .or_insert_with(|| ReviewCard::new(at));
        card.review(grade, at);
        let due = card.due();

        let review = GradedAnswer {
            concept: concept.clone(),
            format,
            grade,
            at,
            via: via.clone(),
        };
        self.history.push(review.clone());
        let mut events = vec![
            LearnerEvent::ReviewRecorded {
                learner: self.learner.clone(),
                review,
            },
            LearnerEvent::ReviewScheduled {
                learner: self.learner.clone(),
                concept: concept.clone(),
                due,
            },
        ];

        let next = policy::next_level(self.level, self.reviews_since_level_change());
        if next != self.level {
            events.push(LearnerEvent::LevelChanged {
                learner: self.learner.clone(),
                from: self.level,
                to: next,
                at,
            });
            self.level = next;
            // The review that changed the level does not count towards the
            // next streak (ADR-0002 D4).
            self.level_since = self.history.len();
        }
        events
    }

    /// A profile of `learner` starting at `level`, with `reviews` replayed
    /// in time order (ADR-0003 D3).
    fn replayed<'a>(
        learner: LearnerId,
        level: Level,
        reviews: impl IntoIterator<Item = &'a GradedAnswer>,
    ) -> LearnerProfile {
        let mut reviews: Vec<&GradedAnswer> = reviews.into_iter().collect();
        reviews.sort_by_key(|r| r.at);
        let mut profile = LearnerProfile::starting_at(learner, level);
        for r in reviews {
            profile.record(&r.concept, r.format, r.grade, r.at, &r.via);
        }
        profile
    }

    /// The profile of `learner` that results from replaying the reviews of
    /// all `profiles` in time order through [`LearnerProfile::record`],
    /// starting at the lowest of their starting levels (ADR-0003 D3).
    pub fn merged(learner: LearnerId, profiles: &[LearnerProfile]) -> LearnerProfile {
        let level = profiles
            .iter()
            .map(|p| p.starting_level)
            .min()
            .unwrap_or_default();
        Self::replayed(learner, level, profiles.iter().flat_map(|p| &p.history))
    }

    /// Split off the reviews that came through `identity` into a profile of
    /// `new_learner`. Both parts are rebuilt by replay from this profile's
    /// starting level (ADR-0003 D3). Returns `(remaining, split_off)`.
    pub fn split(
        &self,
        identity: &Identity,
        new_learner: LearnerId,
    ) -> (LearnerProfile, LearnerProfile) {
        let (split_off, remaining): (Vec<&GradedAnswer>, Vec<&GradedAnswer>) =
            self.history.iter().partition(|r| &r.via == identity);
        (
            Self::replayed(self.learner.clone(), self.starting_level, remaining),
            Self::replayed(new_learner, self.starting_level, split_off),
        )
    }
}
