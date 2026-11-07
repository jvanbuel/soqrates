//! Delivery context: putting questions to an author and reporting the
//! result as a check or a hook decision (ADR-0002). Uses the shared kernel,
//! `assessment` and `learner`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::Result;
use crate::assessment::{Diff, Grade, Question};
use crate::ids::{AuthorId, QuestionId, SessionKey};

/// Whether a failed session blocks (ADR-0001 D8, D9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Mode {
    /// Report the result, never block.
    #[default]
    Advisory,
    /// A failed session blocks the commit/push or fails the check.
    Blocking,
}

/// Events emitted by [`Session`] (ADR-0002 D7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SessionEvent {
    SessionGraded {
        key: SessionKey,
        author: AuthorId,
        correct: usize,
        total: usize,
        passed: bool,
        at: DateTime<Utc>,
    },
}

/// The questions put to one author about one change. An aggregate
/// (ADR-0002 D3, D6): opened with [`Session::open`], consumed by
/// [`Session::conclude`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    key: SessionKey,
    author: AuthorId,
    questions: Vec<Question>,
    created_at: DateTime<Utc>,
}

/// The result of concluding a session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionResult {
    /// One grade per question, in question order.
    pub grades: Vec<(QuestionId, Grade)>,
    pub passed: bool,
    pub event: SessionEvent,
}

impl Session {
    /// Open a session. Fails with `Invalid` if there are more than
    /// `max_questions` questions or two questions about the same concept
    /// (ADR-0002 D6).
    pub fn open(
        key: SessionKey,
        author: AuthorId,
        questions: Vec<Question>,
        max_questions: usize,
        at: DateTime<Utc>,
    ) -> Result<Self> {
        let _ = (key, author, questions, max_questions, at);
        todo!("ADR-0002 D6")
    }

    pub fn key(&self) -> &SessionKey {
        &self.key
    }

    pub fn author(&self) -> &AuthorId {
        &self.author
    }

    pub fn questions(&self) -> &[Question] {
        &self.questions
    }

    pub fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    /// Check that `answers` refer to questions in this session, at most one
    /// per question. Fails with `Invalid` otherwise (ADR-0002 D6).
    pub fn validate_answers(&self, answers: &[crate::assessment::Answer]) -> Result<()> {
        let _ = answers;
        todo!("ADR-0002 D6")
    }

    /// Grade the session with the examiner's `grades`. Questions missing from
    /// `grades` count as `Again`. Passes if the fraction of correct grades is
    /// at least `pass_threshold`; an empty session passes (ADR-0002 D6).
    /// Consumes the session: it can be graded only once.
    pub fn conclude(
        self,
        grades: Vec<(QuestionId, Grade)>,
        pass_threshold: f64,
        at: DateTime<Utc>,
    ) -> SessionResult {
        let _ = (grades, pass_threshold, at);
        todo!("ADR-0002 D6/D7")
    }
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

/// The status Soqrates reports on a pull request (ADR-0001 D8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CheckStatus {
    Pending,
    Success,
    Failure,
    /// Session failed in `Advisory` mode.
    Neutral,
}

/// Which local hook invoked Soqrates (ADR-0001 D9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HookKind {
    /// git `pre-commit`
    PreCommit,
    /// jj `pre-push`
    PrePush,
}

/// What a local hook should do (ADR-0001 D9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HookDecision {
    Allow,
    Block,
}
