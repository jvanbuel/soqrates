//! Applying a grading journal (ADR-0002 D6): the writes that grading a
//! session leaves to be made, and what they did. Part of the router because
//! recovery is nothing but applying the journals found in its stores.

use std::collections::BTreeMap;

use super::{Access, load_or_new, opaque_id};
use crate::app::events::DomainEvent;
use crate::app::ports::{Clock, StateStore};
use crate::delivery::{GradingJournal, StoreId};
use crate::ids::SessionKey;
use crate::learner::{GradedAnswer, LearnerEvent, Level};
use crate::{Error, Result};

/// What recording the reviews of one grading did to the profiles.
#[derive(Debug, Default)]
pub(in crate::app) struct Recorded {
    /// The events of each review, indexed like `GradingJournal::reviews`.
    pub by_review: BTreeMap<usize, Vec<LearnerEvent>>,
    /// The level of each profile after the reviews.
    pub levels: Vec<(StoreId, Level)>,
}

impl Recorded {
    /// Add the `events` of the reviews at `indices`, one entry per review.
    pub fn add(&mut self, indices: &[usize], events: Vec<Vec<LearnerEvent>>) {
        self.by_review.extend(indices.iter().copied().zip(events));
    }

    /// The events of all `count` reviews, in review order.
    pub fn events_in_order(&self, count: usize) -> Vec<DomainEvent> {
        (0..count)
            .filter_map(|i| self.by_review.get(&i))
            .flatten()
            .cloned()
            .map(DomainEvent::from)
            .collect()
    }

    pub fn level_of(&self, store: &StoreId) -> Option<Level> {
        self.levels
            .iter()
            .find(|(id, _)| id == store)
            .map(|(_, l)| *l)
    }
}

/// The mark a profile keeps of a graded session (ADR-0002 D6): opaque, so
/// that the learner store holds no PR reference (ADR-0003 D5), and 128 bits
/// so that two sessions of one learner don't share one.
pub(in crate::app) fn grading_mark(key: &SessionKey) -> String {
    format!(
        "{}{}",
        opaque_id(&key.0),
        opaque_id(&format!("grading:{}", key.0))
    )
}

impl<S: StateStore, C: Clock> Access<'_, S, C> {
    /// Make the writes of `journal`, kept in `store`, then remove it. Every
    /// write is idempotent, so this can run any number of times, also
    /// concurrently (ADR-0002 D6). Returns what recording the reviews did,
    /// or `None` if the reviews were on the profiles already.
    pub async fn apply(&self, store: &S, journal: &GradingJournal) -> Result<Option<Recorded>> {
        let key = journal.key();
        let mut recorded = None;
        // The graded record is written last: once it exists, the reviews
        // are recorded. Without it, recording them again is safe all the
        // same, since the profiles say which sessions they have.
        if store.load_graded(key).await?.is_none() {
            if journal.reviews.is_empty() && !journal.profiles.is_empty() {
                // Written before reviews were journaled: it holds whole
                // profiles. Saving them again could undo later reviews, but
                // the graded record, written last, tells whether it was.
                for (id, profile) in &journal.profiles {
                    self.by_id(id)?.save_profile(profile).await?;
                }
            } else {
                recorded = self.record_reviews(journal).await?;
            }
            if let Some(asked) = &journal.asked {
                store.save_asked_if_current(asked, None).await?;
            }
            store.delete_session(key).await?;
            store.save_graded(&journal.graded).await?;
        }
        store.end_grading(key).await?;
        Ok(recorded)
    }

    /// Record the reviews of `journal` on the profiles they belong to. Each
    /// profile is loaded fresh and saved only if nobody saved it since: when
    /// another session of the learner got there first, the reviews are
    /// recorded on top of theirs (ADR-0002 D6). `None` if every profile
    /// had the reviews already.
    async fn record_reviews(&self, journal: &GradingJournal) -> Result<Option<Recorded>> {
        /// How often one profile may change under us before giving up.
        const ATTEMPTS: usize = 100;
        let mark = grading_mark(journal.key());
        let key = journal.key();
        let mut recorded = Recorded::default();
        let mut any = false;
        for (id, learner) in journal.review_targets() {
            let store = self.by_id(&id)?;
            let indices = journal.review_indices(&id);
            let reviews: Vec<GradedAnswer> = indices
                .iter()
                .map(|&i| journal.reviews[i].review.clone())
                .collect();
            let mut done = false;
            for _ in 0..ATTEMPTS {
                let profile = load_or_new(store, &learner).await?;
                let Some((next, events)) = profile.with_session_reviews(&mark, &reviews) else {
                    done = true;
                    break;
                };
                if store.save_profile_if_current(&next).await? {
                    recorded.add(&indices, events);
                    recorded.levels.push((id.clone(), next.level()));
                    any = true;
                    done = true;
                    break;
                }
            }
            if !done {
                return Err(Error::Port(format!(
                    "the profile of {:?} kept changing while grading {:?}",
                    learner.0, key.0
                )));
            }
        }
        Ok(any.then_some(recorded))
    }
}
