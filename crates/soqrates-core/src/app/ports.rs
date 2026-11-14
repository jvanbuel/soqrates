//! Ports implemented by adapter crates (ADR-0001 D2).
//!
//! The async methods return `impl Future + Send`, so implementers can simply
//! write `async fn` in their impls.

use std::future::Future;

use chrono::{DateTime, Utc};

use crate::app::engine::SessionOutcome;
use crate::assessment::{Answer, Concept, Diff, Grading, Question, QuestionBrief};
use crate::delivery::{
    BankedQuestion, CheckStatus, GradedSession, GradingJournal, PrRef, PrRounds, PullRequest,
    SeenQuestions, Session,
};
use crate::ids::{ConceptId, Identity, LearnerId, SessionKey};
use crate::learner::{IdentityLink, LearnerProfile, LinkCode};
use crate::{Error, Result};

/// The kinds of record a [`DocumentStore`] holds. Each is a collection of
/// JSON documents addressed by a key that is unique within the kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    /// Learner profiles, keyed by learner.
    Profile,
    /// Identity links, keyed by identity; grouped by learner.
    Link,
    /// Link codes, keyed by code.
    LinkCode,
    /// Pending sessions, keyed by session key.
    Session,
    /// Grading journals of sessions being graded, keyed by session key.
    Grading,
    /// Graded-session records, keyed by session key.
    Graded,
    /// The rounds asked on a pull request, keyed by its id.
    Rounds,
    /// Bank questions, keyed by bank id; grouped by concept.
    Bank,
    /// What a learner was shown from the bank, keyed by learner.
    Seen,
}

impl Kind {
    pub const ALL: [Kind; 9] = [
        Kind::Profile,
        Kind::Link,
        Kind::LinkCode,
        Kind::Session,
        Kind::Grading,
        Kind::Graded,
        Kind::Rounds,
        Kind::Bank,
        Kind::Seen,
    ];

    /// What a record of this kind is called in messages.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Profile => "profile",
            Kind::Link => "link",
            Kind::LinkCode => "link code",
            Kind::Session => "session",
            Kind::Grading => "grading journal",
            Kind::Graded => "graded session",
            Kind::Rounds => "rounds",
            Kind::Bank => "bank question",
            Kind::Seen => "seen questions",
        }
    }

    /// Where in a document of this kind its group is (a JSON pointer);
    /// `None` for kinds that are not grouped. Backends that index the
    /// group use [`Kind::group_of`] to read it.
    pub fn group_pointer(self) -> Option<&'static str> {
        match self {
            Kind::Link => Some("/learner"),
            Kind::Bank => Some("/question/concept"),
            _ => None,
        }
    }

    /// The group `doc` belongs to: the learner of a link, the concept of a
    /// bank question. `None` for kinds that are not grouped; an error for
    /// a document of a grouped kind that has no group.
    pub fn group_of(self, doc: &Doc) -> Result<Option<String>> {
        let Some(pointer) = self.group_pointer() else {
            return Ok(None);
        };
        doc.pointer(pointer)
            .and_then(|g| g.as_str())
            .map(|g| Some(g.to_string()))
            .ok_or_else(|| Error::Port(format!("corrupt {}: no group at {pointer}", self.label())))
    }
}

/// A stored document: the JSON the domain type serialises to.
pub type Doc = serde_json::Value;

/// What [`DocumentStore::remove`] removes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Removal<'a> {
    /// The document of `kind` with this key, if there is one.
    Key(Kind, &'a str),
    /// Every document of `kind` in this group (see [`Kind::group_of`]); none
    /// for a kind that is not grouped.
    Group(Kind, &'a str),
}

/// The storage port that backends implement (ADR-0001 D5): JSON documents
/// by kind and key, with the two atomic steps the engine relies on. Records
/// are typed, serialised and queried by the [`StateStore`] implemented once
/// on top of it, so a backend knows nothing about learners or sessions.
///
/// Every method is atomic for the one document it names, and a backend that
/// is shared between processes keeps that between processes. Failures are
/// [`Error::Port`].
pub trait DocumentStore: Send + Sync {
    /// The document of `kind` with `key`, if any.
    fn load(&self, kind: Kind, key: &str) -> impl Future<Output = Result<Option<Doc>>> + Send;

    /// Create or replace the document of `kind` with `key`. Atomic: a
    /// reader sees the old or the new document, never a mix, and a failure
    /// leaves the old one. For a grouped kind the group is
    /// [`Kind::group_of`] `doc`, replaced along with it.
    fn save(&self, kind: Kind, key: &str, doc: &Doc) -> impl Future<Output = Result<()>> + Send;

    /// The documents of `kind`, or only those of `group` (see
    /// [`Kind::group_of`]; a kind that is not grouped has none), in any
    /// order. A snapshot per document, not across documents.
    fn list(
        &self,
        kind: Kind,
        group: Option<&str>,
    ) -> impl Future<Output = Result<Vec<Doc>>> + Send;

    /// Remove what `removals` name; what isn't there is skipped. All or
    /// nothing where the backend has transactions (memory, SQL); one by one,
    /// in order, in the file store, where a failure part-way leaves the
    /// rest.
    fn remove(&self, removals: &[Removal<'_>]) -> impl Future<Output = Result<()>> + Send;

    /// Compare and swap: replace the document of `kind` with `key` by `new`
    /// only if it is still `expected` (`None`: there is none), as one
    /// step. Returns `false`, changing nothing, when it is not. Of several
    /// concurrent swaps from one `expected`, exactly one returns `true`.
    fn swap(
        &self,
        kind: Kind,
        key: &str,
        expected: Option<&Doc>,
        new: &Doc,
    ) -> impl Future<Output = Result<bool>> + Send;

    /// Claim the document of kind `from` with `key`: remove it and store
    /// `doc` as the document of kind `to` with the same key, as one step.
    /// Returns `false`, changing nothing, when there is no such `from`
    /// document or a `to` document with that key already exists. Of several
    /// concurrent claims, at most one returns `true`. A backend without
    /// transactions writes `to` before it removes `from`, so a stop in
    /// between leaves both, never neither.
    fn claim(
        &self,
        from: Kind,
        to: Kind,
        key: &str,
        doc: &Doc,
    ) -> impl Future<Output = Result<bool>> + Send;
}

/// Persists the records of Soqrates: learner profiles, identity links, link
/// codes, sessions, grading journals, graded records, PR rounds and the
/// question bank (ADR-0001 D5, ADR-0003). The engine and every other caller
/// use this port; it is implemented once, for every [`DocumentStore`].
pub trait StateStore: Send + Sync {
    fn load_profile(
        &self,
        learner: &LearnerId,
    ) -> impl Future<Output = Result<Option<LearnerProfile>>> + Send;
    fn save_profile(&self, profile: &LearnerProfile) -> impl Future<Output = Result<()>> + Send;
    /// Save `profile` only if the stored profile of its learner is still the
    /// version `profile` was loaded at (`profile.version()`; a learner
    /// without a profile counts as version 0). The stored profile is then
    /// `profile` one version later. Returns `false`, saving nothing, when
    /// someone saved in between: the caller loads again and retries
    /// (ADR-0002 D6). Check and save are one step.
    fn save_profile_if_current(
        &self,
        profile: &LearnerProfile,
    ) -> impl Future<Output = Result<bool>> + Send;
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
