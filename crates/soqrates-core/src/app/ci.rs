//! The CI check workflow (ADR-0001 D8).

use std::collections::BTreeSet;

use crate::Result;
use crate::app::engine::{ProfileLocation, SessionOutcome, SessionState, Soqrates};
use crate::app::ports::{
    Clock, CodeSource, EarlierSessions, Examiner, Frozen, NoCodebase, ScmProvider, StateStore,
    Submission,
};
use crate::assessment::Question;
use crate::delivery::{CheckStatus, Mode, PrRef, PrRounds, PullRequest, Round, Session};
use crate::ids::{ConceptId, SessionKey};

/// The check summary while answers are being graded (ADR-0004 D9).
pub const GRADING: &str = "Grading your answers…";

/// The session key for a PR at a given head commit. A new push gives a new key.
pub fn session_key(pr: &PullRequest) -> SessionKey {
    SessionKey::pr(&pr.pr.repo, pr.pr.number, &pr.head_sha)
}

/// One run of the CI check. Call it on PR open/push and when the author
/// replies.
///
/// - No round for the current head yet: grade the latest round first if
///   its answers were submitted, setting no check. Then ask again whatever
///   the latest round left unanswered, then about what changed since the latest round (the
///   whole PR on its first round, or after a force-push), leaving out
///   concepts already asked on the PR unless they are due (ADR-0002 D9,
///   D10). Post the questions, freeze the question comments of earlier
///   heads that were never graded (ADR-0004 D7) and set `Pending`.
/// - Nothing new to ask on a later round: carry the latest round's session
///   forward. Its questions stay open, or its result is reported again.
/// - Nothing to ask on the first round: the comment says so and the empty
///   session is graded right away.
/// - Session exists but no answers yet: keep `Pending`.
/// - Answers being graded, e.g. by an overlapping run: keep `Pending`
///   (ADR-0002 D6).
/// - Answers present: grade them and set `Success`, or `Failure` (`Neutral`
///   in `Advisory` mode).
///
/// The examiner reads the pull request's code from `code`, at the commit
/// each session was asked about (ADR-0008 D4).
///
/// Returns the status that was set.
pub async fn run_check<S, E, C, P, K>(
    app: &Soqrates<S, E, C>,
    scm: &P,
    code: &K,
    pr: &PrRef,
) -> Result<CheckStatus>
where
    S: StateStore,
    E: Examiner,
    C: Clock,
    P: ScmProvider,
    K: CodeSource,
{
    CiCheck { app, scm, code }.run(pr).await
}

/// What one run of the check works with: the app, the forge and the code
/// source. The workflow steps are its methods, so the bounds are written
/// once.
struct CiCheck<'a, S, E, C, P, K> {
    app: &'a Soqrates<S, E, C>,
    scm: &'a P,
    code: &'a K,
}

