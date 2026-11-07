//! Domain types.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::schedule::ReviewCard;

/// Stable identity of an author, e.g. `github:12345` or `email:jan@example.com` (D5).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AuthorId(pub String);

/// Identifies a concept, e.g. `rust.ownership.borrowing`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ConceptId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct QuestionId(pub String);

/// Identifies a session. In CI it is derived from the PR and its head commit
/// (D8); locally from the hook invocation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SessionKey(pub String);

/// A unified diff of the change under review.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Diff(pub String);

impl Diff {
    pub fn is_empty(&self) -> bool {
        self.0.trim().is_empty()
    }
}

/// A concept that a change exercises.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Concept {
    pub id: ConceptId,
    pub title: String,
    pub summary: String,
}

/// Knowledge level of an author (D7).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub enum Level {
    #[default]
    Novice,
    Intermediate,
    Expert,
}

/// The shape of a question (D7).
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
        /// What a good answer must cover; used by the grader.
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Question {
    pub id: QuestionId,
    pub concept: ConceptId,
    pub body: QuestionBody,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnswerBody {
    Choice(usize),
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    pub question: QuestionId,
    pub body: AnswerBody,
}

/// Grade of a single answer. It maps one-to-one onto FSRS ratings (D6).
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
    pub fn is_correct(self) -> bool {
        !matches!(self, Grade::Again)
    }
}

/// One graded answer in an author's history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GradedAnswer {
    pub concept: ConceptId,
    pub format: QuestionFormat,
    pub grade: Grade,
    pub at: DateTime<Utc>,
}

/// Everything Soqrates knows about one author (D5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LearnerProfile {
    pub author: AuthorId,
    pub level: Level,
    /// One FSRS card per concept the author has been quizzed on (D6).
    pub cards: BTreeMap<ConceptId, ReviewCard>,
    pub history: Vec<GradedAnswer>,
}

impl LearnerProfile {
    /// The profile of an author Soqrates has never seen.
    pub fn new(author: AuthorId) -> Self {
        Self {
            author,
            level: Level::Novice,
            cards: BTreeMap::new(),
            history: Vec::new(),
        }
    }
}

/// A set of questions put to one author about one change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub key: SessionKey,
    pub author: AuthorId,
    pub questions: Vec<Question>,
    pub created_at: DateTime<Utc>,
}

/// The result of grading a session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionOutcome {
    pub grades: Vec<(QuestionId, Grade)>,
    pub passed: bool,
    pub level_after: Level,
}

/// A reference to a pull/merge request on some forge.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PrRef {
    /// e.g. `jvanbuel/soqrates`
    pub repo: String,
    pub number: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequest {
    pub pr: PrRef,
    pub author: AuthorId,
    pub head_sha: String,
    pub diff: Diff,
}

/// Status of the Soqrates check on a PR (D8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CheckStatus {
    Pending,
    Success,
    Failure,
    /// Quiz failed in `Advisory` mode.
    Neutral,
}

/// Which local hook invoked Soqrates (D9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HookKind {
    /// git `pre-commit`
    PreCommit,
    /// jj `pre-push`
    PrePush,
}

/// What a local hook should do (D9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HookDecision {
    Allow,
    Block,
}
