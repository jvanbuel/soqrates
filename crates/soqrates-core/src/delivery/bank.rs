//! The question bank: questions Soqrates wrote, kept so that later rounds
//! can ask them again instead of writing new ones (ADR-0009).

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::assessment::{BankRef, Question, QuestionBrief, QuestionFormat, QuestionKind};
use crate::delivery::FeedbackId;
use crate::ids::{BankId, ConceptId, LearnerId};
use crate::learner::Level;

/// A question kept in a question bank (ADR-0009 D1). Keyed by its concept;
/// identified by a stable [`BankId`] and versioned (D6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BankedQuestion {
    id: BankId,
    version: u32,
    /// The level of the learner it was first written for.
    level: Level,
    question: Question,
    added_at: DateTime<Utc>,
    /// How often it was put in a session, including the first time.
    asked: u32,
    /// A retired question is kept but never offered again (D6).
    retired: bool,
    /// Open disputes about it, by feedback id, no text (ADR-0011 D6): while
    /// there is one, it is not offered.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    disputes: BTreeSet<FeedbackId>,
    /// `good` feedback about it, by feedback id (ADR-0011 D6).
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    praise: BTreeSet<FeedbackId>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    settled: BTreeSet<FeedbackId>,
}

impl BankedQuestion {
    /// Whether `question` may be banked (ADR-0009 D3): it passed the
    /// consistency check, and it may leave the repository it was asked in
    /// (ADR-0007 D6), so it is about a `General` concept and shows no code
    /// from the change.
    pub fn bankable(question: &Question) -> bool {
        question.checked && question.may_leave_repository()
    }

    /// `question`, written for a learner at `level`, as a new bank entry,
    /// first asked at `now`; `None` if it may not be banked.
    pub fn new(question: &Question, level: Level, now: DateTime<Utc>) -> Option<Self> {
        if !Self::bankable(question) {
            return None;
        }
        let mut question = question.clone();
        question.banked = None;
        Some(Self {
            id: Self::id_of(&question),
            version: 1,
            level,
            question,
            added_at: now,
            asked: 1,
            retired: false,
            disputes: BTreeSet::new(),
            praise: BTreeSet::new(),
            settled: BTreeSet::new(),
        })
    }

    /// The id a question gets when first banked: a hash of what it asks,
    /// so writing the same question twice banks it once (D6).
    pub fn id_of(question: &Question) -> BankId {
        let content = serde_json::json!([
            question.concept,
            question.kind,
            question.body,
            question.explanation
        ]);
        BankId(format!("bank-{}", fnv(&content.to_string())))
    }

    pub fn id(&self) -> &BankId {
        &self.id
    }
    pub fn version(&self) -> u32 {
        self.version
    }
    pub fn concept(&self) -> &ConceptId {
        &self.question.concept
    }
    pub fn format(&self) -> QuestionFormat {
        self.question.format()
    }
    pub fn kind(&self) -> QuestionKind {
        self.question.kind
    }
    pub fn level(&self) -> Level {
        self.level
    }
    pub fn added_at(&self) -> DateTime<Utc> {
        self.added_at
    }
    pub fn asked(&self) -> u32 {
        self.asked
    }
    pub fn is_retired(&self) -> bool {
        self.retired
    }

    /// Whether an open dispute keeps it from being offered (ADR-0011 D6).
    pub fn is_disputed(&self) -> bool {
        !self.disputes.is_empty()
    }

    /// How much `good` feedback it has (ADR-0011 D6).
    pub fn praise(&self) -> usize {
        self.praise.len()
    }

    /// Record the dispute `id` (ADR-0011 D6).
    pub fn dispute(&mut self, id: &FeedbackId) {
        if !self.settled.contains(id) {
            self.disputes.insert(id.clone());
        }
    }

    /// Record the `good` feedback `id` (ADR-0011 D6).
    pub fn commend(&mut self, id: &FeedbackId) {
        if !self.settled.contains(id) {
            self.praise.insert(id.clone());
        }
    }

    /// Forget the feedback `id`: its dispute was resolved, or its learner
    /// deleted (ADR-0011 D7, D8). Returns whether it was recorded here.
    pub fn settle(&mut self, id: &FeedbackId) -> bool {
        self.settled.insert(id.clone());
        let disputed = self.disputes.remove(id);
        self.praise.remove(id) || disputed
    }

    /// Never offer this question again (D6).
    pub fn retire(&mut self) {
        self.retired = true;
    }

    /// Replace the question with a revision of it, e.g. after author
    /// feedback (issue #22): same id, next version (D6). A revision that
    /// may not be banked retires the entry instead.
    pub fn revise(&mut self, question: &Question) {
        if !Self::bankable(question) {
            self.retire();
            return;
        }
        let mut question = question.clone();
        question.banked = None;
        self.question = question;
        self.version += 1;
    }

