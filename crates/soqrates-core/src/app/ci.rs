//! The CI check workflow (ADR-0001 D8).

use std::collections::BTreeSet;

use crate::app::engine::{ProfileLocation, SessionOutcome, Soqrates};
use crate::app::ports::{
    Clock, CodeSource, EarlierSessions, Examiner, Frozen, NoCodebase, ScmProvider, StateStore,
    Submission,
};
use crate::assessment::Question;
use crate::delivery::{
    CheckStatus, FeedbackCommand, Mode, PrRef, PrRounds, PullRequest, Round, Session,
};
use crate::ids::{AuthorId, ConceptId, SessionKey};
use crate::{Error, Result};

/// The check summary while answers are being graded (ADR-0004 D9).
pub const GRADING: &str = "Grading your answers…";

/// The session key for a PR at a given head commit. A new push gives a new key.
pub fn session_key(pr: &PullRequest) -> SessionKey {
    SessionKey(format!("{}{}", key_prefix(&pr.pr), pr.head_sha))
}

/// The start of every session key of `pr`, whatever its head.
fn key_prefix(pr: &PrRef) -> String {
    format!("{}#{}@", pr.repo, pr.number)
}

/// The codebase the session `key` of `pr` was asked about: the head commit
/// in its key, even after a newer push (ADR-0008 D4). `fallback` for a key
/// of another form.
fn codebase_of<K: CodeSource>(code: &K, pr: &PrRef, key: &SessionKey, fallback: &str) -> K::Code {
    let revision = key
        .0
        .strip_prefix(&key_prefix(pr))
        .filter(|head| !head.is_empty())
        .unwrap_or(fallback);
    code.at(revision)
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
    let pull = scm.pull_request(pr).await?;
    take_feedback(app, scm, pr, &pull.author).await?;
    let own_key = session_key(&pull);
    let mut rounds = app.rounds(pr, &pull.author).await?;
    // The session that answers for this head: its own, or one carried
    // forward from an earlier round (ADR-0002 D9).
    let key = rounds
        .for_head(&pull.head_sha)
        .map_or_else(|| own_key.clone(), |r| r.session.clone());
    if key == own_key
        && rounds.for_head(&pull.head_sha).is_none()
        && let Some(session) = app.pending_session(&key).await?
    {
        // A run posted the questions but failed to record the round.
        record_round(app, &mut rounds, &pull, &session).await?;
    }
    if let Some(status) = report(app, scm, code, pr, &pull, &key).await? {
        return Ok(status);
    }
    start_round(app, scm, code, pr, &pull, rounds).await
}

/// Record the PR author's feedback commands that haven't been answered yet,
/// and answer each once: that it was recorded, or why not (ADR-0011 D2).
/// Commands by anyone else never get here (D4).
async fn take_feedback<S, E, C, P>(
    app: &Soqrates<S, E, C>,
    scm: &P,
    pr: &PrRef,
    author: &AuthorId,
) -> Result<()>
where
    S: StateStore,
    E: Examiner,
    C: Clock,
    P: ScmProvider,
{
    for command in scm.feedback_commands(pr, author).await? {
        let note = feedback_note(app, pr, author, &command).await?;
        scm.answer_feedback(pr, &command, &note).await?;
    }
    Ok(())
}

/// What the author is told about `command`. Only a failure to reach a store
/// is an error; everything else is a note to the author.
async fn feedback_note<S, E, C>(
    app: &Soqrates<S, E, C>,
    pr: &PrRef,
    author: &AuthorId,
    command: &FeedbackCommand,
) -> Result<String>
where
    S: StateStore,
    E: Examiner,
    C: Clock,
{
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
        .filter(|k| k.0.starts_with(&key_prefix(pr)))
    else {
        return Ok(not_graded.into());
    };
    match app
        .give_feedback(Some(author), key, request, Some(&command.source))
        .await
    {
        Ok(given) => Ok(given.note()),
        Err(Error::NotFound(_)) => Ok(not_graded.into()),
        Err(Error::Invalid(why)) => Ok(format!("This feedback was not recorded: {why}.")),
        Err(e) => Err(e),
    }
}

