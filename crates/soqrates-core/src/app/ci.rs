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

    let Some(session) = app.pending_session(&key).await? else {
        let session = app
            .start_session(key.clone(), &pull.author, &pull.diff)
            .await?;
        if session.questions().is_empty() {
            // Nothing to ask: grade the empty session right away.
            let outcome = app.submit(&key, &[]).await?;
            return finish(app, scm, pr, &outcome).await;
        }
        scm.post_questions(pr, &session).await?;
        let summary = format!(
            "Soqrates asked {} question(s) about this change. Reply to answer them.",
            session.questions().len()
        );
        scm.set_check(pr, CheckStatus::Pending, &summary).await?;
        return Ok(CheckStatus::Pending);
    };

    match scm.fetch_answers(pr, &session).await? {
        None => {
            scm.set_check(pr, CheckStatus::Pending, "Waiting for answers.")
                .await?;
            Ok(CheckStatus::Pending)
        }
        Some(answers) => {
            let outcome = app.submit(&key, &answers).await?;
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
    let status = match (outcome.passed, app.config.mode) {
        (true, _) => CheckStatus::Success,
        (false, Mode::Blocking) => CheckStatus::Failure,
        (false, Mode::Advisory) => CheckStatus::Neutral,
    };
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
    if let ProfileLocation::Local { host } = &outcome.profile_location {
        summary.push_str(&format!(
            " The learner store was unreachable; this learner profile is local to {host}."
        ));
    }
    scm.set_check(pr, status, &summary).await?;
    Ok(status)
}
