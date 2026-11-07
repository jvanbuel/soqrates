//! Adaptive question policy (ADR-0001 D7).

use crate::model::{GradedAnswer, Level, QuestionFormat};

/// The format to use for the `index`-th question of a session for an author
/// at `level`. Novices get multiple choice, experts get open questions, and
/// intermediates get a mix.
pub fn format_for(level: Level, index: usize) -> QuestionFormat {
    let _ = (level, index);
    todo!("ADR-0001 D7")
}

/// The author's level after taking `history` (oldest first) into account,
/// starting from `current`.
pub fn next_level(current: Level, history: &[GradedAnswer]) -> Level {
    let _ = (current, history);
    todo!("ADR-0001 D7")
}
