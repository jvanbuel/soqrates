//! Hermetic fakes for the ports of `soqrates-core` (ADR-0001 D12).
//!
//! Nothing here talks to a real model, forge or terminal. Everything is
//! deterministic so scenarios can assert exact outcomes.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, TimeZone, Utc};
use soqrates_core::answer_command::{Command, hints_due, parse_answers, read_command};
use soqrates_core::ports::{
    BlameLine, Clock, CodeCommit, CodeMatch, CodeSource, Codebase, Examiner, LearnerStore,
    Prompter, ScmProvider, StateStore,
};
use soqrates_core::{
    Answer, AnswerBody, AuthorId, BankedQuestion, CheckStatus, CodeLocation, CodeSnippet, Concept,
    ConceptId, ConceptScope, Diff, Error, Grade, GradedSession, Grading, GradingJournal, Identity,
    IdentityLink, LearnerId, LearnerProfile, LinkCode, PrRef, PrRounds, PullRequest, Question,
    QuestionBody, QuestionBrief, QuestionFormat, QuestionId, QuestionKind, Result, SeenQuestions,
    Session, SessionKey, SessionOutcome, SnippetOrigin, Weight,
};
use soqrates_store::{FileStore, InMemoryStore, SqlStore};

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
/// - Kinds (ADR-0007 D2): the first kind of the brief. `ExplainYourChange`
///   shows the brief's first hunk; the other kinds that show code show
///   [`FakeExaminer::general_snippet`].
/// - Grading (the answer key): the correct choice, or an open answer starting
///   with `correct`, gets the configurable `correct_grade` (default `Good`);
///   anything else gets `Again`.
/// - Code (ADR-0008): every step reads [`PROBE_FILE`] from the codebase it
///   is given, if there is one, and records what it read in
///   [`FakeExaminer::reads`].
/// - Scope (ADR-0003 D5): `General` unless a scenario classifies the concept
///   otherwise with [`FakeExaminer::classify`].
#[derive(Debug, Clone)]
pub struct FakeExaminer {
    correct_grade: Arc<Mutex<Grade>>,
    scopes: Arc<Mutex<HashMap<ConceptId, ConceptScope>>>,
    weights: Arc<Mutex<HashMap<ConceptId, Weight>>>,
    /// Whether multiple-choice questions come without option reasons, as
    /// questions stored before reasons existed do.
    without_reasons: Arc<Mutex<bool>>,
    /// What each step read: the step and the text of [`PROBE_FILE`].
    reads: Arc<Mutex<Vec<(ExaminerStep, String)>>>,
    /// The concept of every `write_question` call, in order.
    writes: Arc<Mutex<Vec<ConceptId>>>,
}

/// The file the [`FakeExaminer`] reads from every codebase it is given.
pub const PROBE_FILE: &str = "src/lib.rs";

/// An examiner step (ADR-0008 D2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExaminerStep {
    Extract,
    Write,
    Grade,
}

impl Default for FakeExaminer {
    fn default() -> Self {
        Self {
            reads: Arc::default(),
            writes: Arc::default(),
            correct_grade: Arc::new(Mutex::new(Grade::Good)),
            scopes: Arc::new(Mutex::new(HashMap::new())),
            weights: Arc::new(Mutex::new(HashMap::new())),
            without_reasons: Arc::new(Mutex::new(false)),
        }
    }
}

impl FakeExaminer {
    /// What each step read from its codebase, in order (ADR-0008 D2).
    pub fn reads(&self) -> Vec<(ExaminerStep, String)> {
        self.reads.lock().unwrap().clone()
    }

    /// Read [`PROBE_FILE`] from `code`, if there is any code, as `step`.
    async fn probe(&self, step: ExaminerStep, code: &impl Codebase) -> Result<()> {
        if code.is_available()
            && let Some(text) = code.read_file(PROBE_FILE).await?
        {
            self.reads.lock().unwrap().push((step, text));
        }
        Ok(())
    }

    /// The concept of every question written so far, in order.
    pub fn writes(&self) -> Vec<ConceptId> {
        self.writes.lock().unwrap().clone()
    }

