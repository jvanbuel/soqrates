//! The grading journal: grading a session written ahead of its effects, so
//! that a failure or a crash part-way is finished later rather than left
//! half done (ADR-0002 D6).

use serde::{Deserialize, Serialize};

use crate::delivery::{AskedSession, GradedSession, Session};
use crate::ids::{LearnerId, SessionKey};
use crate::learner::{GradedAnswer, LearnerProfile};

/// Which store a journaled review belongs in (ADR-0003 D4, D5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StoreId {
    /// The shared learner store.
    LearnerStore,
    /// The store of this organisation.
    Organisation(String),
}

/// One graded answer that grading a session adds to a learner's profile
/// (ADR-0002 D6): the data [`LearnerProfile::record`] takes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JournaledReview {
    /// The store whose profile of `learner` gets the review.
    pub store: StoreId,
    pub learner: LearnerId,
    pub review: GradedAnswer,
}

/// What grading one session does: the session it claims, the record that it
/// was graded and the reviews to add to the learner's profiles (ADR-0002
/// D6). Written before any of them; while it is stored, the session is being
/// graded. It holds what happened, not what the profiles look like
/// afterwards, so applying it cannot undo reviews added meanwhile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GradingJournal {
    pub session: Session,
    pub graded: GradedSession,
    /// The reviews, in question order.
    #[serde(default)]
    pub reviews: Vec<JournaledReview>,
    /// Journals written before the reviews were journaled hold each whole
    /// profile to save instead. Such a journal is applied the old way;
    /// new journals leave this empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profiles: Vec<(StoreId, LearnerProfile)>,
    /// The session's questions with the answers, kept for feedback in the
    /// store the journal is in (ADR-0011 D1). `None` where nothing may be
    /// kept, and in journals written before feedback existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked: Option<AskedSession>,
}

impl GradingJournal {
    /// The key of the session being graded.
    pub fn key(&self) -> &SessionKey {
        self.session.key()
    }

    /// The profiles the reviews go on: a store and a learner each, in the
    /// order they first occur.
    pub fn review_targets(&self) -> Vec<(StoreId, LearnerId)> {
        let mut targets: Vec<(StoreId, LearnerId)> = Vec::new();
        for r in &self.reviews {
            let target = (r.store.clone(), r.learner.clone());
            if !targets.contains(&target) {
                targets.push(target);
            }
        }
        targets
    }

    /// The positions in `reviews` of those that go into `store`.
    pub fn review_indices(&self, store: &StoreId) -> Vec<usize> {
        (0..self.reviews.len())
            .filter(|&i| self.reviews[i].store == *store)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::AuthorId;

    // @adr-0002-d6
    #[test]
    fn a_journal_written_before_reviews_were_journaled_still_loads() {
        let session = Session::open(
            SessionKey("o/r#1@abc".into()),
            AuthorId("github:1".into()),
            Vec::new(),
            3,
            chrono::Utc::now(),
        )
        .unwrap();
        let old = GradingJournal {
            graded: GradedSession {
                key: session.key().clone(),
                author: session.author().clone(),
                correct: 0,
                total: 0,
                passed: true,
                at: session.created_at(),
            },
            session,
            reviews: Vec::new(),
            profiles: vec![(
                StoreId::LearnerStore,
                LearnerProfile::new(LearnerId("ada".into())),
            )],
            asked: None,
        };
        let mut doc = serde_json::to_value(&old).unwrap();
        doc.as_object_mut().unwrap().remove("reviews");
        let loaded: GradingJournal = serde_json::from_value(doc).unwrap();
        assert_eq!(loaded, old);
        // A new journal doesn't write the old field.
        let new = GradingJournal {
            profiles: Vec::new(),
            ..old
        };
        let doc = serde_json::to_value(&new).unwrap();
        assert!(doc.get("profiles").is_none());
    }
}
