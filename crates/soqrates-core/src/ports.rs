//! Ports implemented by adapter crates (ADR-0001 D2).
//!
//! The async methods return `impl Future + Send`, so implementers can simply
//! write `async fn` in their impls.

use std::future::Future;

use chrono::{DateTime, Utc};

use crate::Result;
use crate::model::*;

/// Persists learner profiles and pending sessions (D5).
pub trait StateStore: Send + Sync {
    fn load_profile(
        &self,
        author: &AuthorId,
    ) -> impl Future<Output = Result<Option<LearnerProfile>>> + Send;
    fn save_profile(&self, profile: &LearnerProfile) -> impl Future<Output = Result<()>> + Send;
    fn load_session(
        &self,
        key: &SessionKey,
    ) -> impl Future<Output = Result<Option<Session>>> + Send;
    fn save_session(&self, session: &Session) -> impl Future<Output = Result<()>> + Send;
    fn delete_session(&self, key: &SessionKey) -> impl Future<Output = Result<()>> + Send;
}

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
    fn set_check(&self, pr: &PrRef, status: CheckStatus)
    -> impl Future<Output = Result<()>> + Send;
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
