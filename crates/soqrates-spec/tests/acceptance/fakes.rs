//! Hermetic fakes for the ports of `soqrates-core` (ADR-0001 D12).
//!
//! Nothing here talks to a real model, forge or terminal. Everything is
//! deterministic so scenarios can assert exact outcomes.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, TimeZone, Utc};
use soqrates_core::ports::{Clock, Examiner, Prompter, ScmProvider, StateStore};
use soqrates_core::{
    Answer, AnswerBody, AuthorId, CheckStatus, Concept, ConceptId, Diff, Error, Grade,
    LearnerProfile, PrRef, PullRequest, Question, QuestionBody, QuestionFormat, QuestionId, Result,
    Session, SessionKey, SessionOutcome,
};
use soqrates_store::{FileStore, InMemoryStore};

// ---------------------------------------------------------------------------
// Diffs
// ---------------------------------------------------------------------------

/// Marker the [`FakeExaminer`] looks for in added lines of a diff.
const CONCEPT_MARKER: &str = "+// concept: ";

/// A unified diff whose added lines name the given concepts, in order. The
/// [`FakeExaminer`] extracts exactly these concepts from it.
pub fn diff_touching(concepts: &[String]) -> Diff {
    if concepts.is_empty() {
        return Diff(String::new());
    }
    let mut text =
        String::from("diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n");
    text.push_str(&format!("@@ -0,0 +1,{} @@\n", concepts.len()));
    for c in concepts {
        text.push_str(CONCEPT_MARKER);
        text.push_str(c);
        text.push('\n');
    }
    Diff(text)
}

// ---------------------------------------------------------------------------
// Examiner
// ---------------------------------------------------------------------------

/// Text of an open answer the [`FakeExaminer`] grades as correct.
pub const CORRECT_TEXT: &str = "correct: a thorough explanation";
/// Text of an open answer the [`FakeExaminer`] grades as wrong.
pub const WRONG_TEXT: &str = "I don't know";

/// A scripted examiner.
///
/// - Concepts: one per `+// concept: <id>` line of the diff, in order.
/// - Questions: id `<concept>#mc` or `<concept>#open`; multiple-choice
///   questions have four options and option 0 is correct.
/// - Grading (the answer key): the correct choice, or an open answer starting
///   with `correct`, gets the configurable `correct_grade` (default `Good`);
///   anything else gets `Again`.
#[derive(Debug, Clone)]
pub struct FakeExaminer {
    correct_grade: Arc<Mutex<Grade>>,
}

impl Default for FakeExaminer {
    fn default() -> Self {
        Self {
            correct_grade: Arc::new(Mutex::new(Grade::Good)),
        }
    }
}

impl FakeExaminer {
    pub fn set_correct_grade(&self, grade: Grade) {
        *self.correct_grade.lock().unwrap() = grade;
    }

    pub fn question_id(concept: &ConceptId, format: QuestionFormat) -> QuestionId {
        let suffix = match format {
            QuestionFormat::MultipleChoice => "mc",
            QuestionFormat::Open => "open",
        };
        QuestionId(format!("{}#{suffix}", concept.0))
    }

    /// An answer to `question` that the answer key grades as correct or wrong.
    pub fn answer(question: &Question, correct: bool) -> Answer {
        let body = match &question.body {
            QuestionBody::MultipleChoice {
                options,
                correct: right,
                ..
            } => {
                if correct {
                    AnswerBody::Choice(*right)
                } else {
                    AnswerBody::Choice((*right + 1) % options.len().max(1))
                }
            }
            QuestionBody::Open { .. } => {
                AnswerBody::Text(if correct { CORRECT_TEXT } else { WRONG_TEXT }.to_string())
            }
        };
        Answer {
            question: question.id.clone(),
            body,
        }
    }
}

impl Examiner for FakeExaminer {
    async fn extract_concepts(&self, diff: &Diff) -> Result<Vec<Concept>> {
        Ok(diff
            .0
            .lines()
            .filter_map(|l| l.strip_prefix(CONCEPT_MARKER))
            .map(|id| Concept {
                id: ConceptId(id.trim().to_string()),
                title: id.trim().to_string(),
                summary: format!("What the change does with {}", id.trim()),
            })
            .collect())
    }

    async fn write_question(&self, concept: &Concept, format: QuestionFormat) -> Result<Question> {
        let id = Self::question_id(&concept.id, format);
        let body = match format {
            QuestionFormat::MultipleChoice => QuestionBody::MultipleChoice {
                prompt: format!("Which statement about {} is true?", concept.title),
                options: vec![
                    "the right one".into(),
                    "a plausible wrong one".into(),
                    "another wrong one".into(),
                    "an obviously wrong one".into(),
                ],
                correct: 0,
            },
            QuestionFormat::Open => QuestionBody::Open {
                prompt: format!("Explain {} in this change.", concept.title),
                rubric: format!("Covers {}", concept.title),
            },
        };
        Ok(Question {
            id,
            concept: concept.id.clone(),
            body,
        })
    }

    async fn grade(&self, question: &Question, answer: &Answer) -> Result<Grade> {
        if answer.question != question.id {
            return Err(Error::Invalid(format!(
                "answer for {:?} graded against {:?}",
                answer.question, question.id
            )));
        }
        let correct = match (&question.body, &answer.body) {
            (QuestionBody::MultipleChoice { correct, .. }, AnswerBody::Choice(c)) => c == correct,
            (QuestionBody::Open { .. }, AnswerBody::Text(t)) => t.starts_with("correct"),
            _ => false,
        };
        Ok(if correct {
            *self.correct_grade.lock().unwrap()
        } else {
            Grade::Again
        })
    }
}

