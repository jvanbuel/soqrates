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
        Self {
            card: rs_fsrs::Card {
                due: now,
                last_review: now,
                ..rs_fsrs::Card::default()
            },
        }
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
        let rating = match grade {
            Grade::Again => rs_fsrs::Rating::Again,
            Grade::Hard => rs_fsrs::Rating::Hard,
            Grade::Good => rs_fsrs::Rating::Good,
            Grade::Easy => rs_fsrs::Rating::Easy,
        };
        self.card = scheduler().next(self.card.clone(), now, rating).card;
    }
}

/// FSRS with its long-term scheduler: no same-day learning steps, because
/// Soqrates reviews happen per change, not in drills (ADR-0001 D6).
fn scheduler() -> rs_fsrs::FSRS {
    rs_fsrs::FSRS::new(rs_fsrs::Parameters {
        enable_short_term: false,
        ..rs_fsrs::Parameters::default()
    })
}
