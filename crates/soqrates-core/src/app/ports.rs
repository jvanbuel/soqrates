//! Ports implemented by adapter crates (ADR-0001 D2).
//!
//! The async methods return `impl Future + Send`, so implementers can simply
//! write `async fn` in their impls.

use std::future::Future;

use chrono::{DateTime, Utc};

use crate::Result;
use crate::app::engine::SessionOutcome;
use crate::assessment::{Answer, Concept, Diff, Grading, Question, QuestionBrief};
use crate::delivery::{
    BankedQuestion, CheckStatus, GradedSession, GradingJournal, PrRef, PrRounds, PullRequest,
    SeenQuestions, Session,
};
use crate::ids::{ConceptId, Identity, LearnerId, SessionKey};
use crate::learner::{IdentityLink, LearnerProfile, LinkCode};

/// Persists learner profiles, identity links, link codes and pending
/// sessions (ADR-0001 D5, ADR-0003).
pub trait StateStore: Send + Sync {
    fn load_profile(
        &self,
        learner: &LearnerId,
    ) -> impl Future<Output = Result<Option<LearnerProfile>>> + Send;
    fn save_profile(&self, profile: &LearnerProfile) -> impl Future<Output = Result<()>> + Send;
    /// Remove the profile and every link of `learner` (ADR-0003 D6).
    fn delete_learner(&self, learner: &LearnerId) -> impl Future<Output = Result<()>> + Send;

    /// The learner an identity is linked to, if any (ADR-0003 D1).
    fn learner_of(
        &self,
        identity: &Identity,
    ) -> impl Future<Output = Result<Option<LearnerId>>> + Send;
    /// Create or replace the link of `link.identity` (at most one per identity).
    fn save_link(&self, link: &IdentityLink) -> impl Future<Output = Result<()>> + Send;
    fn delete_link(&self, identity: &Identity) -> impl Future<Output = Result<()>> + Send;
    fn links_of(
        &self,
        learner: &LearnerId,
    ) -> impl Future<Output = Result<Vec<IdentityLink>>> + Send;

    fn save_link_code(&self, code: &LinkCode) -> impl Future<Output = Result<()>> + Send;
    fn load_link_code(&self, code: &str) -> impl Future<Output = Result<Option<LinkCode>>> + Send;

    fn load_session(
        &self,
        key: &SessionKey,
    ) -> impl Future<Output = Result<Option<Session>>> + Send;
    fn save_session(&self, session: &Session) -> impl Future<Output = Result<()>> + Send;
    fn delete_session(&self, key: &SessionKey) -> impl Future<Output = Result<()>> + Send;
    /// Atomically claim the pending session `journal.key()` and store
    /// `journal` as its grading journal: the session is no longer pending
    /// but being graded. Of several concurrent callers, at most one gets
    /// `true`; `false` when the session is not pending (ADR-0002 D6).
    fn begin_grading(&self, journal: &GradingJournal) -> impl Future<Output = Result<bool>> + Send;
    /// The grading journal of the session `key`, while it is being graded.
    fn load_grading(
        &self,
        key: &SessionKey,
    ) -> impl Future<Output = Result<Option<GradingJournal>>> + Send;
    /// Every grading journal in this store.
    fn gradings(&self) -> impl Future<Output = Result<Vec<GradingJournal>>> + Send;
    /// Remove the grading journal `key` once its writes are done.
    fn end_grading(&self, key: &SessionKey) -> impl Future<Output = Result<()>> + Send;

    /// Record that a session was graded (ADR-0002 D6).
    fn save_graded(&self, graded: &GradedSession) -> impl Future<Output = Result<()>> + Send;
    fn load_graded(
        &self,
        key: &SessionKey,
    ) -> impl Future<Output = Result<Option<GradedSession>>> + Send;

    /// The rounds asked on `pr` so far (ADR-0002 D9).
    fn load_rounds(&self, pr: &PrRef) -> impl Future<Output = Result<Option<PrRounds>>> + Send;
    /// Create or replace the rounds of `rounds.pr()`.
    fn save_rounds(&self, rounds: &PrRounds) -> impl Future<Output = Result<()>> + Send;

