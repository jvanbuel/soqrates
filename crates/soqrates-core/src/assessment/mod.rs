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

/// One idea that a change exercises. The unit of learning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Concept {
    pub id: ConceptId,
    pub title: String,
    pub summary: String,
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
    pub body: QuestionBody,
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

impl Grade {
    /// Only `Again` is incorrect.
    pub fn is_correct(self) -> bool {
        !matches!(self, Grade::Again)
    }
}
