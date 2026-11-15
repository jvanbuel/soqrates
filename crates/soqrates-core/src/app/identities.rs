//! Identities: which learner is behind an author, and linking, unlinking
//! and deleting learners (ADR-0003).

use crate::app::engine::Soqrates;
use crate::app::ports::{Clock, Decision, Examiner, StateStore};
use crate::app::stores::change_bank_entry;
use crate::app::stores::{Access, ProfileLocation};
use crate::ids::{AuthorId, Identity, LearnerId};
use crate::learner::{IdentityLink, LearnerProfile, LinkCode, LinkRejection};
use crate::{Error, Result};

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
        // A code is issued once: one that is in use already is refused, not
        // replaced, so a code someone holds can't be taken over.
        stores
            .learner_store()
            .update_link_code(code.code(), |current| match current {
                None => Ok(Decision::Put(code.clone(), ())),
                Some(_) => Err(Error::Invalid("that link code is already in use".into())),
            })
            .await?;
        Ok(code)
    }

    /// Present `code` as `author`: links `author`'s identity to the learner
    /// that requested the code, merging the two learners (D2, D3). Fails
    /// with [`Error::Link`] when the code is rejected.
    pub async fn present_link_code(&self, author: &AuthorId, code: &str) -> Result<LearnerId> {
        let stores = self.stores().strict().await?;
        let presenter = author.identity()?;
        let now = self.clock.now();
        // Redeemed with an update: of two presenters of one code, the second
        // finds it redeemed and is rejected (D2).
        let link_code = stores
            .learner_store()
            .update_link_code(code, |current| {
                let mut link_code = current.ok_or(LinkRejection::UnknownCode)?;
                link_code.redeem(&presenter, now)?;
                Ok(Decision::Put(link_code.clone(), link_code))
            })
            .await?;
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
            // Their asked sessions and feedback become the target's, before
            // the learner is deleted with all that is theirs (ADR-0011 D4,
            // D8): open disputes stay resolvable and on the bank.
            for asked in store.asked_sessions().await? {
                if *asked.learner() != other {
                    continue;
                }
                // Feedback given meanwhile moves along with it.
                store
                    .update_asked(asked.key(), |current| {
                        Ok(match current {
                            Some(asked) if *asked.learner() == other => {
                                Decision::Put(asked.owned_by(target.clone()), ())
                            }
                            _ => Decision::Keep(()),
                        })
                    })
                    .await?;
            }
            let theirs = store.load_profile(&other).await?;
            store.delete_learner(&other).await?;
            // Merged into the target's profile as it is when saved, so that
            // reviews graded for the target meanwhile are kept.
            store
                .update_profile(target, |current| {
                    if current.is_none() && theirs.is_none() {
                        return Ok(Decision::Keep(()));
                    }
                    let parts: Vec<LearnerProfile> =
                        current.into_iter().chain(theirs.clone()).collect();
                    Ok(Decision::Put(
                        LearnerProfile::merged(target.clone(), &parts),
                        (),
                    ))
                })
                .await?;
        }
        let link = IdentityLink {
            identity: identity.clone(),
            learner: target.clone(),
            since: self.clock.now(),
        };
        stores
            .learner_store()
            .update_link(identity, |_| Ok(Decision::Put(link.clone(), ())))
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
            // Split from the profile as it is when saved, so that reviews
            // graded meanwhile end up on one of the two parts.
            let split_off = store
                .update_profile(&learner, |current| {
                    Ok(match current {
                        None => Decision::Keep(None),
                        Some(profile) => {
                            let (remaining, split_off) =
                                profile.split(&identity, new_learner.clone());
                            Decision::Put(remaining, Some(split_off))
                        }
                    })
                })
                .await?;
            if let Some(split_off) = split_off {
                // The new learner is fresh: nobody else writes its profile.
                store
                    .update_profile(&new_learner, |current| match current {
                        None => Ok(Decision::Put(split_off.clone(), ())),
                        Some(_) => Err(Error::Invalid(format!(
                            "learner {:?} has a profile already",
                            new_learner.0
                        ))),
                    })
                    .await?;
            }
        }
        let link = IdentityLink {
            identity: identity.clone(),
            learner: new_learner.clone(),
            since: self.clock.now(),
        };
        stores
            .learner_store()
            .update_link(&identity, |_| Ok(Decision::Put(link.clone(), ())))
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
        // Their feedback no longer counts on the bank questions it was about
        // (ADR-0011 D8); the stores delete the feedback itself.
        for store in stores.all() {
            for asked in store.asked_sessions().await? {
                if *asked.learner() != learner {
                    continue;
                }
                for q in asked.questions().iter().filter(|q| !q.feedback.is_empty()) {
                    change_bank_entry(stores.bank_of(store), q, |entry| {
                        for f in &q.feedback {
                            entry.settle(&f.id);
                        }
                    })
                    .await?;
                }
            }
        }
        for store in stores.all_with_bank() {
            store.delete_learner(&learner).await?;
        }
        Ok(())
    }
}