impl<S, E, C, P, K> CiCheck<'_, S, E, C, P, K>
where
    S: StateStore,
    E: Examiner,
    C: Clock,
    P: ScmProvider,
    K: CodeSource,
{
    async fn run(&self, pr: &PrRef) -> Result<CheckStatus> {
        let pull = self.scm.pull_request(pr).await?;
        let own_key = session_key(&pull);
        let mut rounds = self.app.rounds(pr, &pull.author).await?;
        // The session that answers for this head: its own, or one carried
        // forward from an earlier round (ADR-0002 D9).
        let key = rounds
            .for_head(&pull.head_sha)
            .map_or_else(|| own_key.clone(), |r| r.session.clone());
        if key == own_key
            && rounds.for_head(&pull.head_sha).is_none()
            && let Some(session) = self.app.pending_session(&key).await?
        {
            // A run posted the questions but failed to record the round.
            self.record_round(&mut rounds, &pull, &session).await?;
        }
        if let Some(status) = self.report(pr, &pull, &key).await? {
            return Ok(status);
        }
        self.start_round(pr, &pull, rounds).await
    }

    /// The codebase the session `key` of `pr` was asked about: the head
    /// commit in its key, even after a newer push (ADR-0008 D4). `fallback`
    /// for a key of another form.
    fn codebase_of(&self, pr: &PrRef, key: &SessionKey, fallback: &str) -> K::Code {
        self.code
            .at(key.pr_head(&pr.repo, pr.number).unwrap_or(fallback))
    }

    /// Report on the session `key` if it exists: its result if it was
    /// graded, otherwise grade the answers or keep waiting for them. `None`
    /// when there is no such session.
    async fn report(
        &self,
        pr: &PrRef,
        pull: &PullRequest,
        key: &SessionKey,
    ) -> Result<Option<CheckStatus>> {
        let (app, scm) = (self.app, self.scm);
        let session = match app.session_state(key).await? {
            SessionState::Absent => return Ok(None),
            SessionState::Graded(graded) => {
                // Already graded: report the result again, ask nothing.
                let status = final_status(graded.passed, app.config.mode);
                let summary = result_summary(graded.correct, graded.total, graded.passed);
                scm.set_check(pull, status, &summary).await?;
                return Ok(Some(status));
            }
            SessionState::Grading => {
                // No new session is opened for its key (ADR-0002 D6).
                let summary = "The answers are being graded.";
                scm.set_check(pull, CheckStatus::Pending, summary).await?;
                return Ok(Some(CheckStatus::Pending));
            }
            SessionState::Pending(session) => session,
        };

        // Also on later runs, so that a run that failed to freeze the earlier
        // comments, or to grade an empty session, is made good by the next one.
        self.supersede_earlier(pr, &session).await?;
        if session.questions().is_empty() {
            let outcome = app.submit(key, &[], &NoCodebase).await?;
            return self.finish(pull, &outcome).await.map(Some);
        }
        match scm.collect_answers(pr, &session).await? {
            None => {
                let summary = format!(
                    "Waiting for answers.{}",
                    location_note(&app.profile_location(&pull.author).await?)
                );
                scm.set_check(pull, CheckStatus::Pending, &summary).await?;
                Ok(Some(CheckStatus::Pending))
            }
            Some(Submission { answers, receipt }) => {
                // Grading takes a while: show the author it has started
                // (ADR-0004 D9).
                let receipt = scm.acknowledge_answers(pr, &session, receipt).await?;
                scm.set_check(pull, CheckStatus::Pending, GRADING).await?;
                let codebase = self.codebase_of(pr, key, &pull.head_sha);
                let outcome = app.submit(key, &answers, &codebase).await?;
                scm.post_results(pr, &session, &outcome, &receipt).await?;
                self.finish(pull, &outcome).await.map(Some)
            }
        }
    }

    /// Ask a new round for the head of `pull` (ADR-0002 D9).
    async fn start_round(
        &self,
        pr: &PrRef,
        pull: &PullRequest,
        mut rounds: PrRounds,
    ) -> Result<CheckStatus> {
        let (app, scm) = (self.app, self.scm);
        let key = session_key(pull);
        // A round for this head whose session is gone, e.g. because posting
        // it failed, doesn't count.
        rounds.forget(&pull.head_sha);
        // Answers submitted before the push count: grade them first, so the
        // latest round is graded rather than carried (ADR-0002 D9).
        self.grade_submitted(pr, &rounds).await?;
        // From the latest round that asked its own questions: a round that
        // carried an earlier session forward asked nothing about its head,
        // e.g. because carried questions filled the budget (ADR-0002 D10).
        let base = rounds.rounds().iter().rev().find(|r| !r.carried);
        let diff = match base {
            // Only the files the PR changes: a merge of the base branch
            // brings in changes by others.
            Some(prev) => scm
                .diff_between(pr, &prev.head, &pull.head_sha)
                .await?
                .map(|d| d.restricted_to_files_of(&pull.diff)),
            None => None,
        }
        .unwrap_or_else(|| pull.diff.clone());
        // The latest round's unanswered questions are asked again, first
        // (ADR-0002 D9, D10).
        let carried: Vec<Question> = match rounds.latest() {
            Some(prev) => app
                .pending_session(&prev.session)
                .await?
                .map(|s| s.questions().to_vec())
                .unwrap_or_default(),
            None => Vec::new(),
        };
        let skip = self.already_asked(&rounds, &carried).await?;
        let codebase = self.codebase_of(pr, &key, &pull.head_sha);
        let questions = app
            .compose_questions(&pull.author, &diff, &skip, &carried, &codebase)
            .await?;

        // Nothing new: the latest round stands, rather than reposting the
        // same questions under a new comment.
        if questions.len() == carried.len()
            && let Some(prev) = rounds.latest()
        {
            let standing = prev.session.clone();
            if let Some(status) = self.report(pr, pull, &standing).await? {
                // Nothing new: the latest round still stands (ADR-0002 D9).
                rounds.record(Round::carrying(
                    pull.head_sha.clone(),
                    standing,
                    app.clock.now(),
                ));
                app.save_rounds(&pull.author, &rounds).await?;
                return Ok(status);
            }
        }

        let session = app
            .open_session(key.clone(), &pull.author, questions, carried.len())
            .await?;
        // Also when there is nothing to ask: the comment then says so, and
        // earlier question comments can point to it (ADR-0004 D7).
        if let Err(e) = scm.publish_questions(pr, &session).await {
            // Don't leave a session whose questions nobody can see.
            let _ = app.discard_session(&key).await;
            return Err(e);
        }
        self.record_round(&mut rounds, pull, &session).await?;
        if session.questions().is_empty() {
            // Nothing to ask: grade the empty session right away.
            self.supersede_earlier(pr, &session).await?;
            let outcome = app.submit(&key, &[], &NoCodebase).await?;
            return self.finish(pull, &outcome).await;
        }
        let summary = format!(
            "Soqrates asked {} question(s) about this change. Answer them in the Soqrates comment.{}",
            session.questions().len(),
            location_note(&app.profile_location(&pull.author).await?)
        );
        scm.set_check(pull, CheckStatus::Pending, &summary).await?;
        self.supersede_earlier(pr, &session).await?;
        Ok(CheckStatus::Pending)
    }

    /// Grade the latest round's session if it is still pending and its
    /// answers were submitted, e.g. before a push that the check ran for
    /// first (ADR-0002 D9, ADR-0004 D7). Its results are posted as for a run
    /// on its own head, but no check is set: the new head's check is about
    /// its own round. A session graded already is left alone (ADR-0002 D6).
    async fn grade_submitted(&self, pr: &PrRef, rounds: &PrRounds) -> Result<()> {
        let (app, scm) = (self.app, self.scm);
        let Some(prev) = rounds.latest() else {
            return Ok(());
        };
        let Some(session) = app.pending_session(&prev.session).await? else {
            return Ok(());
        };
        if session.questions().is_empty() {
            return Ok(());
        }
        let Some(Submission { answers, receipt }) = scm.collect_answers(pr, &session).await? else {
            return Ok(());
        };
        // Acknowledged like any pickup (ADR-0004 D9), but the status is left
        // to the new head's round.
        let receipt = scm.acknowledge_answers(pr, &session, receipt).await?;
        let codebase = self.codebase_of(pr, session.key(), &prev.head);
        let outcome = app.submit(session.key(), &answers, &codebase).await?;
        scm.post_results(pr, &session, &outcome, &receipt).await
    }

    /// The concepts a later round on the PR doesn't ask again (ADR-0002 D9):
    /// those of graded rounds, and the `carried` ones. Concepts of a round
    /// that was superseded before it was graded were carried into the next
    /// round, so they don't count on their own.
    async fn already_asked(
        &self,
        rounds: &PrRounds,
        carried: &[Question],
    ) -> Result<BTreeSet<ConceptId>> {
        let mut asked: BTreeSet<ConceptId> = carried.iter().map(|q| q.concept.clone()).collect();
        for round in rounds.rounds() {
            if self.app.graded(&round.session).await?.is_some() {
                asked.extend(round.concepts.iter().cloned());
            }
        }
        Ok(asked)
    }

    /// Record `session` as the round for the head of `pull`.
    async fn record_round(
        &self,
        rounds: &mut PrRounds,
        pull: &PullRequest,
        session: &Session,
    ) -> Result<()> {
        rounds.record(Round::own(
            pull.head_sha.clone(),
            session.key().clone(),
            session
                .questions()
                .iter()
                .map(|q| q.concept.clone())
                .collect(),
            session.created_at(),
        ));
        self.app.save_rounds(&pull.author, rounds).await
    }

    /// Freeze the question comments of `pr` that `current` superseded and
    /// discard their sessions, so answers to them can't be mistaken for
    /// answers that count (ADR-0004 D7). Only sessions of this PR are
    /// touched, whatever markers other comments carry.
    async fn supersede_earlier(&self, pr: &PrRef, current: &Session) -> Result<()> {
        let sessions = PrSessions {
            app: self.app,
            pr,
            current: current.key(),
        };
        let Frozen { frozen, failure } = self.scm.freeze_superseded(pr, current, &sessions).await?;
        // Freeze first, discard after: a session whose comment could not be
        // frozen stays, and the next run tries again.
        for key in &frozen {
            self.app.discard_session(key).await?;
        }
        failure.map_or(Ok(()), Err)
    }

    /// Report a graded session as the final check status (ADR-0001 D8). The
    /// summary mentions no other identity of the learner (ADR-0003 D6).
    async fn finish(&self, pull: &PullRequest, outcome: &SessionOutcome) -> Result<CheckStatus> {
        let status = final_status(outcome.passed, self.app.config.mode);
        let correct = outcome
            .grades
            .iter()
            .filter(|(_, g)| g.is_correct())
            .count();
        let mut summary = result_summary(correct, outcome.grades.len(), outcome.passed);
        summary.push_str(&location_note(&outcome.profile_location));
        self.scm.set_check(pull, status, &summary).await?;
        Ok(status)
    }
}