// ---------------------------------------------------------------------------
// Clock
// ---------------------------------------------------------------------------

/// A settable clock shared between the world and the engine.
#[derive(Debug, Clone)]
pub struct FakeClock {
    now: Arc<Mutex<DateTime<Utc>>>,
}

impl Default for FakeClock {
    fn default() -> Self {
        Self {
            now: Arc::new(Mutex::new(
                Utc.with_ymd_and_hms(2026, 1, 5, 9, 0, 0).single().unwrap(),
            )),
        }
    }
}

impl FakeClock {
    pub fn advance(&self, by: Duration) {
        *self.now.lock().unwrap() += by;
    }
}

impl Clock for FakeClock {
    fn now(&self) -> DateTime<Utc> {
        *self.now.lock().unwrap()
    }
}

// ---------------------------------------------------------------------------
// SCM provider
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct ScmState {
    pub pull_requests: HashMap<PrRef, PullRequest>,
    /// Every `post_questions` call, in order.
    pub posted: Vec<(PrRef, Session)>,
    /// Every `set_check` call, in order.
    pub checks: Vec<(PrRef, CheckStatus)>,
    /// Replies by the author, keyed by the session they answer.
    pub replies: HashMap<SessionKey, Vec<Answer>>,
}

/// A fake forge. Records what Soqrates posts and lets steps inject the
/// author's reply.
#[derive(Debug, Clone, Default)]
pub struct FakeScm {
    pub state: Arc<Mutex<ScmState>>,
}

impl FakeScm {
    pub fn open(&self, pr: PullRequest) {
        self.state
            .lock()
            .unwrap()
            .pull_requests
            .insert(pr.pr.clone(), pr);
    }

    pub fn pull_request_now(&self, pr: &PrRef) -> PullRequest {
        self.state.lock().unwrap().pull_requests[pr].clone()
    }

    pub fn posted_on(&self, pr: &PrRef) -> Vec<Session> {
        self.state
            .lock()
            .unwrap()
            .posted
            .iter()
            .filter(|(p, _)| p == pr)
            .map(|(_, s)| s.clone())
            .collect()
    }

    pub fn last_check_on(&self, pr: &PrRef) -> Option<CheckStatus> {
        self.state
            .lock()
            .unwrap()
            .checks
            .iter()
            .rev()
            .find(|(p, _)| p == pr)
            .map(|(_, s)| *s)
    }

    pub fn reply(&self, session: &SessionKey, answers: Vec<Answer>) {
        self.state
            .lock()
            .unwrap()
            .replies
            .insert(session.clone(), answers);
    }
}

impl ScmProvider for FakeScm {
    async fn pull_request(&self, pr: &PrRef) -> Result<PullRequest> {
        self.state
            .lock()
            .unwrap()
            .pull_requests
            .get(pr)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("{}#{}", pr.repo, pr.number)))
    }

    async fn post_questions(&self, pr: &PrRef, session: &Session) -> Result<()> {
        self.state
            .lock()
            .unwrap()
            .posted
            .push((pr.clone(), session.clone()));
        Ok(())
    }

    async fn fetch_answers(&self, _pr: &PrRef, session: &Session) -> Result<Option<Vec<Answer>>> {
        Ok(self
            .state
            .lock()
            .unwrap()
            .replies
            .get(session.key())
            .cloned())
    }

    async fn set_check(&self, pr: &PrRef, status: CheckStatus) -> Result<()> {
        self.state.lock().unwrap().checks.push((pr.clone(), status));
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Prompter
// ---------------------------------------------------------------------------

/// Answers every question at the "terminal" correctly or wrongly, and records
/// what was asked and reported.
#[derive(Debug, Default)]
pub struct ScriptedPrompter {
    pub answer_correctly: bool,
    pub asked: Vec<Question>,
    pub reported: Vec<SessionOutcome>,
}

impl Prompter for ScriptedPrompter {
    fn ask(&mut self, question: &Question) -> Result<Answer> {
        self.asked.push(question.clone());
        Ok(FakeExaminer::answer(question, self.answer_correctly))
    }

    fn report(&mut self, outcome: &SessionOutcome) {
        self.reported.push(outcome.clone());
    }
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

/// Either of the real `soqrates-store` adapters, so one world type can use
/// both.
#[derive(Debug)]
pub enum AnyStore {
    Memory(InMemoryStore),
    File(FileStore),
}

impl AnyStore {
    pub fn file(root: &Path) -> Self {
        AnyStore::File(FileStore::new(root))
    }
}

impl StateStore for AnyStore {
    async fn load_profile(&self, author: &AuthorId) -> Result<Option<LearnerProfile>> {
        match self {
            AnyStore::Memory(s) => s.load_profile(author).await,
            AnyStore::File(s) => s.load_profile(author).await,
        }
    }
    async fn save_profile(&self, profile: &LearnerProfile) -> Result<()> {
        match self {
            AnyStore::Memory(s) => s.save_profile(profile).await,
            AnyStore::File(s) => s.save_profile(profile).await,
        }
    }
    async fn load_session(&self, key: &SessionKey) -> Result<Option<Session>> {
        match self {
            AnyStore::Memory(s) => s.load_session(key).await,
            AnyStore::File(s) => s.load_session(key).await,
        }
    }
    async fn save_session(&self, session: &Session) -> Result<()> {
        match self {
            AnyStore::Memory(s) => s.save_session(session).await,
            AnyStore::File(s) => s.save_session(session).await,
        }
    }
    async fn delete_session(&self, key: &SessionKey) -> Result<()> {
        match self {
            AnyStore::Memory(s) => s.delete_session(key).await,
            AnyStore::File(s) => s.delete_session(key).await,
        }
    }
}
