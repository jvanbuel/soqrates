//! The local hook workflow (ADR-0001 D9). Used by `soqrates hook pre-commit`
//! (git) and `soqrates hook pre-push` (jj).

use crate::app::engine::Soqrates;
use crate::app::ports::{Clock, Codebase, Examiner, Prompter, StateStore};
use crate::assessment::Diff;
use crate::delivery::{HookDecision, HookKind, Mode};
use crate::ids::{AuthorId, SessionKey};
use crate::{Error, Result};

/// Quiz `author` about `diff` at the terminal and decide whether the commit
/// or push may proceed. An empty diff asks nothing and allows. Wrong
/// answers get follow-up rounds right away, and the final round decides
/// (ADR-0010 D7). After the results of the final round the author may give
/// feedback on one of its questions, once (ADR-0011 D2); feedback never
/// changes the decision.
/// The examiner may read `code`, what is being committed or pushed
/// (ADR-0008 D4).
pub async fn run_hook<S, E, C, T>(
    app: &Soqrates<S, E, C>,
    prompter: &mut T,
    kind: HookKind,
    author: &AuthorId,
    diff: &Diff,
    code: &impl Codebase,
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
    let mut session = app.start_session(key, author, diff, code).await?;
    let outcome = loop {
        let mut answers = Vec::with_capacity(session.questions().len());
        for q in session.questions() {
            answers.push(prompter.ask(q)?);
        }
        let outcome = app.submit(session.key(), &answers, code).await?;
        prompter.report(&outcome);
        match app.follow_up(&session, &outcome, code).await? {
            Some(next) => session = next,
            None => break outcome,
        }
    };
    // Best effort: a prompt or store that fails here must not block.
    if let Ok(Some(request)) = prompter.ask_feedback(&outcome) {
        let given = app
            .give_feedback(Some(author), session.key(), &request, None)
            .await;
        let note = match given {
            Ok(given) => given.note(),
            Err(Error::Invalid(why)) => format!("This feedback was not recorded: {why}."),
            Err(e) => format!("This feedback could not be recorded: {e}."),
        };
        prompter.feedback_noted(&note);
    }
    Ok(match (outcome.passed, app.config.mode) {
        (false, Mode::Blocking) => HookDecision::Block,
        _ => HookDecision::Allow,
    })
}
