//! Adaptive question policy (ADR-0001 D7, ADR-0002 D4 and D5).

use crate::assessment::QuestionFormat;
use crate::learner::{GradedAnswer, Level};

/// Consecutive correct reviews since the last level change that promote an
/// author one level (ADR-0002 D4).
pub const PROMOTE_AFTER: usize = 6;

/// Consecutive incorrect reviews since the last level change that demote
/// an author one level (ADR-0002 D4).
pub const DEMOTE_AFTER: usize = 3;

/// The format of the `index`-th (0-based) question of a session for an
/// author at `level` (ADR-0002 D5): Novice always multiple choice; Expert
/// always open; Intermediate multiple choice on even and open on odd indices.
pub fn format_for(level: Level, index: usize) -> QuestionFormat {
    match level {
        Level::Novice => QuestionFormat::MultipleChoice,
        Level::Intermediate if index.is_multiple_of(2) => QuestionFormat::MultipleChoice,
        Level::Intermediate => QuestionFormat::Open,
        Level::Expert => QuestionFormat::Open,
    }
}

/// How `open` slots of a session are split between `reviews` due reviews
/// and `new` concepts from the change (ADR-0002 D10): new concepts are
/// guaranteed half the slots, rounded up (or all of them if fewer), due
/// reviews take what that leaves, and new concepts take any slots the
/// reviews don't use. Returns `(reviews, new)` to ask.
pub fn split_slots(open: usize, reviews: usize, new: usize) -> (usize, usize) {
    let reserved = new.min(open.div_ceil(2));
    let reviews = reviews.min(open - reserved);
    let new = new.min(open - reviews);
    (reviews, new)
}

/// The level after `recent` (the reviews since the last level change,
/// oldest first), starting from `current` (ADR-0002 D4). Promotes after
/// [`PROMOTE_AFTER`] trailing correct reviews, demotes after
/// [`DEMOTE_AFTER`] trailing incorrect ones, and stays within
/// Novice..=Expert.
pub fn next_level(current: Level, recent: &[GradedAnswer]) -> Level {
    let trailing = |correct: bool| {
        recent
            .iter()
            .rev()
            .take_while(|r| r.grade.is_correct() == correct)
            .count()
    };
    if trailing(true) >= PROMOTE_AFTER {
        match current {
            Level::Novice => Level::Intermediate,
            Level::Intermediate | Level::Expert => Level::Expert,
        }
    } else if trailing(false) >= DEMOTE_AFTER {
        match current {
            Level::Expert => Level::Intermediate,
            Level::Intermediate | Level::Novice => Level::Novice,
        }
    } else {
        current
    }
}

#[cfg(test)]
mod tests {
    use super::split_slots;

    // @adr-0002-d10
    #[test]
    fn new_concepts_keep_half_the_slots_and_reviews_fill_the_rest() {
        // (open, reviews, new) -> (reviews, new)
        for (open, reviews, new, want) in [
            (5, 0, 0, (0, 0)),
            (5, 0, 7, (0, 5)),
            (5, 7, 0, (5, 0)),
            // Half of the slots, rounded up, are kept for the change.
            (5, 7, 7, (2, 3)),
            (2, 2, 2, (1, 1)),
            (3, 1, 3, (1, 2)),
            // Reviews use what the change doesn't need, and vice versa.
            (5, 7, 1, (4, 1)),
            (5, 1, 7, (1, 4)),
            (1, 3, 3, (0, 1)),
            (0, 3, 3, (0, 0)),
        ] {
            assert_eq!(
                split_slots(open, reviews, new),
                want,
                "{open} {reviews} {new}"
            );
        }
    }
}