    /// Where a session's question came from (D6).
    pub fn reference(&self) -> BankRef {
        BankRef {
            id: self.id.clone(),
            version: self.version,
        }
    }

    /// Whether this question is what `brief` asks for (ADR-0009 D4): not
    /// retired or disputed (ADR-0011 D6), and of the brief's format, of a
    /// kind it offers, with code as it allows (ADR-0007 D2).
    pub fn suits(&self, brief: &QuestionBrief) -> bool {
        !self.retired && !self.is_disputed() && brief.check(&self.question).is_ok()
    }

    /// The question, to put in a session: counted as asked once more, and
    /// pointing back to this entry (D6).
    pub fn take(&mut self) -> Question {
        self.asked += 1;
        let mut question = self.question.clone();
        question.banked = Some(self.reference());
        question
    }

    /// The question as banked, e.g. for one that was just written.
    pub fn question(&self) -> Question {
        let mut question = self.question.clone();
        question.banked = Some(self.reference());
        question
    }
}

/// The bank questions one learner was shown, and in which round (ADR-0009
/// D5). Kept with the bank, never in the learner store: it holds bank ids
/// and round numbers only, no question text (ADR-0003 D5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeenQuestions {
    learner: LearnerId,
    /// How many rounds the learner had with this bank.
    rounds: u64,
    /// Bank id -> the last round it was shown in.
    last_seen: BTreeMap<BankId, u64>,
}

impl SeenQuestions {
    pub fn new(learner: LearnerId) -> Self {
        Self {
            learner,
            rounds: 0,
            last_seen: BTreeMap::new(),
        }
    }

    pub fn learner(&self) -> &LearnerId {
        &self.learner
    }

    /// The number of the round being composed: one more than the last.
    pub fn next_round(&self) -> u64 {
        self.rounds + 1
    }

    /// The round in which `id` was last shown, if ever.
    pub fn last_seen(&self, id: &BankId) -> Option<u64> {
        self.last_seen.get(id).copied()
    }

    /// Record that `id` was shown in round `round`.
    pub fn show(&mut self, id: &BankId, round: u64) {
        self.last_seen.insert(id.clone(), round);
        self.rounds = self.rounds.max(round);
    }

    /// Record that round `round` took place, whatever it showed.
    pub fn end_round(&mut self, round: u64) {
        self.rounds = self.rounds.max(round);
    }

    /// Whether `id` may be shown in round `round` without showing it twice
    /// within `gap` rounds (D5): never seen, or last seen `gap` or more
    /// rounds before.
    pub fn may_show(&self, id: &BankId, round: u64, gap: u64) -> bool {
        self.last_seen(id)
            .is_none_or(|seen| round.saturating_sub(seen) >= gap)
    }
}

/// Which of `bank`'s questions to ask for `brief` in round `round` (ADR-0009
/// D4), if any: a question that suits the brief and that the learner may be
/// shown (D5). Unseen questions first, then the least recently seen; among
/// those, the brief's most preferred kind, then the one with the most `good`
/// feedback (ADR-0011 D6), then the oldest entry. `except`
/// are questions already in this round.
pub fn pick(
    bank: &[BankedQuestion],
    brief: &QuestionBrief,
    seen: &SeenQuestions,
    round: u64,
    gap: u64,
    except: &[BankId],
) -> Option<usize> {
    bank.iter()
        .enumerate()
        .filter(|(_, q)| q.suits(brief) && !except.contains(q.id()))
        .filter(|(_, q)| seen.may_show(q.id(), round, gap))
        .min_by_key(|(_, q)| {
            let kind_rank = brief.kinds.iter().position(|k| *k == q.kind());
            let praise = std::cmp::Reverse(q.praise());
            (
                seen.last_seen(q.id()),
                kind_rank,
                praise,
                q.added_at,
                q.id.clone(),
            )
        })
        .map(|(i, _)| i)
}