/// The sessions of one pull request, for [`ScmProvider::freeze_superseded`].
struct PrSessions<'a, S, E, C> {
    app: &'a Soqrates<S, E, C>,
    pr: &'a PrRef,
    current: &'a SessionKey,
}

impl<S, E, C> EarlierSessions for PrSessions<'_, S, E, C>
where
    S: StateStore,
    E: Examiner,
    C: Clock,
{
    fn may_freeze(&self, key: &SessionKey) -> bool {
        key != self.current && key.is_of_pr(&self.pr.repo, self.pr.number)
    }

    async fn pending(&self, key: &SessionKey) -> Result<Option<Session>> {
        self.app.pending_session(key).await
    }
}

/// "N of M answers correct: passed.", for a fresh result and a repeated one.
fn result_summary(correct: usize, total: usize, passed: bool) -> String {
    format!(
        "{correct} of {total} answers correct: {}.",
        if passed { "passed" } else { "not passed" }
    )
}

/// The status of a graded session (ADR-0001 D8).
fn final_status(passed: bool, mode: Mode) -> CheckStatus {
    match (passed, mode) {
        (true, _) => CheckStatus::Success,
        (false, Mode::Blocking) => CheckStatus::Failure,
        (false, Mode::Advisory) => CheckStatus::Neutral,
    }
}

/// Tells the learner when the learner store was unreachable (ADR-0003 D4).
fn location_note(location: &ProfileLocation) -> String {
    match location {
        ProfileLocation::LearnerStore => String::new(),
        ProfileLocation::Local { host } => {
            format!(" The learner store was unreachable; this learner profile is local to {host}.")
        }
    }
}
