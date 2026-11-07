//! Ports implemented by adapter crates (ADR-0001 D2).
//!
//! The async methods return `impl Future + Send`, so implementers can simply
//! write `async fn` in their impls.

use std::future::Future;

use chrono::{DateTime, Utc};

use crate::Result;
use crate::app::engine::SessionOutcome;
use crate::assessment::{Answer, Concept, Diff, Grade, Question, QuestionFormat};
use crate::delivery::{CheckStatus, GradedSession, PrRef, PullRequest, Session};
use crate::ids::{Identity, LearnerId, SessionKey};
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

    /// Record that a session was graded (ADR-0002 D6).
    fn save_graded(&self, graded: &GradedSession) -> impl Future<Output = Result<()>> + Send;
    fn load_graded(
        &self,
        key: &SessionKey,
    ) -> impl Future<Output = Result<Option<GradedSession>>> + Send;
    /// Undo [`StateStore::save_graded`] when grading is rolled back.
    fn delete_graded(&self, key: &SessionKey) -> impl Future<Output = Result<()>> + Send;
}

/// The store holding the profiles of linked learners, reachable from several
/// forges and owned by the learner or by an organisation (ADR-0003 D4). It
/// stores only general concepts (D5). A port failure means "unreachable".
pub trait LearnerStore: StateStore {}

/// The LLM-backed part of Soqrates: understands diffs, writes and grades
/// questions (D3). Implemented by `soqrates-examiner` on top of Rig.
pub trait Examiner: Send + Sync {
    fn extract_concepts(&self, diff: &Diff) -> impl Future<Output = Result<Vec<Concept>>> + Send;
    fn write_question(
        &self,
        concept: &Concept,
        format: QuestionFormat,
    ) -> impl Future<Output = Result<Question>> + Send;
    fn grade(
        &self,
        question: &Question,
        answer: &Answer,
    ) -> impl Future<Output = Result<Grade>> + Send;
}

/// A forge/CI provider (D4). Implemented by `soqrates-scm-github` and friends.
pub trait ScmProvider: Send + Sync {
    fn pull_request(&self, pr: &PrRef) -> impl Future<Output = Result<PullRequest>> + Send;
    /// Post the session's questions on the PR, e.g. as a comment.
    fn post_questions(
        &self,
        pr: &PrRef,
        session: &Session,
    ) -> impl Future<Output = Result<()>> + Send;
    /// The author's answers to `session`, if they have replied yet.
    fn fetch_answers(
        &self,
        pr: &PrRef,
        session: &Session,
    ) -> impl Future<Output = Result<Option<Vec<Answer>>>> + Send;
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
