//! The CI check workflow (ADR-0001 D8).

use std::collections::BTreeSet;

use crate::Result;
use crate::app::engine::{ProfileLocation, SessionOutcome, Soqrates};
use crate::app::events::DomainEvent;
use crate::app::ports::{
    Clock, CodeSource, Codebase, Examiner, NoCodebase, ScmProvider, StateStore,
};
use crate::assessment::Question;
use crate::delivery::{
    CheckStatus, Mode, PrRef, PrRounds, PullRequest, Round, Session, SessionEvent,
};
use crate::ids::{ConceptId, SessionKey};

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
/// in its key, even after a newer push (ADR-0008 D4), also for a follow-up
/// round (ADR-0010 D2). `fallback` for a key of another form.
fn codebase_of<K: CodeSource>(code: &K, pr: &PrRef, key: &SessionKey, fallback: &str) -> K::Code {
    let (first, _) = key.round();
    let revision = first
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
/// - Answers present: grade them. Wrong answers get a follow-up round on
///   the same commit, and the check stays `Pending` (ADR-0010). Otherwise
///   set `Success`, or `Failure` (`Neutral` in `Advisory` mode), from the
///   result after the final round.
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
    // The latest round of that session: a follow-up, if any (ADR-0010 D2).
    let key = app.latest_follow_up(&key).await?;
    if let Some(status) = report(app, scm, code, pr, &pull, &key).await? {
        return Ok(status);
    }
    start_round(app, scm, code, pr, &pull, rounds).await
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
    if let Some(status) = report_graded(app, scm, pr, key).await? {
        return Ok(Some(status));
    }
    let Some(session) = app.pending_session(key).await? else {
        // Looked up in the order a grading moves through: pending, being
        // graded, graded. So a grading by another run is never missed, and
        // no new session is opened for its key (ADR-0002 D6).
        if app.being_graded(key).await? {
            let summary = "The answers are being graded.";
            scm.set_check(pr, CheckStatus::Pending, summary).await?;
            return Ok(Some(CheckStatus::Pending));
        }
        return report_graded(app, scm, pr, key).await;
    };

    // Also on later runs, so that a run that failed to freeze the earlier
    // comments, or to grade an empty session, is made good by the next one.
    supersede_earlier(app, scm, pr, &session).await?;
    if session.questions().is_empty() {
        let outcome = app.submit(key, &[], &NoCodebase).await?;
        return finish(app, scm, pr, &outcome).await.map(Some);
    }
    scm.hint_commands(pr, &session).await?;
    match scm.fetch_answers(pr, &session).await? {
        None => {
            let summary = format!(
                "Waiting for answers.{}",
                location_note(&app.profile_location(&pull.author).await?)
            );
            scm.set_check(pr, CheckStatus::Pending, &summary).await?;
            Ok(Some(CheckStatus::Pending))
        }
        Some(answers) => {
            // Grading takes a while: show the author it has started
            // (ADR-0004 D9).
            scm.acknowledge_answers(pr, &session).await?;
            scm.set_check(pr, CheckStatus::Pending, GRADING).await?;
            let codebase = codebase_of(code, pr, key, &pull.head_sha);
            let outcome = app.submit(key, &answers, &codebase).await?;
            scm.post_results(pr, &session, &outcome).await?;
            if let Some(status) = ask_follow_up(app, scm, pr, &session, &outcome, &codebase).await?
            {
                return Ok(Some(status));
            }
            finish(app, scm, pr, &outcome).await.map(Some)
        }
    }
}

