//! Delivery context: putting questions to an author and reporting the
//! result as a check or a hook decision (ADR-0002). Uses the shared kernel,
//! `assessment` and `learner`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::assessment::{Answer, Diff, Grade, Question};
use crate::{Error, Result};
use std::collections::{BTreeMap, BTreeSet};

use crate::ids::{AuthorId, Identity, QuestionId, SessionKey};

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

/// The record that a session was graded, kept so that grading happens at
/// most once per session key (ADR-0002 D6) and a re-run of the CI check on
/// the same head reports the result instead of asking again (ADR-0001 D8).
/// Holds no question or answer text (ADR-0003 D5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GradedSession {
    pub key: SessionKey,
    pub author: AuthorId,
    pub correct: usize,
    pub total: usize,
    pub passed: bool,
    pub at: DateTime<Utc>,
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
        if questions.len() > max_questions {
            return Err(Error::Invalid(format!(
                "a session holds at most {max_questions} questions, got {}",
                questions.len()
            )));
        }
        let mut concepts = BTreeSet::new();
        for q in &questions {
            if !concepts.insert(&q.concept) {
                return Err(Error::Invalid(format!(
                    "two questions about concept {:?}",
                    q.concept.0
                )));
            }
        }
        Ok(Self {
            key,
            author,
            questions,
            created_at: at,
        })
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
    pub fn validate_answers(&self, answers: &[Answer]) -> Result<()> {
        let mut seen = BTreeSet::new();
        for a in answers {
            if !self.questions.iter().any(|q| q.id == a.question) {
                return Err(Error::Invalid(format!(
                    "answer to unknown question {:?}",
                    a.question.0
                )));
            }
            if !seen.insert(&a.question) {
                return Err(Error::Invalid(format!(
                    "two answers to question {:?}",
                    a.question.0
                )));
            }
        }
        Ok(())
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
        let grades: Vec<(QuestionId, Grade)> = self
            .questions
            .iter()
            .map(|q| {
                let grade = grades
                    .iter()
                    .find(|(id, _)| id == &q.id)
                    .map_or(Grade::Again, |(_, g)| *g);
                (q.id.clone(), grade)
            })
            .collect();
        let total = grades.len();
        let correct = grades.iter().filter(|(_, g)| g.is_correct()).count();
        let passed = total == 0 || correct as f64 >= pass_threshold * total as f64;
        SessionResult {
            grades,
            passed,
            event: SessionEvent::SessionGraded {
                key: self.key,
                author: self.author,
                correct,
                total,
                passed,
                at,
            },
        }
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

/// The identity a local hook acts as (ADR-0003 D7): the identity configured
/// for the repository's remote host if there is one, otherwise the commit
/// e-mail as an unverified identity.
pub fn local_identity(
    remote_host: Option<&str>,
    commit_email: &str,
    configured: &BTreeMap<String, Identity>,
) -> Identity {
    remote_host
        .and_then(|host| configured.get(host))
        .cloned()
        .unwrap_or_else(|| Identity::Email(commit_email.to_string()))
}
