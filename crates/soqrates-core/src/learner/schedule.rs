//! Spaced repetition with FSRS (ADR-0001 D6).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::assessment::Grade;

/// A review card: the FSRS schedule state of one concept for one author.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewCard {
    card: rs_fsrs::Card,
}

impl ReviewCard {
    /// A card for a concept that has never been reviewed.
    pub fn new(now: DateTime<Utc>) -> Self {
        let _ = now;
        todo!("ADR-0001 D6")
    }

    /// When this concept should be asked again.
    pub fn due(&self) -> DateTime<Utc> {
        self.card.due
    }

    pub fn is_due(&self, now: DateTime<Utc>) -> bool {
        self.due() <= now
    }

    /// How many times this card has been reviewed.
    pub fn reps(&self) -> u32 {
        self.card.reps as u32
    }

    /// FSRS memory stability in days: grows with successful reviews.
    pub fn stability(&self) -> f64 {
        self.card.stability
    }

    /// Record a review with `grade` at `now` and reschedule.
    pub fn review(&mut self, grade: Grade, now: DateTime<Utc>) {
        let _ = (grade, now);
        todo!("ADR-0001 D6")
    }
}
