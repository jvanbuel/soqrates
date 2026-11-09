//! The grading journal: grading a session written ahead of its effects, so
//! that a failure or a crash part-way is finished later rather than left
//! half done (ADR-0002 D6).

use serde::{Deserialize, Serialize};

use crate::delivery::{AskedSession, GradedSession, Session};
use crate::ids::SessionKey;
use crate::learner::LearnerProfile;

/// Which store a journaled profile belongs in (ADR-0003 D4, D5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StoreId {
    /// The shared learner store.
    LearnerStore,
    /// The store of this organisation.
    Organisation(String),
}

/// The complete intended end state of grading one session: the session it
/// claims, the record that it was graded and the learner profiles with the
/// new reviews (ADR-0002 D6). Written before any of them; while it is
/// stored, the session is being graded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GradingJournal {
    pub session: Session,
    pub graded: GradedSession,
    /// Each profile to save, whole, and the store it goes to.
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
}
