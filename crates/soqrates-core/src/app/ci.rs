//! The CI check workflow (ADR-0001 D8).

use crate::Result;
use crate::app::engine::{ProfileLocation, SessionOutcome, Soqrates};
use crate::app::ports::{Clock, Examiner, ScmProvider, StateStore};
use crate::delivery::{CheckStatus, Mode, PrRef, PullRequest};
use crate::ids::SessionKey;

/// The session key for a PR at a given head commit. A new push gives a new key.
pub fn session_key(pr: &PullRequest) -> SessionKey {
    SessionKey(format!("{}#{}@{}", pr.pr.repo, pr.pr.number, pr.head_sha))
}

/// One run of the CI check. Call it on PR open/push and when the author
/// replies.
///
/// - No session for the current head yet: start one, post the questions and
///   set `Pending`.
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
        if session.questions().is_empty() {
            // Nothing to ask: grade the empty session right away.
            let outcome = app.submit(&key, &[]).await?;
            return finish(app, scm, pr, &outcome).await;
        }
        if let Err(e) = scm.post_questions(pr, &session).await {
            // Don't leave a session whose questions nobody can see.
            let _ = app.discard_session(&key).await;
            return Err(e);
        }
        let summary = format!(
            "Soqrates asked {} question(s) about this change. Answer them in the Soqrates comment.{}",
            session.questions().len(),
            location_note(&app.profile_location(&pull.author).await?)
        );
        scm.set_check(pr, CheckStatus::Pending, &summary).await?;
        return Ok(CheckStatus::Pending);
    };

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
