//! The CI check workflow (ADR-0001 D8).

use std::collections::BTreeSet;

use crate::app::engine::{ProfileLocation, SessionOutcome, SessionState, Soqrates};
use crate::app::events::DomainEvent;
use crate::app::ports::{
    Clock, CodeSource, Codebase, EarlierSessions, Examiner, Frozen, NoCodebase, ScmProvider,
    StateStore, Submission,
};
use crate::assessment::Question;
use crate::delivery::{
    CheckStatus, FeedbackCommand, Mode, PrRef, PrRounds, PullRequest, Round, Session, SessionEvent,
};
use crate::ids::{AuthorId, ConceptId, SessionKey};
use crate::{Error, Result};

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
/// - Answers present: grade them. Wrong answers get a follow-up round on
///   the same commit, and the check stays `Pending` (ADR-0010). Otherwise
///   set `Success`, or `Failure` (`Neutral` in `Advisory` mode), from the
///   result after the final round.
///
/// First of all, the author's new feedback commands are recorded and
/// answered (ADR-0011 D2).
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
        self.take_feedback(pr, &pull.author).await?;
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
        // The latest round of that session: a follow-up, if any (ADR-0010 D2).
        let key = self.app.latest_follow_up(&key).await?;
        if let Some(status) = self.report(pr, &pull, &key).await? {
            return Ok(status);
        }
        self.start_round(pr, &pull, rounds).await
    }

    /// Record the PR author's feedback commands that haven't been answered
    /// yet, and answer each once: that it was recorded, or why not
    /// (ADR-0011 D2). Commands by anyone else never get here (D4).
    async fn take_feedback(&self, pr: &PrRef, author: &AuthorId) -> Result<()> {
        for command in self.scm.feedback_commands(pr, author).await? {
            let note = self.feedback_note(pr, author, &command).await?;
            self.scm.answer_feedback(pr, &command, &note).await?;
        }
        Ok(())
    }

    /// What the author is told about `command`. Only a failure to reach a
    /// store is an error; everything else is a note to the author.
    async fn feedback_note(
        &self,
        pr: &PrRef,
        author: &AuthorId,
        command: &FeedbackCommand,
    ) -> Result<String> {
        let request = match &command.request {
            Ok(request) => request,
            Err(why) => return Ok(why.clone()),
        };
        // Only results of this pull request's own sessions.
        let not_graded = "There are no graded results for this feedback to be about yet: \
                          feedback is on the questions of a results comment.";
        let Some(key) = command
            .session
            .as_ref()
            .filter(|k| k.is_of_pr(&pr.repo, pr.number))
        else {
            return Ok(not_graded.into());
        };
        match self
            .app
            .give_feedback(Some(author), key, request, Some(&command.source))
            .await
        {
            Ok(given) => Ok(given.note()),
            Err(Error::NotFound(_)) => Ok(not_graded.into()),
            Err(Error::Invalid(why)) => Ok(format!("This feedback was not recorded: {why}.")),
            Err(e) => Err(e),
        }
    }

    /// The codebase the session `key` of `pr` was asked about: the head
    /// commit in its key, even after a newer push (ADR-0008 D4), also for a
    /// follow-up round (ADR-0010 D2). `fallback` for a key of another form.
    fn codebase_of(&self, pr: &PrRef, key: &SessionKey, fallback: &str) -> K::Code {
        let (first, _) = key.round();
        self.code
            .at(first.pr_head(&pr.repo, pr.number).unwrap_or(fallback))
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
                let summary = result_summary(graded.correct, graded.total, graded.passed, key);
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
                if let Some(status) = self
                    .ask_follow_up(pull, &session, &outcome, &codebase)
                    .await?
                {
                    return Ok(Some(status));
                }
                self.finish(pull, &outcome).await.map(Some)
            }
        }
    }

    /// Ask the follow-up round after `session`, graded with `outcome`, if it
    /// has one: post its questions and keep the check `Pending` (ADR-0010
    /// D1, D4). `None` when there is none to ask. If the follow-up can't be
    /// posted, its session is discarded and the graded result stands.
    async fn ask_follow_up(
        &self,
        pull: &PullRequest,
        session: &Session,
        outcome: &SessionOutcome,
        code: &impl Codebase,
    ) -> Result<Option<CheckStatus>> {
        let (app, scm) = (self.app, self.scm);
        let Some(follow_up) = app.follow_up(session, outcome, code).await? else {
            return Ok(None);
        };
        if let Err(e) = scm.publish_questions(&pull.pr, &follow_up).await {
            let _ = app.discard_session(follow_up.key()).await;
            return Err(e);
        }
        let summary = format!(
            "{} answer(s) were wrong: Soqrates asked a follow-up question about each. Answer \
             them in the new Soqrates comment.{}",
            follow_up.questions().len(),
            location_note(&outcome.profile_location)
        );
        scm.set_check(pull, CheckStatus::Pending, &summary).await?;
        Ok(Some(CheckStatus::Pending))
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
        // latest round is graded rather than carried (ADR-0002 D9); also
        // those to a follow-up round, which then asks no further one
        // (ADR-0010 D6).
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
            // Its latest round: a follow-up stays open too (ADR-0010 D6).
            let latest = app.latest_follow_up(&standing).await?;
            if let Some(status) = self.report(pr, pull, &latest).await? {
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
        let latest = app.latest_follow_up(&prev.session).await?;
        let Some(session) = app.pending_session(&latest).await? else {
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
        // For a follow-up, about every concept of its first round
        // (ADR-0010 D4).
        let graded = outcome.events.iter().find_map(|e| match e {
            DomainEvent::Session(SessionEvent::SessionGraded {
                key,
                correct,
                total,
                ..
            }) => Some((key, *correct, *total)),
            _ => None,
        });
        let mut summary = match graded {
            Some((key, correct, total)) => result_summary(correct, total, outcome.passed, key),
            None => result_summary(
                outcome.correct(),
                outcome.grades.len(),
                outcome.passed,
                &SessionKey(String::new()),
            ),
        };
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

/// The check summary of the session `key`, graded with `correct` of `total`
/// answers right. A pass in a follow-up round says so, so that the first
/// attempt stays visible (ADR-0010 D4).
fn result_summary(correct: usize, total: usize, passed: bool, key: &SessionKey) -> String {
    let verdict = match (passed, key.round().1 > 1) {
        (true, true) => "passed after a follow-up",
        (true, false) => "passed",
        (false, _) => "not passed",
    };
    format!("{correct} of {total} answers correct: {verdict}.")
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