/// Report on the session `key` if it exists: its result if it was graded,
/// otherwise grade the answers or keep waiting for them. `None` when there
/// is no such session.
async fn report<S, E, C, P, K>(
    app: &Soqrates<S, E, C>,
    scm: &P,
    code: &K,
    pr: &PrRef,
    pull: &PullRequest,
    key: &SessionKey,
) -> Result<Option<CheckStatus>>
where
    S: StateStore,
    E: Examiner,
    C: Clock,
    P: ScmProvider,
    K: CodeSource,
{
    // Already graded: report the result again, ask nothing.
    if let Some(status) = report_graded(app, scm, pull, key).await? {
        return Ok(Some(status));
    }
    let Some(session) = app.pending_session(key).await? else {
        // Looked up in the order a grading moves through: pending, being
        // graded, graded. So a grading by another run is never missed, and
        // no new session is opened for its key (ADR-0002 D6).
        if app.being_graded(key).await? {
            let summary = "The answers are being graded.";
            scm.set_check(pull, CheckStatus::Pending, summary).await?;
            return Ok(Some(CheckStatus::Pending));
        }
        return report_graded(app, scm, pull, key).await;
    };

    // Also on later runs, so that a run that failed to freeze the earlier
    // comments, or to grade an empty session, is made good by the next one.
    supersede_earlier(app, scm, pr, &session).await?;
    if session.questions().is_empty() {
        let outcome = app.submit(key, &[], &NoCodebase).await?;
        return finish(app, scm, pull, &outcome).await.map(Some);
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
            let codebase = codebase_of(code, pr, key, &pull.head_sha);
            let outcome = app.submit(key, &answers, &codebase).await?;
            scm.post_results(pr, &session, &outcome, &receipt).await?;
            finish(app, scm, pull, &outcome).await.map(Some)
        }
    }
}

/// Report the result of the session `key` again if it was graded.
async fn report_graded<S, E, C, P>(
    app: &Soqrates<S, E, C>,
    scm: &P,
    pull: &PullRequest,
    key: &SessionKey,
) -> Result<Option<CheckStatus>>
where
    S: StateStore,
    E: Examiner,
    C: Clock,
    P: ScmProvider,
{
    let Some(graded) = app.graded(key).await? else {
        return Ok(None);
    };
    let status = final_status(graded.passed, app.config.mode);
    let summary = format!(
        "{} of {} answers correct: {}.",
        graded.correct,
        graded.total,
        if graded.passed {
            "passed"
        } else {
            "not passed"
        }
    );
    scm.set_check(pull, status, &summary).await?;
    Ok(Some(status))
}

