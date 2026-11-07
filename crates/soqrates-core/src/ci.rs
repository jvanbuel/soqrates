//! The CI check workflow (ADR-0001 D8).

use crate::Result;
use crate::engine::Soqrates;
use crate::model::*;
use crate::ports::{Clock, Examiner, ScmProvider, StateStore};

/// The session key for a PR at a given head commit. A new push gives a new key.
pub fn session_key(pr: &PullRequest) -> SessionKey {
    let _ = pr;
    todo!("ADR-0001 D8")
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
    let _ = (app, scm, pr);
    todo!("ADR-0001 D8")
}
