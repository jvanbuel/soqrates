//! Domain events (ADR-0002 D7).

use serde::{Deserialize, Serialize};

use crate::delivery::SessionEvent;
use crate::learner::LearnerEvent;

/// Everything that happened during a session, in order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DomainEvent {
    Learner(LearnerEvent),
    Session(SessionEvent),
}

impl From<LearnerEvent> for DomainEvent {
    fn from(e: LearnerEvent) -> Self {
        DomainEvent::Learner(e)
    }
}

impl From<SessionEvent> for DomainEvent {
    fn from(e: SessionEvent) -> Self {
        DomainEvent::Session(e)
    }
}
