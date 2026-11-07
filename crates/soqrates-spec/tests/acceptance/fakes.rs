//! Hermetic fakes for the ports of `soqrates-core` (ADR-0001 D12).
//!
//! Nothing here talks to a real model, forge or terminal. Everything is
//! deterministic so scenarios can assert exact outcomes.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, TimeZone, Utc};
use soqrates_core::ports::{Clock, Examiner, LearnerStore, Prompter, ScmProvider, StateStore};
use soqrates_core::{
    Answer, AnswerBody, CheckStatus, Concept, ConceptId, ConceptScope, Diff, Error, Grade,
    GradedSession, Grading, Identity, IdentityLink, LearnerId, LearnerProfile, LinkCode, PrRef,
    PullRequest, Question, QuestionBody, QuestionFormat, QuestionId, Result, Session, SessionKey,
    SessionOutcome,
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
/// - Scope (ADR-0003 D5): `General` unless a scenario classifies the concept
///   otherwise with [`FakeExaminer::classify`].
#[derive(Debug, Clone)]
pub struct FakeExaminer {
    correct_grade: Arc<Mutex<Grade>>,
    scopes: Arc<Mutex<HashMap<ConceptId, ConceptScope>>>,
}

impl Default for FakeExaminer {
    fn default() -> Self {
        Self {
            correct_grade: Arc::new(Mutex::new(Grade::Good)),
            scopes: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl FakeExaminer {
    pub fn set_correct_grade(&self, grade: Grade) {
        *self.correct_grade.lock().unwrap() = grade;
    }

    /// The grade the answer key gives a correct answer.
    pub fn correct_grade(&self) -> Grade {
        *self.correct_grade.lock().unwrap()
    }

    /// Classify `concept` as `scope` from now on (ADR-0003 D5).
    pub fn classify(&self, concept: &ConceptId, scope: ConceptScope) {
        self.scopes.lock().unwrap().insert(concept.clone(), scope);
    }

    /// The scope this examiner gives `concept`: `General` by default.
    pub fn scope_of(&self, concept: &ConceptId) -> ConceptScope {
        self.scopes
            .lock()
            .unwrap()
            .get(concept)
            .copied()
            .unwrap_or(ConceptScope::General)
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
            .map(|id| {
                let cid = ConceptId(id.trim().to_string());
                Concept {
                    scope: self.scope_of(&cid),
                    id: cid,
                    title: id.trim().to_string(),
                    summary: format!("What the change does with {}", id.trim()),
                }
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
            scope: concept.scope,
            body,
            explanation: Self::explanation(&concept.id),
        })
    }

    async fn grade(&self, question: &Question, answer: &Answer) -> Result<Grading> {
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
        let grade = if correct {
            *self.correct_grade.lock().unwrap()
        } else {
            Grade::Again
        };
        // Open answers get the grader's own feedback; multiple choice leaves
        // it empty so the question's explanation is used (ADR-0004 D1).
        let feedback = match &question.body {
            QuestionBody::Open { .. } => Self::open_feedback(&question.concept, correct),
            QuestionBody::MultipleChoice { .. } => String::new(),
        };
        Ok(Grading { grade, feedback })
    }
}

impl FakeExaminer {
    /// The explanation the fake writes for `concept`.
    pub fn explanation(concept: &ConceptId) -> String {
        format!("Because {} works that way.", concept.0)
    }

    /// The feedback the fake gives on an open answer about `concept`.
    pub fn open_feedback(concept: &ConceptId, correct: bool) -> String {
        if correct {
            format!("Covers {} well.", concept.0)
        } else {
            format!("Misses what {} does.", concept.0)
        }
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

    /// Move the clock to `at`, which must not be in its past.
    pub fn set(&self, at: DateTime<Utc>) {
        let mut now = self.now.lock().unwrap();
        assert!(at >= *now, "the clock cannot go back from {now} to {at}");
        *now = at;
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
    /// The summary of every `set_check` call, in the same order as `checks`.
    pub summaries: Vec<(PrRef, String)>,
    /// Replies by the author, keyed by the session they answer.
    pub replies: HashMap<SessionKey, Vec<Answer>>,
    /// Make the next `post_questions` call fail, as on a transient error.
    pub fail_next_post: bool,
    /// Every `post_results` call, in order.
    pub results: Vec<(PrRef, SessionKey, SessionOutcome)>,
    /// Question comments frozen as superseded: the session key, the session
    /// shown (if it was still stored) and the key of the newer session.
    pub superseded: Vec<(PrRef, SessionKey, Option<Session>, SessionKey)>,
    /// Make the next `supersede_questions` call fail.
    pub fail_next_supersede: bool,
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

    /// The summary of the latest check set on `pr`.
    pub fn last_summary_on(&self, pr: &PrRef) -> Option<String> {
        self.state
            .lock()
            .unwrap()
            .summaries
            .iter()
            .rev()
            .find(|(p, _)| p == pr)
            .map(|(_, s)| s.clone())
    }

    /// The question comments frozen as superseded on `pr`, in order.
    pub fn superseded_on(&self, pr: &PrRef) -> Vec<(SessionKey, Option<Session>, SessionKey)> {
        self.state
            .lock()
            .unwrap()
            .superseded
            .iter()
            .filter(|(p, ..)| p == pr)
            .map(|(_, k, s, n)| (k.clone(), s.clone(), n.clone()))
            .collect()
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
        let mut state = self.state.lock().unwrap();
        if std::mem::take(&mut state.fail_next_post) {
            return Err(Error::Port("the forge is unavailable".into()));
        }
        state.posted.push((pr.clone(), session.clone()));
        Ok(())
    }

    async fn post_results(
        &self,
        pr: &PrRef,
        session: &Session,
        outcome: &SessionOutcome,
    ) -> Result<()> {
        self.state.lock().unwrap().results.push((
            pr.clone(),
            session.key().clone(),
            outcome.clone(),
        ));
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

    async fn superseded_questions(&self, pr: &PrRef, current: &Session) -> Result<Vec<SessionKey>> {
        let state = self.state.lock().unwrap();
        let posted = |k: &SessionKey| state.posted.iter().any(|(p, s)| p == pr && s.key() == k);
        if !posted(current.key()) {
            return Ok(Vec::new());
        }
        let frozen = |k: &SessionKey| {
            state.results.iter().any(|(p, r, _)| p == pr && r == k)
                || state
                    .superseded
                    .iter()
                    .any(|(p, r, _, _)| p == pr && r == k)
        };
        let mut keys: Vec<SessionKey> = Vec::new();
        for (_, s) in state.posted.iter().filter(|(p, _)| p == pr) {
            let k = s.key();
            if k != current.key() && !frozen(k) && !keys.contains(k) {
                keys.push(k.clone());
            }
        }
        Ok(keys)
    }

    async fn supersede_questions(
        &self,
        pr: &PrRef,
        superseded: &SessionKey,
        session: Option<&Session>,
        current: &Session,
    ) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        if std::mem::take(&mut state.fail_next_supersede) {
            return Err(Error::Port("the forge is unavailable".into()));
        }
        state.superseded.push((
            pr.clone(),
            superseded.clone(),
            session.cloned(),
            current.key().clone(),
        ));
        Ok(())
    }

    async fn set_check(&self, pr: &PrRef, status: CheckStatus, summary: &str) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        state.checks.push((pr.clone(), status));
        state.summaries.push((pr.clone(), summary.to_string()));
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
/// both, or a store that cannot be reached.
#[derive(Debug)]
pub enum AnyStore {
    Memory(InMemoryStore),
    File(FileStore),
    /// The wrapped store, as seen from a forge that cannot reach it: every
    /// call fails with `Error::Port` (ADR-0003 D4). Steps can still inspect
    /// the wrapped store with [`AnyStore::inner`].
    Unreachable(Box<AnyStore>),
    /// An in-memory store whose next `save_profile` fails while the flag is
    /// set, as on a transient write error.
    FailingProfileSave(InMemoryStore, std::sync::atomic::AtomicBool),
}

impl AnyStore {
    pub fn file(root: &Path) -> Self {
        AnyStore::File(FileStore::new(root))
    }

    pub fn memory() -> Self {
        AnyStore::Memory(InMemoryStore::new())
    }

    /// The store behind any `Unreachable` wrapper.
    pub fn inner(&self) -> &AnyStore {
        match self {
            AnyStore::Unreachable(s) => s.inner(),
            s => s,
        }
    }

    /// Make this store unreachable (or keep it so).
    pub fn into_unreachable(self) -> Self {
        match self {
            s @ AnyStore::Unreachable(_) => s,
            s => AnyStore::Unreachable(Box::new(s)),
        }
    }
}

fn unreachable() -> Error {
    Error::Port("the learner store cannot be reached".into())
}

macro_rules! delegate {
    ($self:ident, $method:ident($($arg:expr),*)) => {
        match $self {
            AnyStore::Memory(s) => s.$method($($arg),*).await,
            AnyStore::File(s) => s.$method($($arg),*).await,
            AnyStore::Unreachable(_) => Err(unreachable()),
            AnyStore::FailingProfileSave(s, _) => s.$method($($arg),*).await,
        }
    };
}

impl StateStore for AnyStore {
    async fn load_profile(&self, learner: &LearnerId) -> Result<Option<LearnerProfile>> {
        delegate!(self, load_profile(learner))
    }
    async fn save_profile(&self, profile: &LearnerProfile) -> Result<()> {
        if let AnyStore::FailingProfileSave(_, fail) = self
            && fail.swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(Error::Port("the learner profile could not be saved".into()));
        }
        delegate!(self, save_profile(profile))
    }
    async fn delete_learner(&self, learner: &LearnerId) -> Result<()> {
        delegate!(self, delete_learner(learner))
    }
    async fn learner_of(&self, identity: &Identity) -> Result<Option<LearnerId>> {
        delegate!(self, learner_of(identity))
    }
    async fn save_link(&self, link: &IdentityLink) -> Result<()> {
        delegate!(self, save_link(link))
    }
    async fn delete_link(&self, identity: &Identity) -> Result<()> {
        delegate!(self, delete_link(identity))
    }
    async fn links_of(&self, learner: &LearnerId) -> Result<Vec<IdentityLink>> {
        delegate!(self, links_of(learner))
    }
    async fn save_link_code(&self, code: &LinkCode) -> Result<()> {
        delegate!(self, save_link_code(code))
    }
    async fn load_link_code(&self, code: &str) -> Result<Option<LinkCode>> {
        delegate!(self, load_link_code(code))
    }
    async fn load_session(&self, key: &SessionKey) -> Result<Option<Session>> {
        delegate!(self, load_session(key))
    }
    async fn save_session(&self, session: &Session) -> Result<()> {
        delegate!(self, save_session(session))
    }
    async fn delete_session(&self, key: &SessionKey) -> Result<()> {
        delegate!(self, delete_session(key))
    }
    async fn take_session(&self, key: &SessionKey) -> Result<Option<Session>> {
        delegate!(self, take_session(key))
    }
    async fn save_graded(&self, graded: &GradedSession) -> Result<()> {
        delegate!(self, save_graded(graded))
    }
    async fn load_graded(&self, key: &SessionKey) -> Result<Option<GradedSession>> {
        delegate!(self, load_graded(key))
    }
    async fn delete_graded(&self, key: &SessionKey) -> Result<()> {
        delegate!(self, delete_graded(key))
    }
}

impl LearnerStore for AnyStore {}
