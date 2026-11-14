//! Feedback on the questions of graded sessions: recording it, a
//! maintainer resolving it, and exporting what was asked (ADR-0011).

use crate::app::engine::Soqrates;
use crate::app::ports::{Clock, Examiner, StateStore};
use crate::delivery::{
    AskedQuestion, FeedbackGiven, FeedbackId, FeedbackRequest, QuestionFeedback,
};
use crate::ids::{AuthorId, SessionKey};
use crate::{Error, Result};

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
        for _ in 0..100 {
            let previous = asked.clone();
            let given = asked.give(&learner, request, source, self.clock.now())?;
            if !given.new || store.save_asked_if_current(&asked, Some(&previous)).await? {
                stores.sync_feedback(store, &asked).await?;
                return Ok(given);
            }
            asked = store.load_asked(key).await?.ok_or_else(not_graded)?;
        }
        Err(Error::Port("asked session kept changing".into()))
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
                if asked.find(id).is_none() {
                    continue;
                }
                for _ in 0..100 {
                    let previous = asked.clone();
                    let Some(result) = asked.resolve(id, accept) else {
                        break;
                    };
                    if store.save_asked_if_current(&asked, Some(&previous)).await? {
                        stores.sync_feedback(store, &asked).await?;
                        return Ok(result);
                    }
                    asked = store
                        .load_asked(previous.key())
                        .await?
                        .ok_or_else(|| Error::NotFound("asked session deleted".into()))?;
                }
                return Err(Error::Port("asked session kept changing".into()));
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
