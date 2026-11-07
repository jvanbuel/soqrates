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