/// 64-bit FNV-1a of `text`, in hex.
pub(super) fn fnv(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::assessment::{CodeSnippet, ConceptScope, QuestionBody, SnippetOrigin};
    use crate::ids::QuestionId;

    fn question(concept: &str, prompt: &str) -> Question {
        Question {
            id: QuestionId(format!("{concept}/mc")),
            concept: ConceptId(concept.into()),
            scope: ConceptScope::General,
            kind: QuestionKind::Scenario,
            body: QuestionBody::MultipleChoice {
                prompt: prompt.into(),
                code: None,
                options: vec!["a".into(), "b".into()],
                options_language: None,
                correct: 0,
                reasons: Vec::new(),
            },
            explanation: "because".into(),
            checked: true,
            banked: None,
        }
    }

    fn at(day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, day, 0, 0, 0).unwrap()
    }

    fn brief() -> QuestionBrief {
        QuestionBrief::new(
            QuestionFormat::MultipleChoice,
            &[QuestionKind::Scenario],
            Vec::new(),
        )
    }

    // @adr-0009-d3
    #[test]
    fn only_checked_questions_that_may_leave_the_repository_are_banked() {
        let q = question("rust.ownership", "Q?");
        assert!(BankedQuestion::bankable(&q));
        let unchecked = Question {
            checked: false,
            ..q.clone()
        };
        assert!(BankedQuestion::new(&unchecked, Level::Novice, at(1)).is_none());
        let project = Question {
            scope: ConceptScope::Project,
            ..q.clone()
        };
        assert!(!BankedQuestion::bankable(&project), "project concepts stay");
        let mut quoting = q.clone();
        quoting.kind = QuestionKind::ExplainYourChange;
        if let QuestionBody::MultipleChoice { code, .. } = &mut quoting.body {
            *code = Some(CodeSnippet {
                language: "rust".into(),
                text: "let x = 1;".into(),
                origin: SnippetOrigin::Change,
            });
        }
        assert!(
            !BankedQuestion::bankable(&quoting),
            "the author's code stays"
        );
    }

    // @adr-0009-d6
    #[test]
    fn a_banked_question_keeps_its_id_across_versions() {
        let q = question("rust.ownership", "Q?");
        let mut banked = BankedQuestion::new(&q, Level::Novice, at(1)).unwrap();
        assert_eq!(banked.version(), 1);
        let id = banked.id().clone();
        assert_eq!(
            id,
            BankedQuestion::id_of(&q),
            "the same question, the same id"
        );
        assert_ne!(
            id,
            BankedQuestion::id_of(&question("rust.ownership", "Other?"))
        );
        banked.revise(&question("rust.ownership", "Better?"));
        assert_eq!(banked.id(), &id);
        assert_eq!(banked.version(), 2);
        let taken = banked.take();
        assert_eq!(
            taken.banked,
            Some(BankRef {
                id: id.clone(),
                version: 2
            })
        );
        assert_eq!(banked.asked(), 2);
        banked.retire();
        assert!(
            !banked.suits(&brief()),
            "a retired question is never offered"
        );
    }

    // @adr-0009-d4 @adr-0009-d5
    #[test]
    fn unseen_questions_come_first_then_the_least_recently_seen() {
        let a = BankedQuestion::new(&question("c", "A?"), Level::Novice, at(1)).unwrap();
        let b = BankedQuestion::new(&question("c", "B?"), Level::Novice, at(2)).unwrap();
        let bank = vec![a.clone(), b.clone()];
        let mut seen = SeenQuestions::new(LearnerId("ada".into()));
        assert_eq!(pick(&bank, &brief(), &seen, 1, 3, &[]), Some(0), "oldest");
        seen.show(a.id(), 1);
        assert_eq!(pick(&bank, &brief(), &seen, 2, 3, &[]), Some(1), "unseen");
        seen.show(b.id(), 2);
        assert_eq!(
            pick(&bank, &brief(), &seen, 3, 3, &[]),
            None,
            "both too recent"
        );
        assert_eq!(pick(&bank, &brief(), &seen, 4, 3, &[]), Some(0));
        assert_eq!(pick(&bank, &brief(), &seen, 4, 3, &[a.id().clone()]), None);
        let open = QuestionBrief::new(QuestionFormat::Open, &[QuestionKind::Scenario], Vec::new());
        assert_eq!(pick(&bank, &open, &seen, 9, 3, &[]), None, "wrong format");
    }

    // @adr-0011-d6
    #[test]
    fn disputed_questions_are_not_offered_and_praised_ones_come_first() {
        let mut a = BankedQuestion::new(&question("c", "A?"), Level::Novice, at(1)).unwrap();
        let mut b = BankedQuestion::new(&question("c", "B?"), Level::Novice, at(2)).unwrap();
        let seen = SeenQuestions::new(LearnerId("ada".into()));
        b.commend(&FeedbackId("fb-good".into()));
        assert_eq!(
            pick(&[a.clone(), b.clone()], &brief(), &seen, 1, 3, &[]),
            Some(1),
            "more good feedback beats being older"
        );
        b.dispute(&FeedbackId("fb-1".into()));
        assert!(!b.suits(&brief()));
        assert_eq!(
            pick(&[a.clone(), b.clone()], &brief(), &seen, 1, 3, &[]),
            Some(0)
        );
        a.dispute(&FeedbackId("fb-2".into()));
        assert_eq!(
            pick(&[a.clone(), b.clone()], &brief(), &seen, 1, 3, &[]),
            None
        );
        assert!(b.settle(&FeedbackId("fb-1".into())));
        assert!(!b.settle(&FeedbackId("fb-1".into())));
        assert!(b.suits(&brief()));
        // Stored before feedback existed, it still loads.
        let mut json = serde_json::to_value(&a).unwrap();
        json.as_object_mut().unwrap().remove("disputes");
        let loaded: BankedQuestion = serde_json::from_value(json).unwrap();
        assert!(!loaded.is_disputed());
    }
}
