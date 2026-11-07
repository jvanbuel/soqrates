//! Assessment context: what a change is about, and how answers are judged
//! (ADR-0002). Uses only the shared kernel.

use serde::{Deserialize, Serialize};

use crate::ids::{ConceptId, QuestionId};

/// A change: the unified diff Soqrates asks about.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Diff(pub String);

impl Diff {
    pub fn is_empty(&self) -> bool {
        self.0.trim().is_empty()
    }
}

/// Whether a concept is general technology knowledge or specific to one
/// organisation's code (ADR-0003 D5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum ConceptScope {
    /// e.g. `rust.async.cancellation`. May be stored in a learner store
    /// shared across organisations.
    General,
    /// Specific to one organisation's code. Never leaves that organisation,
    /// and never becomes `General`. The safe default.
    #[default]
    Project,
}

/// One idea that a change exercises. The unit of learning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Concept {
    pub id: ConceptId,
    pub title: String,
    pub summary: String,
    pub scope: ConceptScope,
}

/// The shape of a question (ADR-0001 D7, ADR-0002 D5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum QuestionFormat {
    MultipleChoice,
    Open,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuestionBody {
    MultipleChoice {
        prompt: String,
        options: Vec<String>,
        /// Index into `options`.
        correct: usize,
    },
    Open {
        prompt: String,
        /// What a good answer must cover; used when grading.
        rubric: String,
    },
}

impl QuestionBody {
    pub fn format(&self) -> QuestionFormat {
        match self {
            QuestionBody::MultipleChoice { .. } => QuestionFormat::MultipleChoice,
            QuestionBody::Open { .. } => QuestionFormat::Open,
        }
    }
}

/// A question about one concept, in one format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Question {
    pub id: QuestionId,
    pub concept: ConceptId,
    /// The scope of `concept`, which decides where its review is stored
    /// (ADR-0003 D5).
    pub scope: ConceptScope,
    pub body: QuestionBody,
    /// Why the correct answer is correct (multiple choice) or what a good
    /// answer covers (open). Shown only after grading (ADR-0004 D1).
    pub explanation: String,
}

impl Question {
    pub fn format(&self) -> QuestionFormat {
        self.body.format()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnswerBody {
    Choice(usize),
    Text(String),
}

/// The author's response to a question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    pub question: QuestionId,
    pub body: AnswerBody,
}

/// The grade of one answer. Maps one-to-one onto FSRS ratings (ADR-0001 D6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Grade {
    /// Wrong.
    Again,
    /// Correct with significant gaps.
    Hard,
    /// Correct.
    Good,
    /// Correct and thorough.
    Easy,
}

/// The examiner's verdict on one answer: the grade and a short explanation
/// for the author (ADR-0004 D1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grading {
    pub grade: Grade,
    pub feedback: String,
}

impl Grade {
    /// Only `Again` is incorrect.
    pub fn is_correct(self) -> bool {
        !matches!(self, Grade::Again)
    }
}
