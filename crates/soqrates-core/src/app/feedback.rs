//! Feedback on the questions of graded sessions: recording it, a
//! maintainer resolving it, and exporting what was asked (ADR-0011).

use crate::app::engine::Soqrates;
use crate::app::ports::{Clock, Examiner, StateStore};
use crate::delivery::{
    AskedQuestion, BankedQuestion, FeedbackGiven, FeedbackId, FeedbackKind, FeedbackRequest,
    QuestionFeedback,
};
use crate::ids::{AuthorId, SessionKey};
use crate::{Error, Result};

/// Apply `change` to the bank question `asked` came from, if any, in
/// `bank`. Does nothing for a question that isn't banked, or whose bank
/// entry is gone.
pub(super) async fn change_bank_entry<S: StateStore>(
    bank: &S,
    asked: &AskedQuestion,
    change: impl FnOnce(&mut BankedQuestion),
) -> Result<()> {
    let Some(reference) = asked.bank() else {
        return Ok(());
    };
    let mut entries = bank.bank_questions(&asked.question.concept).await?;
    if let Some(entry) = entries.iter_mut().find(|e| *e.id() == reference.id) {
        change(entry);
        bank.save_bank_question(entry).await?;
    }
    Ok(())
}

impl<S: StateStore, E: Examiner, C: Clock> Soqrates<S, E, C> {
    /// Record `request`, feedback on a question of the graded session `key`
    /// (ADR-0011 D2). `by` is the author giving it, who must be the
    /// session's; `None` on a surface where holding the session key shows
    /// that, such as MCP. `source` names where it came from, e.g. a forge
    /// comment, so the same feedback counts once.
    ///
    /// Fails with `NotFound` when the session wasn't graded (or nothing of
    /// it was kept) and with `Invalid` for anyone but its author or a
    /// question it doesn't have. A dispute keeps the question's bank entry
    /// from being reused; `good` feedback raises it (D6). No grade changes
    /// (D7).
    pub async fn give_feedback(
        &self,
        by: Option<&AuthorId>,
        key: &SessionKey,
        request: &FeedbackRequest,
        source: Option<&str>,
    ) -> Result<FeedbackGiven> {
        let stores = self.stores().strict().await?;
        let not_graded = || {
            Error::NotFound(format!(
                "graded results for session {:?}: feedback is on the questions of graded results",
                key.0
            ))
        };
        let (store, mut asked, learner) = match by {
            Some(author) => {
                let store = stores
                    .feedback_store(&author.identity()?)?
                    .ok_or_else(not_graded)?;
                let asked = store.load_asked(key).await?.ok_or_else(not_graded)?;
                let learner = stores.place(author).await?.learner;
                (store, asked, learner)
            }
            None => {
                let mut found = None;
                for store in stores.feedback_stores() {
                    if let Some(asked) = store.load_asked(key).await? {
                        found = Some((store, asked));
                        break;
                    }
                }
                let (store, asked) = found.ok_or_else(not_graded)?;
                let learner = asked.learner().clone();
                (store, asked, learner)
            }
        };
        let given = asked.give(&learner, request, source, self.clock.now())?;
        if !given.new {
            return Ok(given);
        }
        // The bank first: noting the same feedback again is harmless, and
        // once the asked session is saved, the same source isn't noted again.
        let number = given.number;
        let question = &asked.questions()[number - 1];
        let id = given.feedback.id.clone();
        match given.feedback.kind {
            kind if kind.disputes() => {
                change_bank_entry(stores.bank_of(store), question, |e| e.dispute(&id)).await?
            }
            FeedbackKind::Good => {
                change_bank_entry(stores.bank_of(store), question, |e| e.commend(&id)).await?
            }
            _ => {}
        }
        store.save_asked(&asked).await?;
        Ok(given)
    }

    /// A maintainer accepts or rejects the feedback `id` (ADR-0011 D7). It
    /// no longer keeps its bank question from being reused; accepting a
    /// dispute retires that question. Grades and review cards stay as they
    /// are. Fails with `NotFound` for unknown feedback.
    pub async fn resolve_feedback(
        &self,
        id: &FeedbackId,
        accept: bool,
    ) -> Result<(AskedQuestion, QuestionFeedback)> {
        let stores = self.stores().strict().await?;
        for store in stores.feedback_stores() {
            for mut asked in store.asked_sessions().await? {
                let Some((question, feedback)) = asked.resolve(id, accept) else {
                    continue;
                };
                let retire = accept && feedback.kind.disputes();
                change_bank_entry(stores.bank_of(store), &question, |e| {
                    e.settle(id);
                    if retire {
                        e.retire();
                    }
                })
                .await?;
                store.save_asked(&asked).await?;
                return Ok((question, feedback));
            }
        }
        Err(Error::NotFound(format!("feedback {:?}", id.0)))
    }

    /// Every asked question, oldest session first, as JSON lines for
    /// `soqrates questions export` (ADR-0011 D9); with `with_feedback`, with
    /// the answers, grades and feedback.
    pub async fn export_questions(&self, with_feedback: bool) -> Result<Vec<serde_json::Value>> {
        let stores = self.stores().strict().await?;
        let mut sessions = Vec::new();
        for store in stores.feedback_stores() {
            sessions.extend(store.asked_sessions().await?);
        }
        sessions.sort_by(|a, b| (a.at(), a.key()).cmp(&(b.at(), b.key())));
        Ok(sessions
            .iter()
            .flat_map(|s| s.export(with_feedback))
            .collect())
    }
}
