//! The CI check workflow (ADR-0001 D8).

use crate::Result;
use crate::app::engine::{ProfileLocation, SessionOutcome, Soqrates};
use crate::app::ports::{Clock, Examiner, ScmProvider, StateStore};
use crate::delivery::{CheckStatus, Mode, PrRef, PullRequest, Session};
use crate::ids::SessionKey;

/// The session key for a PR at a given head commit. A new push gives a new key.
pub fn session_key(pr: &PullRequest) -> SessionKey {
    SessionKey(format!("{}{}", key_prefix(&pr.pr), pr.head_sha))
}

/// The start of every session key of `pr`, whatever its head.
fn key_prefix(pr: &PrRef) -> String {
    format!("{}#{}@", pr.repo, pr.number)
}

/// One run of the CI check. Call it on PR open/push and when the author
/// replies.
///
/// - No session for the current head yet: start one, post the questions,
///   freeze the question comments of earlier heads that were never graded
///   (ADR-0004 D7) and set `Pending`. With nothing to ask, the comment says
///   so and the empty session is graded right away.
/// - Session exists but no answers yet: keep `Pending`.
/// - Answers present: grade them and set `Success`, or `Failure` (`Neutral`
///   in `Advisory` mode).
///
/// Returns the status that was set.
pub async fn run_check<S, E, C, P>(
    app: &Soqrates<S, E, C>,
    scm: &P,
    pr: &PrRef,
) -> Result<CheckStatus>
where
    S: StateStore,
    E: Examiner,
    C: Clock,
    P: ScmProvider,
{
    let pull = scm.pull_request(pr).await?;
    let key = session_key(&pull);

    // Already graded for this head: report the result again, ask nothing.
    if let Some(graded) = app.graded(&key).await? {
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
        scm.set_check(pr, status, &summary).await?;
        return Ok(status);
    }

    let Some(session) = app.pending_session(&key).await? else {
        let session = app
            .start_session(key.clone(), &pull.author, &pull.diff)
            .await?;
        // Also when there is nothing to ask: the comment then says so, and
        // earlier question comments can point to it (ADR-0004 D7).
        if let Err(e) = scm.post_questions(pr, &session).await {
            // Don't leave a session whose questions nobody can see.
            let _ = app.discard_session(&key).await;
            return Err(e);
        }
        if session.questions().is_empty() {
            // Nothing to ask: grade the empty session right away.
            supersede_earlier(app, scm, pr, &session).await?;
            let outcome = app.submit(&key, &[]).await?;
            return finish(app, scm, pr, &outcome).await;
        }
        let summary = format!(
            "Soqrates asked {} question(s) about this change. Answer them in the Soqrates comment.{}",
            session.questions().len(),
            location_note(&app.profile_location(&pull.author).await?)
        );
        scm.set_check(pr, CheckStatus::Pending, &summary).await?;
        supersede_earlier(app, scm, pr, &session).await?;
        return Ok(CheckStatus::Pending);
    };

    // Also on later runs, so that a run that failed to freeze the earlier
    // comments, or to grade an empty session, is made good by the next one.
    supersede_earlier(app, scm, pr, &session).await?;
    if session.questions().is_empty() {
        let outcome = app.submit(&key, &[]).await?;
        return finish(app, scm, pr, &outcome).await;
    }
    match scm.fetch_answers(pr, &session).await? {
        None => {
            let summary = format!(
                "Waiting for answers.{}",
                location_note(&app.profile_location(&pull.author).await?)
            );
            scm.set_check(pr, CheckStatus::Pending, &summary).await?;
            Ok(CheckStatus::Pending)
        }
        Some(answers) => {
            let outcome = app.submit(&key, &answers).await?;
            scm.post_results(pr, &session, &outcome).await?;
            finish(app, scm, pr, &outcome).await
        }
    }
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
    scm.set_check(pr, status, &summary).await?;
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