/// Ask the follow-up round after `session`, graded with `outcome`, if it
/// has one: post its questions and keep the check `Pending` (ADR-0010 D1,
/// D4). `None` when there is none to ask. If the follow-up can't be
/// posted, its session is discarded and the graded result stands.
async fn ask_follow_up<S, E, C, P>(
    app: &Soqrates<S, E, C>,
    scm: &P,
    pr: &PrRef,
    session: &Session,
    outcome: &SessionOutcome,
    code: &impl Codebase,
) -> Result<Option<CheckStatus>>
where
    S: StateStore,
    E: Examiner,
    C: Clock,
    P: ScmProvider,
{
    let Some(follow_up) = app.follow_up(session, outcome, code).await? else {
        return Ok(None);
    };
    if let Err(e) = scm.post_questions(pr, &follow_up).await {
        let _ = app.discard_session(follow_up.key()).await;
        return Err(e);
    }
    let summary = format!(
        "{} answer(s) were wrong: Soqrates asked a follow-up question about each. Answer them in \
         the new Soqrates comment.{}",
        follow_up.questions().len(),
        location_note(&outcome.profile_location)
    );
    scm.set_check(pr, CheckStatus::Pending, &summary).await?;
    Ok(Some(CheckStatus::Pending))
}

/// Report the result of the session `key` again if it was graded.
async fn report_graded<S, E, C, P>(
    app: &Soqrates<S, E, C>,
    scm: &P,
    pr: &PrRef,
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
    let summary = result_summary(graded.correct, graded.total, graded.passed, key);
    scm.set_check(pr, status, &summary).await?;
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
    // latest round is graded rather than carried (ADR-0002 D9); also those
    // to a follow-up round, which then asks no further one (ADR-0010 D6).
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
        // Its latest round: a follow-up stays open too (ADR-0010 D6).
        let latest = app.latest_follow_up(&standing).await?;
        if let Some(status) = report(app, scm, code, pr, pull, &latest).await? {
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
    if let Err(e) = scm.post_questions(pr, &session).await {
        // Don't leave a session whose questions nobody can see.
        let _ = app.discard_session(&key).await;
        return Err(e);
    }
    record_round(app, &mut rounds, pull, &session).await?;
    if session.questions().is_empty() {
        // Nothing to ask: grade the empty session right away.
        supersede_earlier(app, scm, pr, &session).await?;
        let outcome = app.submit(&key, &[], &NoCodebase).await?;
        return finish(app, scm, pr, &outcome).await;
    }
    let summary = format!(
        "Soqrates asked {} question(s) about this change. Answer them in the Soqrates comment.{}",
        session.questions().len(),
        location_note(&app.profile_location(&pull.author).await?)
    );
    scm.set_check(pr, CheckStatus::Pending, &summary).await?;
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
    let latest = app.latest_follow_up(&prev.session).await?;
    let Some(session) = app.pending_session(&latest).await? else {
        return Ok(());
    };
    if session.questions().is_empty() {
        return Ok(());
    }
    let Some(answers) = scm.fetch_answers(pr, &session).await? else {
        return Ok(());
    };
    // Acknowledged like any pickup (ADR-0004 D9), but the status is left
    // to the new head's round.
    scm.acknowledge_answers(pr, &session).await?;
    let codebase = codebase_of(code, pr, session.key(), &prev.head);
    let outcome = app.submit(session.key(), &answers, &codebase).await?;
    scm.post_results(pr, &session, &outcome).await
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
    let prefix = key_prefix(pr);
    for key in scm.superseded_questions(pr, current).await? {
        if key == *current.key() || !key.0.starts_with(&prefix) {
            continue;
        }
        let session = app.pending_session(&key).await?;
        // Freeze first: if that fails, the session stays and the next run
        // tries again.
        scm.supersede_questions(pr, &key, session.as_ref(), current)
            .await?;
        if session.is_some() {
            app.discard_session(&key).await?;
        }
    }
    Ok(())
}

/// Report a graded session as the final check status (ADR-0001 D8). The
/// summary mentions no other identity of the learner (ADR-0003 D6).
async fn finish<S, E, C, P>(
    app: &Soqrates<S, E, C>,
    scm: &P,
    pr: &PrRef,
    outcome: &SessionOutcome,
) -> Result<CheckStatus>
where
    S: StateStore,
    E: Examiner,
    C: Clock,
    P: ScmProvider,
{
    let status = final_status(outcome.passed, app.config.mode);
    // For a follow-up, about every concept of its first round (ADR-0010 D4).
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
    scm.set_check(pr, status, &summary).await?;
    Ok(status)
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