    pub fn set_correct_grade(&self, grade: Grade) {
        *self.correct_grade.lock().unwrap() = grade;
    }

    /// The grade the answer key gives a correct answer.
    pub fn correct_grade(&self) -> Grade {
        *self.correct_grade.lock().unwrap()
    }

    /// Write multiple-choice questions without option reasons from now on.
    pub fn write_without_reasons(&self) {
        *self.without_reasons.lock().unwrap() = true;
    }

    /// Weigh `concept` as `weight` from now on (ADR-0002 D10).
    pub fn weigh(&self, concept: &ConceptId, weight: Weight) {
        self.weights.lock().unwrap().insert(concept.clone(), weight);
    }

    /// The weight this examiner gives `concept`: `Core` by default.
    pub fn weight_of(&self, concept: &ConceptId) -> Weight {
        self.weights
            .lock()
            .unwrap()
            .get(concept)
            .copied()
            .unwrap_or_default()
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
    async fn extract_concepts(&self, diff: &Diff, code: &impl Codebase) -> Result<Vec<Concept>> {
        self.probe(ExaminerStep::Extract, code).await?;
        Ok(marker_lines(diff)
            .into_iter()
            .map(|(id, location)| {
                let cid = ConceptId(id.trim().to_string());
                Concept {
                    scope: self.scope_of(&cid),
                    weight: self.weight_of(&cid),
                    id: cid,
                    title: id.trim().to_string(),
                    summary: format!("What the change does with {}", id.trim()),
                    locations: location.into_iter().collect(),
                }
            })
            .collect())
    }

    /// Writes the brief's first kind (ADR-0007 D2): the author's first hunk
    /// for `ExplainYourChange`, a general snippet for the other kinds that
    /// show code, with code options for `ChooseTheFix`.
    async fn write_question(
        &self,
        concept: &Concept,
        brief: &QuestionBrief,
        codebase: &impl Codebase,
    ) -> Result<Question> {
        self.probe(ExaminerStep::Write, codebase).await?;
        // A concept written about again gets a new variant, as a model's
        // questions vary (ADR-0009 D5).
        let variant = {
            let mut writes = self.writes.lock().unwrap();
            writes.push(concept.id.clone());
            writes.iter().filter(|c| **c == concept.id).count()
        };
        let again = if variant > 1 {
            format!(" (variant {variant})")
        } else {
            String::new()
        };
        let format = brief.format;
        let kind = brief.kinds[0];
        let code = match kind {
            QuestionKind::Scenario => None,
            QuestionKind::ExplainYourChange => brief.hunks.first().map(|h| h.snippet()),
            _ => Some(Self::general_snippet(&concept.id)),
        };
        let options_language = (kind == QuestionKind::ChooseTheFix).then(|| "rust".to_string());
        let id = Self::question_id(&concept.id, format);
        let body = match format {
            QuestionFormat::MultipleChoice => {
                let options: Vec<String> = vec![
                    "the right one".into(),
                    "a plausible wrong one".into(),
                    "another wrong one".into(),
                    "an obviously wrong one".into(),
                ];
                let without_reasons = *self.without_reasons.lock().unwrap();
                let reasons = options
                    .iter()
                    .filter(|_| !without_reasons)
                    .enumerate()
                    .map(|(i, option)| match i {
                        0 => Self::explanation(&concept.id),
                        _ => Self::reason(&concept.id, option),
                    })
                    .collect();
                QuestionBody::MultipleChoice {
                    prompt: format!("Which statement about {} is true?{again}", concept.title),
                    code,
                    options,
                    options_language,
                    correct: 0,
                    reasons,
                }
            }
            QuestionFormat::Open => QuestionBody::Open {
                prompt: format!("Explain {} in this change.{again}", concept.title),
                code,
                rubric: format!("Covers {}", concept.title),
            },
        };
        Ok(Question {
            id,
            concept: concept.id.clone(),
            scope: concept.scope,
            kind,
            body,
            explanation: Self::explanation(&concept.id),
            // The fake's reasons discuss only its options (issue #5).
            checked: true,
            banked: None,
        })
    }

    async fn grade(
        &self,
        question: &Question,
        answer: &Answer,
        code: &impl Codebase,
    ) -> Result<Grading> {
        self.probe(ExaminerStep::Grade, code).await?;
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

/// The concept of each `+// concept: <id>` line of `diff`, with where it is
/// on the new side (ADR-0007 D4), if it is under a file header: from its
/// marker line to the line before the next marker, or the end of its hunk.
fn marker_lines(diff: &Diff) -> Vec<(&str, Option<CodeLocation>)> {
    let mut out: Vec<(&str, Option<CodeLocation>)> = Vec::new();
    let mut file: Option<&str> = None;
    let mut line = 0u32;
    // The location still growing: that of the latest marker in this hunk.
    let mut growing: Option<usize> = None;
    for l in diff.0.lines() {
        if let Some(header) = l.strip_prefix("diff --git ") {
            file = header.rsplit_once(" b/").map(|(_, b)| b);
            growing = None;
        } else if let Some(hunk) = l.strip_prefix("@@ ") {
            line = hunk
                .split_whitespace()
                .find_map(|w| w.strip_prefix('+'))
                .and_then(|w| w.split(',').next()?.parse().ok())
                .unwrap_or(1);
            growing = None;
        } else if let Some(id) = l.strip_prefix(CONCEPT_MARKER) {
            let location = file.map(|f| CodeLocation {
                file: f.to_string(),
                start: line,
                end: line,
            });
            growing = location.is_some().then_some(out.len());
            out.push((id, location));
            line += 1;
        } else if l.starts_with(' ') || (l.starts_with('+') && !l.starts_with("+++")) {
            if let Some(Some(loc)) = growing.map(|i| &mut out[i].1) {
                loc.end = line;
            }
            line += 1;
        }
    }
    out
}

impl FakeExaminer {
    /// The general snippet the fake writes about `concept`: code it wrote
    /// itself, never the author's.
    pub fn general_snippet(concept: &ConceptId) -> CodeSnippet {
        CodeSnippet {
            language: "rust".into(),
            text: format!(
                "fn example() {{\n    // a general example of {}\n}}",
                concept.0
            ),
            origin: SnippetOrigin::General,
        }
    }

    /// The explanation the fake writes for `concept`.
    pub fn explanation(concept: &ConceptId) -> String {
        format!("Because {} works that way.", concept.0)
    }

    /// The reason the fake gives for the wrong `option` about `concept`.
    pub fn reason(concept: &ConceptId, option: &str) -> String {
        format!("\"{option}\" is not how {} works.", concept.0)
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
    /// The commits of each pull request's branch, oldest first: the head
    /// after the commit and the change it made. The pull request's diff is
    /// all of them together.
    pub commits: HashMap<PrRef, Vec<(String, Diff)>>,
    /// Every `diff_between` call, in order: the pull request, from and to.
    pub compared: Vec<(PrRef, String, String)>,
    /// Every `post_questions` call, in order.
    pub posted: Vec<(PrRef, Session)>,
    /// Every `set_check` call, in order.
    pub checks: Vec<(PrRef, CheckStatus)>,
    /// The summary of every `set_check` call, in the same order as `checks`.
    pub summaries: Vec<(PrRef, String)>,
    /// Replies by the author, keyed by the session they answer.
    pub replies: HashMap<SessionKey, Vec<Answer>>,
    /// Answers that don't count as a reply (ADR-0004 D3): ticked but not
    /// submitted, or given by someone other than the author.
    pub ignored: Vec<(SessionKey, Vec<Answer>)>,
    /// Make the next `post_questions` call fail, as on a transient error.
    pub fail_next_post: bool,
    /// Every `post_results` call, in order.
    pub results: Vec<(PrRef, SessionKey, SessionOutcome)>,
    /// Question comments frozen as superseded: the session key, the session
    /// shown (if it was still stored) and the key of the newer session.
    pub superseded: Vec<(PrRef, SessionKey, Option<Session>, SessionKey)>,
    /// Make the next `supersede_questions` call fail.
    pub fail_next_supersede: bool,
    /// The slug of the app Soqrates posts as, if any (ADR-0004 D8).
    pub app_slug: Option<String>,
    /// Comments posted on pull requests after a question comment, in order.
    pub comments: Vec<FakeComment>,
    /// The comments that got a hint, by id, in order (ADR-0004 D8).
    pub hints: Vec<u64>,
    /// What submitted the answers that `fetch_answers` last found, by
    /// session (ADR-0004 D9).
    pub submitted_by: HashMap<SessionKey, Submitter>,
    /// The reactions currently on submitting comments (ADR-0004 D9).
    pub reactions: Vec<(Submitter, Reaction)>,
    /// What the author saw happen on the pull request, in order: check
    /// summaries, reactions and results (ADR-0004 D9).
    pub timeline: Vec<String>,
}

/// A comment on a pull request, posted after the question comment of
/// `session`.
#[derive(Debug, Clone)]
pub struct FakeComment {
    pub id: u64,
    pub pr: PrRef,
    pub session: SessionKey,
    pub author: AuthorId,
    pub body: String,
}

/// What submitted a session's answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Submitter {
    /// Answers given with [`FakeScm::reply`], e.g. by ticking boxes.
    Reply,
    /// The comment with this id.
    Comment(u64),
}

/// A reaction Soqrates puts on the submitting comment (ADR-0004 D9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reaction {
    /// 👀: the answers were picked up and are being graded.
    Eyes,
    /// ✅ (👍 on GitHub): the answers were graded.
    Done,
}

/// A fake forge. Records what Soqrates posts and lets steps inject the
/// author's reply.
#[derive(Debug, Clone, Default)]
pub struct FakeScm {
    pub state: Arc<Mutex<ScmState>>,
}

impl FakeScm {
    /// Open `pr`, or replace its branch with one commit making `pr.diff`, as
    /// on a force-push.
    pub fn open(&self, pr: PullRequest) {
        let mut state = self.state.lock().unwrap();
        state
            .commits
            .insert(pr.pr.clone(), vec![(pr.head_sha.clone(), pr.diff.clone())]);
        state.pull_requests.insert(pr.pr.clone(), pr);
    }

    /// Push one commit making `change` onto the branch of `pr`.
    pub fn push(&self, pr: &PrRef, head: &str, change: Diff) {
        let mut state = self.state.lock().unwrap();
        let commits = state.commits.entry(pr.clone()).or_default();
        commits.push((head.to_string(), change));
        let diff = Diff(commits.iter().map(|(_, d)| d.0.as_str()).collect());
        let pull = state
            .pull_requests
            .get_mut(pr)
            .expect("push to an open pull request");
        pull.head_sha = head.to_string();
        pull.diff = diff;
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

    /// Answers to `session` by `by`, `submitted` or only ticked. Only those
    /// the session's author submitted are a reply (ADR-0004 D3).
    pub fn answer_as(
        &self,
        session: &Session,
        by: &AuthorId,
        submitted: bool,
        answers: Vec<Answer>,
    ) {
        if submitted && by == session.author() {
            self.reply(session.key(), answers);
        } else {
            self.state
                .lock()
                .unwrap()
                .ignored
                .push((session.key().clone(), answers));
        }
    }

    /// Post a comment by `author` after the question comment of `session`;
    /// returns its id.
    pub fn comment(&self, pr: &PrRef, session: &SessionKey, author: &AuthorId, body: &str) -> u64 {
        let mut state = self.state.lock().unwrap();
        let id = state.comments.len() as u64 + 1;
        state.comments.push(FakeComment {
            id,
            pr: pr.clone(),
            session: session.clone(),
            author: author.clone(),
            body: body.to_string(),
        });
        id
    }

    /// Replace the body of the comment `id`, as an edit does.
    pub fn edit_comment(&self, id: u64, body: &str) {
        let mut state = self.state.lock().unwrap();
        let comment = state
            .comments
            .iter_mut()
            .find(|c| c.id == id)
            .expect("edit an existing comment");
        comment.body = body.to_string();
    }

    pub fn reply(&self, session: &SessionKey, answers: Vec<Answer>) {
        self.state
            .lock()
            .unwrap()
            .replies
            .insert(session.clone(), answers);
    }
}

/// The text of every file in the pull request's checkout at `revision`.
pub fn file_at(revision: &str) -> String {
    format!("the code at {revision}")
}

/// A codebase in which every file reads as its `text` (ADR-0008 D4). The
/// other operations find nothing: the fake examiner only reads.
#[derive(Debug, Clone)]
pub struct FakeCodebase(pub String);

impl Codebase for FakeCodebase {
    async fn read_file(&self, _path: &str) -> Result<Option<String>> {
        Ok(Some(self.0.clone()))
    }
    async fn list_files(&self) -> Result<Vec<String>> {
        Ok(vec![PROBE_FILE.to_string()])
    }
    async fn grep(
        &self,
        _pattern: &str,
        _glob: Option<&str>,
        _ignore_case: bool,
        _limit: usize,
    ) -> Result<Vec<CodeMatch>> {
        Ok(Vec::new())
    }
    async fn history(&self, _path: &str, _limit: usize) -> Result<Vec<CodeCommit>> {
        Ok(Vec::new())
    }
    async fn blame(&self, _path: &str, _start: u32, _end: u32) -> Result<Option<Vec<BlameLine>>> {
        Ok(None)
    }
}

/// The checkout of a pull request, as the action makes it (ADR-0008 D4):
/// every commit reads as [`file_at`] it.
#[derive(Debug, Clone, Copy, Default)]
pub struct FakeCheckout;

impl CodeSource for FakeCheckout {
    type Code = FakeCodebase;
    fn at(&self, revision: &str) -> FakeCodebase {
        FakeCodebase(file_at(revision))
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

    async fn diff_between(&self, pr: &PrRef, from: &str, to: &str) -> Result<Option<Diff>> {
        let mut state = self.state.lock().unwrap();
        state
            .compared
            .push((pr.clone(), from.to_string(), to.to_string()));
        let commits = state.commits.get(pr).cloned().unwrap_or_default();
        let (Some(start), Some(end)) = (
            commits.iter().position(|(h, _)| h == from),
            commits.iter().position(|(h, _)| h == to),
        ) else {
            return Ok(None);
        };
        if start > end {
            return Ok(None);
        }
        Ok(Some(Diff(
            commits[start + 1..=end]
                .iter()
                .map(|(_, d)| d.0.as_str())
                .collect(),
        )))
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
        let mut state = self.state.lock().unwrap();
        state
            .results
            .push((pr.clone(), session.key().clone(), outcome.clone()));
        state.timeline.push("results".into());
        // 👀 becomes ✅ (ADR-0004 D9).
        if let Some(by) = state.submitted_by.get(session.key()).copied() {
            state
                .reactions
                .retain(|(s, r)| !(*s == by && *r == Reaction::Eyes));
            state.reactions.push((by, Reaction::Done));
            state.timeline.push(format!("{by:?}: {:?}", Reaction::Done));
        }
        Ok(())
    }

    /// The structured answers given with [`FakeScm::reply`], else the
    /// latest answer comment of the author, read as the GitHub adapter
    /// reads it (ADR-0004 D8).
    async fn fetch_answers(&self, pr: &PrRef, session: &Session) -> Result<Option<Vec<Answer>>> {
        let mut state = self.state.lock().unwrap();
        if let Some(answers) = state.replies.get(session.key()).cloned() {
            state
                .submitted_by
                .insert(session.key().clone(), Submitter::Reply);
            return Ok(Some(answers));
        }
        let slug = state.app_slug.clone();
        let found = state
            .comments
            .iter()
            .rev()
            .filter(|c| c.pr == *pr && c.session == *session.key() && c.author == *session.author())
            .find(|c| matches!(read_command(&c.body, slug.as_deref()), Command::Answer(_)))
            .map(|c| {
                (
                    c.id,
                    parse_answers(&c.body, session.questions(), slug.as_deref()),
                )
            });
        match found {
            Some((id, answers @ Some(_))) => {
                state
                    .submitted_by
                    .insert(session.key().clone(), Submitter::Comment(id));
                Ok(answers)
            }
            _ => Ok(None),
        }
    }

    async fn hint_commands(&self, pr: &PrRef, session: &Session) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        let slug = state.app_slug.clone();
        let candidates: Vec<(u64, String)> = state
            .comments
            .iter()
            .filter(|c| c.pr == *pr && c.session == *session.key() && c.author == *session.author())
            .map(|c| (c.id, c.body.clone()))
            .collect();
        let candidates: Vec<(u64, &str)> =
            candidates.iter().map(|(i, b)| (*i, b.as_str())).collect();
        let due = hints_due(&candidates, &state.hints, slug.as_deref());
        state.hints.extend(due);
        Ok(())
    }

    async fn acknowledge_answers(&self, _pr: &PrRef, session: &Session) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        if let Some(by) = state.submitted_by.get(session.key()).copied() {
            state.reactions.push((by, Reaction::Eyes));
            state.timeline.push(format!("{by:?}: {:?}", Reaction::Eyes));
        }
        Ok(())
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
        state.timeline.push(format!("check {status:?}: {summary}"));
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
/// both, or a store that cannot be reached or fails to write.
#[derive(Debug)]
pub enum AnyStore {
    Memory(InMemoryStore),
    File(FileStore),
    /// The SQL database store on SQLite (ADR-0005).
    Sql(SqlStore),
    /// The wrapped store, as seen from a forge that cannot reach it: every
    /// call fails with `Error::Port` (ADR-0003 D4). Steps can still inspect
    /// the wrapped store with [`AnyStore::inner`].
    Unreachable(Box<AnyStore>),
    /// The wrapped store with injected write failures, as on a transient
    /// error or a process that stops part-way (ADR-0002 D6).
    Faulty(Box<AnyStore>, Mutex<Faults>),
}

/// Which writes of an [`AnyStore::Faulty`] store fail.
#[derive(Debug, Clone)]
pub struct Faults {
    /// Only writes by this port method count; all writes when `None`.
    pub only: Option<String>,
    /// Writes that still succeed before the failures start.
    pub allowed: usize,
    /// Writes that fail after those; `usize::MAX` for all of them.
    pub failing: usize,
}

impl AnyStore {
    pub fn file(root: &Path) -> Self {
        AnyStore::File(FileStore::new(root))
    }

    /// The SQLite database `state.db` in `dir`, created if missing.
    pub async fn sqlite(dir: &Path) -> Self {
        let url = format!("sqlite://{}?mode=rwc", dir.join("state.db").display());
        AnyStore::Sql(SqlStore::connect(&url).await.expect("connect to SQLite"))
    }

    pub fn memory() -> Self {
        AnyStore::Memory(InMemoryStore::new())
    }

    /// The store behind any `Unreachable` or `Faulty` wrapper.
    pub fn inner(&self) -> &AnyStore {
        match self {
            AnyStore::Unreachable(s) | AnyStore::Faulty(s, _) => s.inner(),
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

    /// This store with `faults` injected, replacing any injected before.
    pub fn into_faulty(self, faults: Faults) -> Self {
        match self.into_healthy() {
            s @ (AnyStore::Memory(_) | AnyStore::File(_) | AnyStore::Sql(_)) => {
                AnyStore::Faulty(Box::new(s), Mutex::new(faults))
            }
            other => panic!("faults are injected into a reachable store, not {other:?}"),
        }
    }

    /// This store without injected faults.
    pub fn into_healthy(self) -> Self {
        match self {
            AnyStore::Faulty(s, _) => s.into_healthy(),
            s => s,
        }
    }

    /// Fail the write by `method` if the injected faults say so.
    fn fault(&self, method: &str) -> Result<()> {
        let AnyStore::Faulty(_, faults) = self else {
            return Ok(());
        };
        let mut f = faults.lock().unwrap();
        if f.only.as_deref().is_some_and(|m| m != method) {
            return Ok(());
        }
        if f.allowed > 0 {
            f.allowed -= 1;
            return Ok(());
        }
        if f.failing == 0 {
            return Ok(());
        }
        if f.failing != usize::MAX {
            f.failing -= 1;
        }
        Err(Error::Port(format!("the store failed to {method}")))
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
            AnyStore::Sql(s) => s.$method($($arg),*).await,
            AnyStore::Unreachable(_) => Err(unreachable()),
            // Not recursive, so the futures stay nameable.
            AnyStore::Faulty(s, _) => match &**s {
                AnyStore::Memory(s) => s.$method($($arg),*).await,
                AnyStore::File(s) => s.$method($($arg),*).await,
                AnyStore::Sql(s) => s.$method($($arg),*).await,
                _ => unreachable!("faults are injected into a base store"),
            },
        }
    };
}

/// A write: fails if injected faults say so, else is delegated.
macro_rules! write {
    ($self:ident, $method:ident($($arg:expr),*)) => {{
        $self.fault(stringify!($method))?;
        delegate!($self, $method($($arg),*))
    }};
}

impl StateStore for AnyStore {
    async fn load_profile(&self, learner: &LearnerId) -> Result<Option<LearnerProfile>> {
        delegate!(self, load_profile(learner))
    }
    async fn save_profile(&self, profile: &LearnerProfile) -> Result<()> {
        write!(self, save_profile(profile))
    }
    async fn delete_learner(&self, learner: &LearnerId) -> Result<()> {
        write!(self, delete_learner(learner))
    }
    async fn learner_of(&self, identity: &Identity) -> Result<Option<LearnerId>> {
        delegate!(self, learner_of(identity))
    }
    async fn save_link(&self, link: &IdentityLink) -> Result<()> {
        write!(self, save_link(link))
    }
    async fn delete_link(&self, identity: &Identity) -> Result<()> {
        write!(self, delete_link(identity))
    }
    async fn links_of(&self, learner: &LearnerId) -> Result<Vec<IdentityLink>> {
        delegate!(self, links_of(learner))
    }
    async fn save_link_code(&self, code: &LinkCode) -> Result<()> {
        write!(self, save_link_code(code))
    }
    async fn load_link_code(&self, code: &str) -> Result<Option<LinkCode>> {
        delegate!(self, load_link_code(code))
    }
    async fn load_session(&self, key: &SessionKey) -> Result<Option<Session>> {
        delegate!(self, load_session(key))
    }
    async fn save_session(&self, session: &Session) -> Result<()> {
        write!(self, save_session(session))
    }
    async fn delete_session(&self, key: &SessionKey) -> Result<()> {
        write!(self, delete_session(key))
    }
    async fn begin_grading(&self, journal: &GradingJournal) -> Result<bool> {
        write!(self, begin_grading(journal))
    }
    async fn load_grading(&self, key: &SessionKey) -> Result<Option<GradingJournal>> {
        delegate!(self, load_grading(key))
    }
    async fn gradings(&self) -> Result<Vec<GradingJournal>> {
        delegate!(self, gradings())
    }
    async fn end_grading(&self, key: &SessionKey) -> Result<()> {
        write!(self, end_grading(key))
    }
    async fn save_graded(&self, graded: &GradedSession) -> Result<()> {
        write!(self, save_graded(graded))
    }
    async fn load_graded(&self, key: &SessionKey) -> Result<Option<GradedSession>> {
        delegate!(self, load_graded(key))
    }
    async fn load_rounds(&self, pr: &PrRef) -> Result<Option<PrRounds>> {
        delegate!(self, load_rounds(pr))
    }
    async fn save_rounds(&self, rounds: &PrRounds) -> Result<()> {
        write!(self, save_rounds(rounds))
    }
    async fn bank_questions(&self, concept: &ConceptId) -> Result<Vec<BankedQuestion>> {
        delegate!(self, bank_questions(concept))
    }
    async fn save_bank_question(&self, question: &BankedQuestion) -> Result<()> {
        write!(self, save_bank_question(question))
    }
    async fn load_seen(&self, learner: &LearnerId) -> Result<Option<SeenQuestions>> {
        delegate!(self, load_seen(learner))
    }
    async fn save_seen(&self, seen: &SeenQuestions) -> Result<()> {
        write!(self, save_seen(seen))
    }
}

impl LearnerStore for AnyStore {}
