//! The local hook workflow (ADR-0001 D9). Used by `soqrates hook pre-commit`
//! (git) and `soqrates hook pre-push` (jj).

use crate::Result;
use crate::engine::Soqrates;
use crate::model::*;
use crate::ports::{Clock, Examiner, Prompter, StateStore};

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
    let _ = (app, prompter, kind, author, diff);
    todo!("ADR-0001 D9")
}
