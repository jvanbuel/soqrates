//! Learner context: what Soqrates knows about each author (ADR-0002).
//! Uses the shared kernel and `assessment`.

pub mod identity;
pub mod policy;
pub mod schedule;

use std::collections::{BTreeMap, BTreeSet};

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
    /// How often this profile was saved, as the store counts it. A save
    /// that expects a version other than the stored one is refused, so two
    /// writers of one profile cannot overwrite each other (ADR-0002 D6).
    /// Profiles stored before this existed load as version 0.
    #[serde(default)]
    version: u64,
    /// Marks of the gradings whose reviews are on this profile, so that
    /// applying a grading journal again records nothing twice (ADR-0002 D6).
    /// A mark is opaque: the learner store may not hold PR references
    /// (ADR-0003 D5). The set only grows, one mark per graded session; it is
    /// left unbounded for now, since bounding it needs a way to tell that
    /// no journal of an old session can come back.
    #[serde(default)]
    applied: BTreeSet<String>,
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
            version: 0,
            applied: BTreeSet::new(),
        }
    }

    /// The version of this profile; see [`LearnerProfile::with_version`].
    pub fn version(&self) -> u64 {
        self.version
    }

    /// This profile at `version`. For store adapters, which store a profile
    /// one version after the one it was loaded at (ADR-0002 D6).
    pub fn with_version(mut self, version: u64) -> Self {
        self.version = version;
        self
    }

    /// Whether the reviews marked `mark` are on this profile.
    pub fn has_applied(&self, mark: &str) -> bool {
        self.applied.contains(mark)
    }

    /// This profile with `reviews`, the reviews of one grading, recorded in
    /// order through [`LearnerProfile::record`], and the events each
    /// emitted. The grading is identified by `mark`, an opaque string that
    /// is the same each time the same session is applied. `None` if the
    /// reviews marked `mark` are on this profile already: recording them
    /// again would count them twice (ADR-0002 D6).
    pub fn with_session_reviews(
        &self,
        mark: &str,
        reviews: &[GradedAnswer],
    ) -> Option<(LearnerProfile, Vec<Vec<LearnerEvent>>)> {
        if self.has_applied(mark) {
            return None;
        }
        let mut next = self.clone();
        let events = reviews
            .iter()
            .map(|r| next.record(&r.concept, r.format, r.grade, r.at, &r.via))
            .collect();
        next.applied.insert(mark.to_string());
        Some((next, events))
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
        let mut merged = Self::replayed(learner, level, profiles.iter().flat_map(|p| &p.history));
        merged.applied = profiles.iter().flat_map(|p| p.applied.clone()).collect();
        // Newer than every part, so a writer that loaded one of them is
        // refused rather than overwriting the merge.
        merged.version = profiles.iter().map(|p| p.version).max().unwrap_or(0) + 1;
        merged
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
        let mut remaining = Self::replayed(self.learner.clone(), self.starting_level, remaining);
        remaining.applied = self.applied.clone();
        let mut split_off = Self::replayed(new_learner, self.starting_level, split_off);
        // Newer than this profile, so a writer that loaded it is refused.
        remaining.version = self.version + 1;
        split_off.version = self.version + 1;
        (remaining, split_off)
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn review(concept: &str, grade: Grade, minute: u32) -> GradedAnswer {
        GradedAnswer {
            concept: ConceptId(concept.into()),
            format: QuestionFormat::MultipleChoice,
            grade,
            at: Utc.with_ymd_and_hms(2026, 1, 5, 9, minute, 0).unwrap(),
            via: Identity::parse("github:1001").unwrap(),
        }
    }

    // @adr-0002-d6
    #[test]
    fn reviews_of_a_session_are_recorded_once_however_often_they_are_applied() {
        let reviews = [
            review("rust.ownership", Grade::Good, 0),
            review("rust.traits", Grade::Again, 1),
        ];
        let profile = LearnerProfile::new(LearnerId("ada".into()));
        let (once, events) = profile.with_session_reviews("s1", &reviews).unwrap();
        assert_eq!(events.len(), 2, "the events of each review");
        assert_eq!(once.history().len(), 2);
        assert!(once.has_applied("s1"));
        // The original is untouched: nothing is recorded until it is saved.
        assert!(profile.history().is_empty());

        let mut again = once.clone();
        for _ in 0..3 {
            assert!(again.with_session_reviews("s1", &reviews).is_none());
            if let Some((next, _)) = again.with_session_reviews("s2", &[]) {
                again = next;
            }
        }
        assert_eq!(again.history(), once.history());
        assert_eq!(again.cards(), once.cards());
        assert_eq!(again.level(), once.level());
        assert_eq!(again.version(), once.version());
    }

    // @adr-0002-d6
    #[test]
    fn a_profile_stored_without_version_or_marks_still_loads() {
        let profile = LearnerProfile::new(LearnerId("ada".into()));
        let mut doc = serde_json::to_value(&profile).unwrap();
        let fields = doc.as_object_mut().unwrap();
        fields.remove("version").expect("a version is stored");
        fields.remove("applied").expect("the marks are stored");
        let old: LearnerProfile = serde_json::from_value(doc).unwrap();
        assert_eq!(old, profile);
        assert_eq!(old.version(), 0);
    }

    // @adr-0002-d6
    #[test]
    fn merging_and_splitting_produce_a_newer_version_and_keep_the_marks() {
        let reviews = [review("rust.ownership", Grade::Good, 0)];
        let ada = LearnerProfile::new(LearnerId("ada".into()));
        let (ada, _) = ada.with_session_reviews("s1", &reviews).unwrap();
        let ada = ada.with_version(4);
        let bob = LearnerProfile::new(LearnerId("bob".into())).with_version(2);
        let merged = LearnerProfile::merged(LearnerId("ada".into()), &[ada.clone(), bob]);
        assert_eq!(merged.version(), 5);
        assert!(merged.has_applied("s1"));
        let (remaining, split_off) = ada.split(
            &Identity::parse("github:1001").unwrap(),
            LearnerId("new".into()),
        );
        assert_eq!(remaining.version(), 5);
        assert_eq!(split_off.version(), 5);
        assert!(remaining.has_applied("s1"));
    }
}
