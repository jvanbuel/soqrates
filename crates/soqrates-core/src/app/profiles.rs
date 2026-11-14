//! Reading a learner's profile and what is due on it (ADR-0001 D5, D6;
//! ADR-0002 D8).

use crate::Result;
use crate::app::engine::Soqrates;
use crate::app::ports::{Clock, Examiner, StateStore};
use crate::app::stores::{Placement, load_or_new};
use crate::assessment::ConceptScope;
use crate::ids::{AuthorId, ConceptId};
use crate::learner::LearnerProfile;

impl<S: StateStore, E: Examiner, C: Clock> Soqrates<S, E, C> {
    /// The profile of `author`'s learner, or a fresh `Novice` profile if the
    /// learner is new (ADR-0001 D5, ADR-0003 D1).
    pub async fn profile(&self, author: &AuthorId) -> Result<LearnerProfile> {
        let stores = self.stores().strict().await?;
        let place = stores.place(author).await?;
        load_or_new(place.primary, &place.learner).await
    }

    /// Due concepts with the scope of the store their card lives in, earliest
    /// due first, ties by concept ID (ADR-0002 D8, ADR-0003 D5).
    pub(super) async fn due_with_scope(
        &self,
        place: &Placement<'_, S>,
    ) -> Result<Vec<(ConceptId, ConceptScope)>> {
        let now = self.clock.now();
        let mut due = Vec::new();
        let primary = load_or_new(place.primary, &place.learner).await?;
        // With a separate project store, the primary store only holds general
        // concepts. Otherwise the scope is unknown: assume the safer Project.
        let primary_scope = match place.project {
            Some(_) => ConceptScope::General,
            None => ConceptScope::Project,
        };
        for c in primary.due_reviews(now) {
            let at = primary.card(&c).map(|k| k.due());
            due.push((at, c, primary_scope));
        }
        if let Some(project) = place.project {
            let profile = load_or_new(project, &place.learner).await?;
            for c in profile.due_reviews(now) {
                let at = profile.card(&c).map(|k| k.due());
                due.push((at, c, ConceptScope::Project));
            }
        }
        due.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
        // A concept can have a card in both stores if it was classified
        // differently over time. Ask it once, at its earliest position, and
        // keep the safer Project scope (ADR-0002 D6, ADR-0003 D5).
        let mut unique: Vec<(ConceptId, ConceptScope)> = Vec::new();
        for (_, concept, scope) in due {
            match unique.iter_mut().find(|(c, _)| *c == concept) {
                Some(existing) if scope == ConceptScope::Project => existing.1 = scope,
                Some(_) => {}
                None => unique.push((concept, scope)),
            }
        }
        Ok(unique)
    }

    /// Concepts whose review is due for `author` (ADR-0001 D6, ADR-0002 D8).
    pub async fn due_reviews(&self, author: &AuthorId) -> Result<Vec<ConceptId>> {
        let stores = self.stores().strict().await?;
        let place = stores.place(author).await?;
        Ok(self
            .due_with_scope(&place)
            .await?
            .into_iter()
            .map(|(c, _)| c)
            .collect())
    }
}