    /// The question bank's questions about `concept`, retired ones
    /// included, in the order they were banked (ADR-0009 D1).
    fn bank_questions(
        &self,
        concept: &ConceptId,
    ) -> impl Future<Output = Result<Vec<BankedQuestion>>> + Send;
    /// Create or replace the bank question `question.id()`.
    fn save_bank_question(
        &self,
        question: &BankedQuestion,
    ) -> impl Future<Output = Result<()>> + Send;
    /// The bank questions `learner` was shown (ADR-0009 D5).
    fn load_seen(
        &self,
        learner: &LearnerId,
    ) -> impl Future<Output = Result<Option<SeenQuestions>>> + Send;
    /// Create or replace what `seen.learner()` was shown.
    fn save_seen(&self, seen: &SeenQuestions) -> impl Future<Output = Result<()>> + Send;
}

/// The store holding the profiles of linked learners, reachable from several
/// forges and owned by the learner or by an organisation (ADR-0003 D4). It
/// stores only general concepts (D5). A port failure means "unreachable".
pub trait LearnerStore: StateStore {}

/// A line that [`Codebase::grep`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeMatch {
    pub path: String,
    /// 1-based.
    pub line: u32,
    pub text: String,
}

/// A commit in a file's history ([`Codebase::history`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeCommit {
    /// Abbreviated commit id.
    pub id: String,
    pub author: String,
    /// `YYYY-MM-DD`.
    pub date: String,
    /// The first line of the message.
    pub summary: String,
}

/// A line of a file with the commit that last changed it
/// ([`Codebase::blame`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlameLine {
    /// 1-based.
    pub line: u32,
    /// Abbreviated commit id; all zeros for a change not committed yet.
    pub commit: String,
    pub author: String,
    /// `YYYY-MM-DD`.
    pub date: String,
    pub text: String,
}

/// A read-only view of the repository at the revision a change produces
/// (ADR-0008 D1). Reads only what version control holds at that revision:
/// never the working tree, untracked or ignored files, or anything outside
/// the repository. Paths are as [`crate::assessment::code_path`] returns
/// them, and patterns are checked by the examiner; implementations may
/// assume so.
pub trait Codebase: Send + Sync {
    /// Whether there is any code to read. `false` for [`NoCodebase`]: the
    /// examiner then offers no tools (ADR-0008 D5).
    fn is_available(&self) -> bool {
        true
    }
    /// The text of the file at `path`, or `None` when there is no such
    /// file, or it isn't text.
    fn read_file(&self, path: &str) -> impl Future<Output = Result<Option<String>>> + Send;
    /// Every file path at the revision, sorted.
    fn list_files(&self) -> impl Future<Output = Result<Vec<String>>> + Send;
    /// The lines matching the extended regular expression `pattern`, in
    /// files matching `glob` if given (`**` matches across directories),
    /// at most `limit`, in path and line order. Binary files are skipped.
    fn grep(
        &self,
        pattern: &str,
        glob: Option<&str>,
        ignore_case: bool,
        limit: usize,
    ) -> impl Future<Output = Result<Vec<CodeMatch>>> + Send;
    /// The latest `limit` commits that changed `path` (`""` for the whole
    /// repository), newest first, up to the revision. May be cut short
    /// where the history is.
    fn history(
        &self,
        path: &str,
        limit: usize,
    ) -> impl Future<Output = Result<Vec<CodeCommit>>> + Send;
    /// Lines `start..=end` of `path` with the commit that last changed each;
    /// `None` when there is no such file.
    fn blame(
        &self,
        path: &str,
        start: u32,
        end: u32,
    ) -> impl Future<Output = Result<Option<Vec<BlameLine>>>> + Send;
}

/// The codebase of a surface that has none, such as MCP (ADR-0008 D4).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoCodebase;

