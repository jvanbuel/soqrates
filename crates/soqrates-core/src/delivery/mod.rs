//! Delivery context: putting questions to an author and reporting the
//! result as a check or a hook decision (ADR-0002). Uses the shared kernel,
//! `assessment` and `learner`.

pub mod answer_command;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::assessment::{Answer, AnswerBody, Diff, Grade, Question};
use crate::{Error, Result};
use std::collections::{BTreeMap, BTreeSet};

use crate::ids::{AuthorId, ConceptId, Identity, QuestionId, SessionKey};

mod bank;
mod journal;
pub use bank::{BankedQuestion, SeenQuestions, pick};
pub use journal::{GradingJournal, StoreId};

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

/// The result of one question in a graded session (ADR-0004 D1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuestionResult {
    pub question: QuestionId,
    pub concept: ConceptId,
    /// The question as asked.
    pub prompt: String,
    /// The options of a multiple-choice question, in the order shown.
    pub options: Vec<String>,
    /// The index of the correct option, for multiple choice.
    pub correct_option: Option<usize>,
    /// What the author answered; `None` if unanswered.
    pub answer: Option<AnswerBody>,
    pub grade: Grade,
    /// Why the answer was right or wrong.
    pub feedback: String,
    /// When this concept is next due for review. Profile data: shown only
    /// on private surfaces (ADR-0004 D2).
    pub next_review: Option<DateTime<Utc>>,
}

impl QuestionResult {
    pub fn is_correct(&self) -> bool {
        self.grade.is_correct()
    }
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

impl PrRef {
    /// The text form of the reference, e.g. `jvanbuel/soqrates#7`: the key
    /// of its [`PrRounds`] in a store.
    pub fn id(&self) -> String {
        format!("{}#{}", self.repo, self.number)
    }
}

/// What Soqrates asked about one head of a pull request (ADR-0002 D9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Round {
    /// The head commit the round is for.
    pub head: String,
    /// The session that answers for this head: the round's own, or, when
    /// the head brought nothing new to ask, the latest earlier round's
    /// session, carried forward.
    pub session: SessionKey,
    /// The concepts this round's session asks, carried ones included
    /// (ADR-0002 D9). Empty when the round's session was carried forward.
    pub concepts: Vec<ConceptId>,
    pub at: DateTime<Utc>,
}

/// The rounds of one pull request, oldest first (ADR-0002 D9). Holds
/// concept IDs and commits only, no question or answer text (ADR-0003 D5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrRounds {
    pr: PrRef,
    rounds: Vec<Round>,
}

impl PrRounds {
    pub fn new(pr: PrRef) -> Self {
        Self {
            pr,
            rounds: Vec::new(),
        }
    }

    pub fn pr(&self) -> &PrRef {
        &self.pr
    }

    pub fn rounds(&self) -> &[Round] {
        &self.rounds
    }

    /// The most recent round.
    pub fn latest(&self) -> Option<&Round> {
        self.rounds.last()
    }

    /// The round for `head`, if there was one.
    pub fn for_head(&self, head: &str) -> Option<&Round> {
        self.rounds.iter().find(|r| r.head == head)
    }

    /// Add `round`. A head has at most one round: recording a head again
    /// replaces its round and makes it the latest.
    pub fn record(&mut self, round: Round) {
        self.forget(&round.head);
        self.rounds.push(round);
    }

    /// Remove the round for `head`, if any.
    pub fn forget(&mut self, head: &str) {
        self.rounds.retain(|r| r.head != head);
    }
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