/// Ask a new round for the head of `pull` (ADR-0002 D9).
async fn start_round<S, E, C, P, K>(
    app: &Soqrates<S, E, C>,
    scm: &P,
    code: &K,
    pr: &PrRef,
    pull: &PullRequest,
    mut rounds: PrRounds,
) -> Result<CheckStatus>
where
    S: StateStore,
    E: Examiner,
    C: Clock,
    P: ScmProvider,
    K: CodeSource,
{
    let key = session_key(pull);
    // A round for this head whose session is gone, e.g. because posting it
    // failed, doesn't count.
    rounds.forget(&pull.head_sha);
    // Answers submitted before the push count: grade them first, so the
    // latest round is graded rather than carried (ADR-0002 D9).
    grade_submitted(app, scm, code, pr, &rounds).await?;
    // From the latest round that asked its own questions: a round that
    // carried an earlier session forward asked nothing about its head, e.g.
    // because carried questions filled the budget (ADR-0002 D10).
    let prefix = key_prefix(pr);
    let base = rounds
        .rounds()
        .iter()
        .rev()
        .find(|r| r.session.0 == format!("{prefix}{}", r.head));
    let diff = match base {
        // Only the files the PR changes: a merge of the base branch brings
        // in changes by others.
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
    let skip = already_asked(app, &rounds, &carried).await?;
    let codebase = codebase_of(code, pr, &key, &pull.head_sha);
    let questions = app
        .compose_questions(&pull.author, &diff, &skip, &carried, &codebase)
        .await?;

    // Nothing new: the latest round stands, rather than reposting the same
    // questions under a new comment.
    if questions.len() == carried.len()
        && let Some(prev) = rounds.latest()
    {
        let standing = prev.session.clone();
        if let Some(status) = report(app, scm, code, pr, pull, &standing).await? {
            // Nothing new: the latest round still stands (ADR-0002 D9).
            rounds.record(Round {
                head: pull.head_sha.clone(),
                session: standing,
                concepts: Vec::new(),
                at: app.clock.now(),
            });
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
    record_round(app, &mut rounds, pull, &session).await?;
    if session.questions().is_empty() {
        // Nothing to ask: grade the empty session right away.
        supersede_earlier(app, scm, pr, &session).await?;
        let outcome = app.submit(&key, &[], &NoCodebase).await?;
        return finish(app, scm, pull, &outcome).await;
    }
    let summary = format!(
        "Soqrates asked {} question(s) about this change. Answer them in the Soqrates comment.{}",
        session.questions().len(),
        location_note(&app.profile_location(&pull.author).await?)
    );
    scm.set_check(pull, CheckStatus::Pending, &summary).await?;
    supersede_earlier(app, scm, pr, &session).await?;
    Ok(CheckStatus::Pending)
}

/// Grade the latest round's session if it is still pending and its answers
/// were submitted, e.g. before a push that the check ran for first
/// (ADR-0002 D9, ADR-0004 D7). Its results are posted as for a run on its
/// own head, but no check is set: the new head's check is about its own
/// round. A session graded already is left alone (ADR-0002 D6).
async fn grade_submitted<S, E, C, P, K>(
    app: &Soqrates<S, E, C>,
    scm: &P,
    code: &K,
    pr: &PrRef,
    rounds: &PrRounds,
) -> Result<()>
where
    S: StateStore,
    E: Examiner,
    C: Clock,
    P: ScmProvider,
    K: CodeSource,
{
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
    let codebase = codebase_of(code, pr, session.key(), &prev.head);
    let outcome = app.submit(session.key(), &answers, &codebase).await?;
    scm.post_results(pr, &session, &outcome, &receipt).await
}

/// The concepts a later round on the PR doesn't ask again (ADR-0002 D9):
/// those of graded rounds, and the `carried` ones. Concepts of a round that
/// was superseded before it was graded were carried into the next round, so
/// they don't count on their own.
async fn already_asked<S, E, C>(
    app: &Soqrates<S, E, C>,
    rounds: &PrRounds,
    carried: &[Question],
) -> Result<BTreeSet<ConceptId>>
where
    S: StateStore,
    E: Examiner,
    C: Clock,
{
    let mut asked: BTreeSet<ConceptId> = carried.iter().map(|q| q.concept.clone()).collect();
    for round in rounds.rounds() {
        if app.graded(&round.session).await?.is_some() {
            asked.extend(round.concepts.iter().cloned());
        }
    }
    Ok(asked)
}

/// Record `session` as the round for the head of `pull`.
async fn record_round<S, E, C>(
    app: &Soqrates<S, E, C>,
    rounds: &mut PrRounds,
    pull: &PullRequest,
    session: &Session,
) -> Result<()>
where
    S: StateStore,
    E: Examiner,
    C: Clock,
{
    rounds.record(Round {
        head: pull.head_sha.clone(),
        session: session.key().clone(),
        concepts: session
            .questions()
            .iter()
            .map(|q| q.concept.clone())
            .collect(),
        at: session.created_at(),
    });
    app.save_rounds(&pull.author, rounds).await
}

/// Freeze the question comments of `pr` that `current` superseded and
/// discard their sessions, so answers to them can't be mistaken for answers
/// that count (ADR-0004 D7). Only sessions of this PR are touched, whatever
/// markers other comments carry.
async fn supersede_earlier<S, E, C, P>(
    app: &Soqrates<S, E, C>,
    scm: &P,
    pr: &PrRef,
    current: &Session,
) -> Result<()>
where
    S: StateStore,
    E: Examiner,
    C: Clock,
    P: ScmProvider,
{
    let sessions = PrSessions {
        app,
        prefix: key_prefix(pr),
        current: current.key(),
    };
    let Frozen { frozen, failure } = scm.freeze_superseded(pr, current, &sessions).await?;
    // Freeze first, discard after: a session whose comment could not be
    // frozen stays, and the next run tries again.
    for key in &frozen {
        app.discard_session(key).await?;
    }
    failure.map_or(Ok(()), Err)
}

/// The sessions of one pull request, for [`ScmProvider::freeze_superseded`].
struct PrSessions<'a, S, E, C> {
    app: &'a Soqrates<S, E, C>,
    prefix: String,
    current: &'a SessionKey,
}

impl<S, E, C> EarlierSessions for PrSessions<'_, S, E, C>
where
    S: StateStore,
    E: Examiner,
    C: Clock,
{
    fn may_freeze(&self, key: &SessionKey) -> bool {
        key != self.current && key.0.starts_with(&self.prefix)
    }

    async fn pending(&self, key: &SessionKey) -> Result<Option<Session>> {
        self.app.pending_session(key).await
    }
}

/// Report a graded session as the final check status (ADR-0001 D8). The
/// summary mentions no other identity of the learner (ADR-0003 D6).
async fn finish<S, E, C, P>(
    app: &Soqrates<S, E, C>,
    scm: &P,
    pull: &PullRequest,
    outcome: &SessionOutcome,
) -> Result<CheckStatus>
where
    S: StateStore,
    E: Examiner,
    C: Clock,
    P: ScmProvider,
{
    let status = final_status(outcome.passed, app.config.mode);
    let correct = outcome
        .grades
        .iter()
        .filter(|(_, g)| g.is_correct())
        .count();
    let mut summary = format!(
        "{correct} of {} answers correct: {}.",
        outcome.grades.len(),
        if outcome.passed {
            "passed"
        } else {
            "not passed"
        }
    );
    summary.push_str(&location_note(&outcome.profile_location));
    scm.set_check(pull, status, &summary).await?;
    Ok(status)
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
