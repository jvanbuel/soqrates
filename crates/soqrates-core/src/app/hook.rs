//! The local hook workflow (ADR-0001 D9). Used by `soqrates hook pre-commit`
//! (git) and `soqrates hook pre-push` (jj).

use crate::Result;
use crate::app::engine::Soqrates;
use crate::app::ports::{Clock, Examiner, Prompter, StateStore};
use crate::assessment::Diff;
use crate::delivery::{HookDecision, HookKind, Mode};
use crate::ids::{AuthorId, SessionKey};

/// Quiz `author` about `diff` at the terminal and decide whether the commit
/// or push may proceed. An empty diff asks nothing and allows.
pub async fn run_hook<S, E, C, T>(
    app: &Soqrates<S, E, C>,
    prompter: &mut T,
    kind: HookKind,
    author: &AuthorId,
    diff: &Diff,
) -> Result<HookDecision>
where
    S: StateStore,
    E: Examiner,
    C: Clock,
    T: Prompter,
{
    if diff.is_empty() {
        return Ok(HookDecision::Allow);
    }
    let key = SessionKey(format!(
        "hook:{kind:?}:{}:{}",
        author.0,
        app.clock.now().to_rfc3339()
    ));
    let session = app.start_session(key.clone(), author, diff).await?;
    let mut answers = Vec::with_capacity(session.questions().len());
    for q in session.questions() {
        answers.push(prompter.ask(q)?);
    }
    let outcome = app.submit(&key, &answers).await?;
    prompter.report(&outcome);
    Ok(match (outcome.passed, app.config.mode) {
        (false, Mode::Blocking) => HookDecision::Block,
        _ => HookDecision::Allow,
    })
}
