//! Adaptive question policy (ADR-0001 D7, ADR-0002 D4 and D5, ADR-0007
//! D2).

use crate::assessment::{QuestionFormat, QuestionKind};
use crate::learner::{GradedAnswer, Level};

/// Consecutive correct reviews since the last level change that promote an
/// author one level (ADR-0002 D4).
pub const PROMOTE_AFTER: usize = 6;

/// Consecutive incorrect reviews since the last level change that demote
/// an author one level (ADR-0002 D4).
pub const DEMOTE_AFTER: usize = 3;

/// The format of every question for an author at `level` (ADR-0002 D5):
/// multiple choice, unless open questions are enabled and the author is an
/// Expert. A session never alternates formats.
pub fn format_for(level: Level, open_questions: bool) -> QuestionFormat {
    match level {
        Level::Expert if open_questions => QuestionFormat::Open,
        _ => QuestionFormat::MultipleChoice,
    }
}

/// The kinds a question in `format` for an author at `level` may have,
/// most preferred first (ADR-0007 D2). Kinds in `asked` (those of the
/// session's earlier questions) move to the back, keeping their order, so
/// that a round mixes kinds. The examiner picks the first that suits the
/// concept.
pub fn kinds_for(
    level: Level,
    format: QuestionFormat,
    asked: &[QuestionKind],
) -> Vec<QuestionKind> {
    use QuestionKind::*;
    // One preference per level, whatever the format (ADR-0002 D5 no longer
    // mixes formats); kinds that don't fit `format` are dropped below.
    let preferred: &[QuestionKind] = match level {
        Level::Novice => &[PredictOutcome, SpotTheBug, Scenario, ChooseTheFix],
        Level::Intermediate => &[SpotTheBug, ChooseTheFix, PredictOutcome, Scenario],
        Level::Expert => &[ExplainYourChange, SpotTheBug, Scenario, PredictOutcome],
    };
    let fitting = preferred.iter().copied().filter(|k| k.fits(format));
    let (fresh, repeated): (Vec<_>, Vec<_>) = fitting.partition(|k| !asked.contains(k));
    fresh.into_iter().chain(repeated).collect()
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
    use super::{kinds_for, split_slots};
    use crate::assessment::{QuestionFormat, QuestionKind};
    use crate::learner::Level;

    // @adr-0007-d2
    #[test]
    fn the_level_steers_the_kind_and_rounds_mix_kinds() {
        use QuestionFormat::{MultipleChoice, Open};
        use QuestionKind::*;
        assert_eq!(
            kinds_for(Level::Novice, MultipleChoice, &[])[0],
            PredictOutcome
        );
        assert_eq!(kinds_for(Level::Expert, Open, &[])[0], ExplainYourChange);
        assert_eq!(
            kinds_for(Level::Intermediate, MultipleChoice, &[])[0],
            SpotTheBug
        );
        // An expert without open questions keeps the same preference.
        assert_eq!(
            kinds_for(Level::Expert, MultipleChoice, &[])[0],
            ExplainYourChange
        );
        // Choose the fix is never offered for an open question.
        for level in [Level::Novice, Level::Intermediate, Level::Expert] {
            assert!(!kinds_for(level, Open, &[]).contains(&ChooseTheFix));
            assert!(kinds_for(level, MultipleChoice, &[]).contains(&Scenario));
        }
        // A kind already asked moves to the back.
        assert_eq!(
            kinds_for(Level::Novice, MultipleChoice, &[PredictOutcome]),
            [SpotTheBug, Scenario, ChooseTheFix, PredictOutcome]
        );
        assert_eq!(
            kinds_for(Level::Expert, Open, &[ExplainYourChange, SpotTheBug]),
            [Scenario, PredictOutcome, ExplainYourChange, SpotTheBug]
        );
    }

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