impl Codebase for NoCodebase {
    fn is_available(&self) -> bool {
        false
    }
    async fn read_file(&self, _path: &str) -> Result<Option<String>> {
        Ok(None)
    }
    async fn list_files(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
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

impl<T: Codebase> Codebase for &T {
    fn is_available(&self) -> bool {
        (**self).is_available()
    }
    fn read_file(&self, path: &str) -> impl Future<Output = Result<Option<String>>> + Send {
        (**self).read_file(path)
    }
    fn list_files(&self) -> impl Future<Output = Result<Vec<String>>> + Send {
        (**self).list_files()
    }
    fn grep(
        &self,
        pattern: &str,
        glob: Option<&str>,
        ignore_case: bool,
        limit: usize,
    ) -> impl Future<Output = Result<Vec<CodeMatch>>> + Send {
        (**self).grep(pattern, glob, ignore_case, limit)
    }
    fn history(
        &self,
        path: &str,
        limit: usize,
    ) -> impl Future<Output = Result<Vec<CodeCommit>>> + Send {
        (**self).history(path, limit)
    }
    fn blame(
        &self,
        path: &str,
        start: u32,
        end: u32,
    ) -> impl Future<Output = Result<Option<Vec<BlameLine>>>> + Send {
        (**self).blame(path, start, end)
    }
}

/// Where the CI check finds the code of a pull request at a commit
/// (ADR-0008 D4): a checkout of it, or nowhere.
pub trait CodeSource: Send + Sync {
    type Code: Codebase;
    /// The codebase at commit `revision`. Not available when the commit
    /// isn't there, e.g. after a force-push beyond the checkout's history.
    fn at(&self, revision: &str) -> Self::Code;
}

/// A CI check without the code: every revision is [`NoCodebase`].
#[derive(Debug, Clone, Copy, Default)]
pub struct NoCodeSource;

impl CodeSource for NoCodeSource {
    type Code = NoCodebase;
    fn at(&self, _revision: &str) -> NoCodebase {
        NoCodebase
    }
}

/// The LLM-backed part of Soqrates: understands diffs, writes and grades
/// questions (D3). Implemented by `soqrates-examiner` on top of Rig. Each
/// step may read `code`, the codebase the change is made in (ADR-0008 D2).
pub trait Examiner: Send + Sync {
    /// The concepts `diff` exercises, each with where in the diff it was
    /// found (ADR-0007 D4).
    fn extract_concepts(
        &self,
        diff: &Diff,
        code: &impl Codebase,
    ) -> impl Future<Output = Result<Vec<Concept>>> + Send;
    /// Write a question about `concept` as `brief` asks: its format, the
    /// first of its kinds that suits the concept, and code from or
    /// mirroring its hunks (ADR-0007 D2-D4).
    fn write_question(
        &self,
        concept: &Concept,
        brief: &QuestionBrief,
        code: &impl Codebase,
    ) -> impl Future<Output = Result<Question>> + Send;
    /// Write a follow-up question about `concept` as `brief` asks
    /// (ADR-0010 D1, D3): a new question, from another angle than
    /// `previous`, which the author answered with `answer` (`None` when
    /// unanswered) and got wrong. It must not repeat `previous`. The default
    /// fails: an examiner that can't write follow-ups asks none.
    fn write_follow_up(
        &self,
        concept: &Concept,
        brief: &QuestionBrief,
        previous: &Question,
        answer: Option<&Answer>,
        code: &impl Codebase,
    ) -> impl Future<Output = Result<Question>> + Send {
        let _ = (concept, brief, previous, answer, code);
        async {
            Err(crate::Error::Port(
                "examiner: writes no follow-up questions".into(),
            ))
        }
    }
    /// Grade `answer` and explain the grade to the author (ADR-0004 D1).
    fn grade(
        &self,
        question: &Question,
        answer: &Answer,
        code: &impl Codebase,
    ) -> impl Future<Output = Result<Grading>> + Send;
}

/// A forge/CI provider (D4). Implemented by `soqrates-scm-github` and friends.
pub trait ScmProvider: Send + Sync {
    fn pull_request(&self, pr: &PrRef) -> impl Future<Output = Result<PullRequest>> + Send;
    /// The change on `pr` from commit `from` to commit `to`, or `None` when
    /// `from` is not an ancestor of `to`, e.g. after a force-push
    /// (ADR-0002 D9).
    fn diff_between(
        &self,
        pr: &PrRef,
        from: &str,
        to: &str,
    ) -> impl Future<Output = Result<Option<Diff>>> + Send;
    /// Post the session's questions on the PR, e.g. as a comment. A session
    /// without questions is posted too, saying there is nothing to answer
    /// (ADR-0004 D7).
    fn post_questions(
        &self,
        pr: &PrRef,
        session: &Session,
    ) -> impl Future<Output = Result<()>> + Send;
    /// The author's answers to `session`, if they have submitted them
    /// (ADR-0004 D3). Answers by anyone else, or ticked but not submitted,
    /// are `None`.
    fn fetch_answers(
        &self,
        pr: &PrRef,
        session: &Session,
    ) -> impl Future<Output = Result<Option<Vec<Answer>>>> + Send;
    /// Point out, once each, the author's comments on `session` that start
    /// like an answer command but aren't one, e.g. with a hint to use
    /// `/soqrates answer` (ADR-0004 D8). Best effort: providers without
    /// comments do nothing.
    fn hint_commands(
        &self,
        _pr: &PrRef,
        _session: &Session,
    ) -> impl Future<Output = Result<()>> + Send {
        async { Ok(()) }
    }
    /// Show the author that their answers to `session` were picked up and
    /// are being graded, e.g. with a 👀 reaction on the comment that
    /// submitted them (ADR-0004 D9). Called after
    /// [`ScmProvider::fetch_answers`] returned answers, before grading.
    fn acknowledge_answers(
        &self,
        _pr: &PrRef,
        _session: &Session,
    ) -> impl Future<Output = Result<()>> + Send {
        async { Ok(()) }
    }
    /// Post the per-question results of a graded session and mark the
    /// question comment as graded (ADR-0004 D1, D2, D5). Shows this
    /// session's results only, never the learner's profile. Replaces the
    /// acknowledgement of [`ScmProvider::acknowledge_answers`] with one
    /// saying the answers were graded (ADR-0004 D9).
    fn post_results(
        &self,
        pr: &PrRef,
        session: &Session,
        outcome: &SessionOutcome,
    ) -> impl Future<Output = Result<()>> + Send;
    /// The sessions whose question comments on `pr` still take answers
    /// although a newer push superseded them: posted by the same account as
    /// `current`'s question comment, for another session, and neither graded
    /// nor superseded yet (ADR-0004 D7). Empty when `current` has no question
    /// comment.
    fn superseded_questions(
        &self,
        pr: &PrRef,
        current: &Session,
    ) -> impl Future<Output = Result<Vec<SessionKey>>> + Send;
    /// Freeze the question comment of `superseded`: say a newer push
    /// superseded it, point to `current`'s question comment, and show
    /// `session`'s questions collapsed, without boxes (ADR-0004 D7).
    /// `session` is `None` when the superseded session is no longer stored.
    fn supersede_questions(
        &self,
        pr: &PrRef,
        superseded: &SessionKey,
        session: Option<&Session>,
        current: &Session,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Set the check status with a human-readable `summary`. The summary
    /// never mentions the learner's other identities (ADR-0003 D6).
    fn set_check(
        &self,
        pr: &PrRef,
        status: CheckStatus,
        summary: &str,
    ) -> impl Future<Output = Result<()>> + Send;
}

/// Source of time, so that scheduling is testable (D6, D12).
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

/// Asks a question at the terminal during a local hook (D9).
pub trait Prompter {
    fn ask(&mut self, question: &Question) -> Result<Answer>;
    /// Show the outcome to the author.
    fn report(&mut self, outcome: &SessionOutcome);
}
