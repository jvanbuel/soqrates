//! Identities: which learner is behind an author, and linking, unlinking
//! and deleting learners (ADR-0003).

use crate::Result;
use crate::app::engine::Soqrates;
use crate::app::ports::{Clock, Examiner, StateStore};
use crate::app::stores::{Access, ProfileLocation};
use crate::ids::{AuthorId, Identity, LearnerId};
use crate::learner::{IdentityLink, LearnerProfile, LinkCode, LinkRejection};

impl<S: StateStore, E: Examiner, C: Clock> Soqrates<S, E, C> {
    /// Where `author`'s profile lives right now (ADR-0003 D4).
    pub async fn profile_location(&self, author: &AuthorId) -> Result<ProfileLocation> {
        let stores = self.stores().strict().await?;
        Ok(stores.place(author).await?.location)
    }

    /// The learner behind `author`'s identity. The first time an identity is
    /// seen, a new learner is created and linked to it (D1).
    pub async fn learner_of(&self, author: &AuthorId) -> Result<LearnerId> {
        let stores = self.stores().strict().await?;
        Ok(stores.place(author).await?.learner)
    }

    /// Issue the link code `code` for `author`'s learner (D2). The caller
    /// supplies an unguessable `code`, e.g. from the OS random source.
    pub async fn request_link_code(&self, author: &AuthorId, code: String) -> Result<LinkCode> {
        let stores = self.stores().strict().await?;
        let identity = author.identity()?;
        let learner = stores.resolve_in(&identity, stores.learner_store()).await?;
        let code = LinkCode::issue(code, identity, learner, self.clock.now());
        stores.learner_store().save_link_code(&code).await?;
        Ok(code)
    }

    /// Present `code` as `author`: links `author`'s identity to the learner
    /// that requested the code, merging the two learners (D2, D3). Fails
    /// with [`Error::Link`] when the code is rejected.
    pub async fn present_link_code(&self, author: &AuthorId, code: &str) -> Result<LearnerId> {
        let stores = self.stores().strict().await?;
        let presenter = author.identity()?;
        let mut link_code = self
            .store
            .load_link_code(code)
            .await?
            .ok_or(LinkRejection::UnknownCode)?;
        link_code.redeem(&presenter, self.clock.now())?;
        stores.learner_store().save_link_code(&link_code).await?;
        let target = self
            .store
            .learner_of(link_code.requested_by())
            .await?
            .unwrap_or_else(|| link_code.learner().clone());
        self.link_into(&stores, &target, &presenter).await
    }

    /// An administrator of `organisation` declares that `a` and `b` are the
    /// same learner. Both hosts must be administered by `organisation` (D2).
    pub async fn declare_link(
        &self,
        organisation: &str,
        a: &AuthorId,
        b: &AuthorId,
    ) -> Result<LearnerId> {
        let stores = self.stores().strict().await?;
        let (a, b) = (a.identity()?, b.identity()?);
        for identity in [&a, &b] {
            if stores.organisation_of(identity) != Some(organisation) {
                return Err(LinkRejection::NotAdministered {
                    host: identity.host().unwrap_or("e-mail").to_string(),
                    organisation: organisation.to_string(),
                }
                .into());
            }
        }
        let target = stores.resolve_in(&a, stores.learner_store()).await?;
        self.link_into(&stores, &target, &b).await
    }

    /// Link `identity` to `target`, merging its learner in if that learner
    /// has no other identities; reject otherwise (D2, D3).
    async fn link_into(
        &self,
        stores: &Access<'_, S, C>,
        target: &LearnerId,
        identity: &Identity,
    ) -> Result<LearnerId> {
        let other = stores.resolve_in(identity, stores.learner_store()).await?;
        if &other == target {
            return Ok(other);
        }
        if stores.learner_store().links_of(&other).await?.len() > 1 {
            return Err(LinkRejection::BelongsToAnotherLearner(identity.clone()).into());
        }
        for store in stores.all() {
            let a = store.load_profile(target).await?;
            let b = store.load_profile(&other).await?;
            if a.is_none() && b.is_none() {
                continue;
            }
            let parts: Vec<LearnerProfile> = a.into_iter().chain(b).collect();
            store.delete_learner(&other).await?;
            store
                .save_profile(&LearnerProfile::merged(target.clone(), &parts))
                .await?;
        }
        stores
            .learner_store()
            .save_link(&IdentityLink {
                identity: identity.clone(),
                learner: target.clone(),
                since: self.clock.now(),
            })
            .await?;
        Ok(target.clone())
    }

    /// Unlink `author`'s identity from its learner, splitting the profile
    /// (D3). Returns the new learner of the unlinked identity.
    pub async fn unlink(&self, author: &AuthorId) -> Result<LearnerId> {
        let stores = self.stores().strict().await?;
        let identity = author.identity()?;
        let learner = stores.resolve_in(&identity, stores.learner_store()).await?;
        if stores.learner_store().links_of(&learner).await?.len() <= 1 {
            return Ok(learner);
        }
        let new_learner = stores
            .fresh_learner_id(&identity, stores.learner_store())
            .await?;
        for store in stores.all() {
            if let Some(profile) = store.load_profile(&learner).await? {
                let (remaining, split_off) = profile.split(&identity, new_learner.clone());
                store.save_profile(&remaining).await?;
                store.save_profile(&split_off).await?;
            }
        }
        stores
            .learner_store()
            .save_link(&IdentityLink {
                identity,
                learner: new_learner.clone(),
                since: self.clock.now(),
            })
            .await?;
        Ok(new_learner)
    }

    /// All identities of `author`'s learner, sorted (D6).
    pub async fn identities(&self, author: &AuthorId) -> Result<Vec<Identity>> {
        let stores = self.stores().strict().await?;
        let learner = stores
            .resolve_in(&author.identity()?, stores.learner_store())
            .await?;
        let mut ids: Vec<Identity> = self
            .store
            .links_of(&learner)
            .await?
            .into_iter()
            .map(|l| l.identity)
            .collect();
        ids.sort();
        Ok(ids)
    }

    /// Delete `author`'s learner: every review and every link, in every
    /// store (D6).
    pub async fn delete_learner(&self, author: &AuthorId) -> Result<()> {
        let stores = self.stores().strict().await?;
        let learner = stores
            .resolve_in(&author.identity()?, stores.learner_store())
            .await?;
        for store in stores.all_with_bank() {
            store.delete_learner(&learner).await?;
        }
        Ok(())
    }
}
