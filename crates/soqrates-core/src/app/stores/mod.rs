//! The store router: the one place that knows how the stores are laid out
//! (ADR-0003 D4, D5) and what to do when the learner store is unreachable.
//!
//! The services of the engine never touch a store directly. They ask the
//! router for one, through one of three entry points on [`Stores`], and each
//! entry point says what happens to the gradings left behind first
//! (ADR-0002 D6):
//!
//! - [`Stores::strict`] finishes them and fails if one can't be finished,
//! - [`Stores::best_effort`] tries and carries on regardless,
//! - [`Stores::without_recovery`] doesn't, for documents that no grading
//!   writes.
//!
//! What a service gets back, an [`Access`], is the only way to reach the
//! stores, so a new operation has to pick one. Applying a journal lives here
//! too, in [`journal`], because recovery is nothing but applying the journals
//! found in the stores: were it in the grading service, the router would
//! depend on the service that depends on the router.
//!
//! The router is a view over the stores that [`Soqrates`](super::Soqrates)
//! keeps in its public fields, built per call; it owns nothing, so the
//! fields stay what they were.

mod journal;

use std::collections::BTreeMap;
use std::future::Future;

use serde::{Deserialize, Serialize};

use crate::app::ports::{Clock, StateStore};
use crate::delivery::{
    AskedQuestion, AskedSession, BankedQuestion, GradedSession, Session, StoreId,
};
use crate::ids::{AuthorId, Identity, LearnerId, SessionKey};
use crate::learner::{IdentityLink, LearnerProfile};
use crate::{Error, Result};

pub(in crate::app) use journal::{Recorded, grading_mark};

/// Where a learner's profile was read from and written to (ADR-0003 D4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProfileLocation {
    /// The shared learner store.
    LearnerStore,
    /// The learner store was unreachable; a profile local to this forge host
    /// was used instead.
    Local { host: String },
}

/// Where one learner's data lives for one interaction (ADR-0003 D4, D5).
pub(in crate::app) struct Placement<'a, S> {
    pub learner: LearnerId,
    /// Holds the profile with general reviews (and links).
    pub primary: &'a S,
    pub primary_id: StoreId,
    /// Holds the profile with project reviews: the organisation's store.
    /// `None` when general and project reviews share `primary`.
    pub project: Option<&'a S>,
    pub project_id: Option<StoreId>,
    pub location: ProfileLocation,
}

/// The stores of one engine, as the router sees them.
pub(in crate::app) struct Stores<'a, S, C> {
    learner: &'a S,
    orgs: &'a BTreeMap<String, S>,
    shared_bank: Option<&'a S>,
    forges: &'a BTreeMap<String, String>,
    clock: &'a C,
}

impl<S, C> Clone for Stores<'_, S, C> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S, C> Copy for Stores<'_, S, C> {}

/// Access to the stores after recovery was dealt with. Only [`Stores`] makes
/// one.
pub(in crate::app) struct Access<'a, S, C>(Stores<'a, S, C>);

impl<S, C> Clone for Access<'_, S, C> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S, C> Copy for Access<'_, S, C> {}

impl<'a, S: StateStore, C: Clock> Stores<'a, S, C> {
    pub fn new(
        learner: &'a S,
        orgs: &'a BTreeMap<String, S>,
        shared_bank: Option<&'a S>,
        forges: &'a BTreeMap<String, String>,
        clock: &'a C,
    ) -> Self {
        Self {
            learner,
            orgs,
            shared_bank,
            forges,
            clock,
        }
    }

    /// Finish every grading left behind first, e.g. by a crash or a failed
    /// write, and fail if one can't be finished (ADR-0002 D6). For every
    /// operation that must not act on a half-recorded grading.
    pub async fn strict(self) -> Result<Access<'a, S, C>> {
        let access = Access(self);
        access.recover().await?;
        Ok(access)
    }

    /// Try to finish the gradings left behind, and carry on whether or not
    /// that worked. For looking a session up: one that can't be finished yet
    /// is reported as being graded, not as an error (ADR-0002 D6).
    pub async fn best_effort(self) -> Access<'a, S, C> {
        let access = Access(self);
        let _ = access.recover().await;
        access
    }

    /// Access without recovering. Only for documents that no grading
    /// writes or reads, such as the rounds of a pull request.
    pub fn without_recovery(self) -> Access<'a, S, C> {
        Access(self)
    }
}

/// Whether `error` says a store can't be reached. Only the learner store may
/// be unreachable without failing the operation, and only when there are
/// organisation stores to fall back to (ADR-0003 D4).
fn unreachable(error: &Error) -> bool {
    matches!(error, Error::Port(_))
}

impl<'a, S: StateStore, C: Clock> Access<'a, S, C> {
    // --- topology ------------------------------------------------------------

    /// The learner store (ADR-0003 D4): links, link codes, profiles with
    /// general reviews and, without organisation stores, sessions.
    pub fn learner_store(&self) -> &'a S {
        self.0.learner
    }

    /// Every store that may hold data about learners: the learner store,
    /// then the organisation stores.
    pub fn all(&self) -> impl Iterator<Item = &'a S> + use<'a, S, C> {
        std::iter::once(self.0.learner).chain(self.0.orgs.values())
    }

    /// [`all`](Self::all) and the shared question bank, if there is one.
    pub fn all_with_bank(&self) -> impl Iterator<Item = &'a S> + use<'a, S, C> {
        self.all().chain(self.0.shared_bank)
    }

    /// The organisation administering `identity`'s forge, if configured.
    pub fn organisation_of(&self, identity: &Identity) -> Option<&'a str> {
        identity
            .host()
            .and_then(|h| self.0.forges.get(h))
            .map(String::as_str)
    }

    /// The store for project concepts and sessions of `identity`'s forge:
    /// its organisation's store. Without configured forges, everything lives
    /// in the learner store (a single-organisation deployment). A configured
    /// organisation without a store is a configuration error: project data
    /// must never fall into the shared learner store (ADR-0003 D5).
    fn org_store_for(&self, identity: &Identity) -> Result<Option<&'a S>> {
        match self.organisation_of(identity) {
            None => Ok(None),
            Some(org) => self
                .0
                .orgs
                .get(org)
                .map(Some)
                .ok_or_else(|| Error::Invalid(format!("no store for organisation {org:?}"))),
        }
    }

    /// The store that holds sessions (question text, answers) for `author`:
    /// never the shared learner store when an organisation store exists.
    pub fn session_store(&self, author: &AuthorId) -> Result<&'a S> {
        Ok(self
            .org_store_for(&author.identity()?)?
            .unwrap_or(self.0.learner))
    }

    /// The store holding the question bank for `author`'s rounds (ADR-0009
    /// D2): the shared bank if there is one, else the store of `author`'s
    /// sessions. Never the shared learner store: with organisation stores
    /// configured, an author outside every organisation gets no bank.
    pub fn bank_store(&self, author: &AuthorId) -> Result<Option<&'a S>> {
        if let Some(shared) = self.0.shared_bank {
            return Ok(Some(shared));
        }
        match self.org_store_for(&author.identity()?)? {
            Some(store) => Ok(Some(store)),
            None if self.0.orgs.is_empty() => Ok(Some(self.0.learner)),
            None => Ok(None),
        }
    }

    /// The store that keeps `identity`'s asked sessions and their feedback
    /// (ADR-0011 D1): the organisation's store, or the single store of a
    /// deployment without organisations. `None` for an identity outside
    /// every organisation when there are organisation stores: the remaining
    /// store is the learner store, which holds no question text.
    pub fn feedback_store(&self, identity: &Identity) -> Result<Option<&'a S>> {
        match self.org_store_for(identity)? {
            Some(store) => Ok(Some(store)),
            None if self.0.orgs.is_empty() => Ok(Some(self.0.learner)),
            None => Ok(None),
        }
    }

    /// Every store that may keep asked sessions (ADR-0011 D1).
    pub fn feedback_stores(&self) -> Vec<&'a S> {
        if self.0.orgs.is_empty() {
            vec![self.0.learner]
        } else {
            self.0.orgs.values().collect()
        }
    }

    /// The bank holding the bank questions asked from `store`'s sessions
    /// (ADR-0009 D2): the shared bank if there is one, else `store`.
    pub fn bank_of(&self, store: &'a S) -> &'a S {
        self.0.shared_bank.unwrap_or(store)
    }

    /// The store `id` names.
    pub fn by_id(&self, id: &StoreId) -> Result<&'a S> {
        match id {
            StoreId::LearnerStore => Ok(self.0.learner),
            StoreId::Organisation(org) => self
                .0
                .orgs
                .get(org)
                .ok_or_else(|| Error::Invalid(format!("no store for organisation {org:?}"))),
        }
    }

    // --- placement -----------------------------------------------------------

    /// An opaque learner ID, derived from `identity` but not revealing it
    /// (ADR-0003 D1), that `store` does not use yet.
    pub async fn fresh_learner_id(&self, identity: &Identity, store: &S) -> Result<LearnerId> {
        let base = opaque_id(&identity.to_string());
        let mut n = 1;
        loop {
            let id = if n == 1 {
                LearnerId(format!("learner-{base}"))
            } else {
                LearnerId(format!("learner-{base}-{n}"))
            };
            if store.load_profile(&id).await?.is_none() && store.links_of(&id).await?.is_empty() {
                return Ok(id);
            }
            n += 1;
        }
    }

    /// Resolve `identity` in `store`, creating and linking a new learner the
    /// first time it is seen (ADR-0003 D1).
    pub async fn resolve_in(&self, identity: &Identity, store: &S) -> Result<LearnerId> {
        if let Some(learner) = store.learner_of(identity).await? {
            return Ok(learner);
        }
        let learner = self.fresh_learner_id(identity, store).await?;
        store
            .save_link(&IdentityLink {
                identity: identity.clone(),
                learner: learner.clone(),
                since: self.0.clock.now(),
            })
            .await?;
        store
            .save_profile(&LearnerProfile::new(learner.clone()))
            .await?;
        Ok(learner)
    }

    /// Where `author`'s data lives. Falls back to the organisation's store
    /// when the learner store is unreachable (ADR-0003 D4).
    pub async fn place(&self, author: &AuthorId) -> Result<Placement<'a, S>> {
        let identity = author.identity()?;
        let org_store = self.org_store_for(&identity)?;
        let org_id = self
            .organisation_of(&identity)
            .map(|org| StoreId::Organisation(org.to_string()));
        match self.resolve_in(&identity, self.0.learner).await {
            Ok(learner) => Ok(Placement {
                learner,
                primary: self.0.learner,
                primary_id: StoreId::LearnerStore,
                project: org_store,
                project_id: org_id,
                location: ProfileLocation::LearnerStore,
            }),
            Err(e) if unreachable(&e) && org_store.is_some() => {
                let local = org_store.expect("checked");
                Ok(Placement {
                    learner: self.resolve_in(&identity, local).await?,
                    primary: local,
                    primary_id: org_id.expect("an organisation store has an organisation"),
                    project: None,
                    project_id: None,
                    location: ProfileLocation::Local {
                        host: identity.host().unwrap_or_default().to_string(),
                    },
                })
            }
            Err(e) => Err(e),
        }
    }

    // --- lookups across the stores ----------------------------------------------

    /// What reading from the learner store gave: `None` if it is unreachable
    /// and there are organisation stores to look in instead (ADR-0003 D4).
    /// The one place that rule is written.
    fn tolerate<T>(&self, read: Result<T>) -> Result<Option<T>> {
        match read {
            Ok(value) => Ok(Some(value)),
            Err(e) if unreachable(&e) && !self.0.orgs.is_empty() => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// The first thing `lookup` finds, in the learner store and then the
    /// organisation stores, with the store it was found in.
    async fn first_in<T, F, Fut>(&self, lookup: F) -> Result<Option<(T, &'a S)>>
    where
        F: Fn(&'a S) -> Fut,
        Fut: Future<Output = Result<Option<T>>>,
    {
        if let Some(Some(found)) = self.tolerate(lookup(self.0.learner).await)? {
            return Ok(Some((found, self.0.learner)));
        }
        for store in self.0.orgs.values() {
            if let Some(found) = lookup(store).await? {
                return Ok(Some((found, store)));
            }
        }
        Ok(None)
    }

    /// The pending session `key` and the store it lives in. A session whose
    /// grading journal is stored is being graded, not pending.
    pub async fn find_session(&self, key: &SessionKey) -> Result<Option<(Session, &'a S)>> {
        // An unreachable learner store must not hide sessions kept in the
        // organisation stores (ADR-0003 D4).
        match self.first_in(|store| store.load_session(key)).await? {
            Some((_, store)) if store.load_grading(key).await?.is_some() => Ok(None),
            found => Ok(found),
        }
    }

    /// The record of the graded session `key`, if it was graded.
    pub async fn graded_record(&self, key: &SessionKey) -> Result<Option<GradedSession>> {
        Ok(self
            .first_in(|store| store.load_graded(key))
            .await?
            .map(|(graded, _)| graded))
    }

    /// Whether a grading journal of session `key` is stored.
    pub async fn grading_recorded(&self, key: &SessionKey) -> Result<bool> {
        Ok(self
            .first_in(|store| store.load_grading(key))
            .await?
            .is_some())
    }

    /// Replay durable feedback, including snapshots saved before a bank failure.
    pub async fn sync_feedback(&self, store: &S, asked: &AskedSession) -> Result<()> {
        for question in asked.questions() {
            if question.feedback.is_empty() {
                continue;
            }
            change_bank_entry(self.bank_of(store), question, |entry| {
                asked.update_bank_entry(question, entry);
            })
            .await?;
        }
        Ok(())
    }

    // --- recovery ------------------------------------------------------------

    /// Finish every grading left behind, e.g. by a crash or a failed write
    /// (ADR-0002 D6). Fails with the first error, after trying all.
    async fn recover(&self) -> Result<()> {
        let mut failed = None;
        for (i, store) in self.all().enumerate() {
            let read = store.gradings().await;
            // The learner store may be unreachable (ADR-0003 D4).
            let journals = match if i == 0 {
                self.tolerate(read)
            } else {
                read.map(Some)
            } {
                Ok(Some(journals)) => journals,
                Ok(None) => continue,
                Err(e) => {
                    failed.get_or_insert(e);
                    continue;
                }
            };
            for journal in &journals {
                if let Err(e) = self.apply(store, journal).await {
                    failed.get_or_insert(e);
                }
            }
            match store.asked_sessions().await {
                Ok(sessions) => {
                    for asked in sessions {
                        if let Err(e) = self.sync_feedback(store, &asked).await {
                            failed.get_or_insert(e);
                        }
                    }
                }
                Err(e) => {
                    failed.get_or_insert(e);
                }
            }
        }
        failed.map_or(Ok(()), Err)
    }
}

/// The profile of `learner` in `store`, or a new one.
pub(in crate::app) async fn load_or_new<S: StateStore>(
    store: &S,
    learner: &LearnerId,
) -> Result<LearnerProfile> {
    Ok(store
        .load_profile(learner)
        .await?
        .unwrap_or_else(|| LearnerProfile::new(learner.clone())))
}

/// A stable, non-reversible-at-a-glance ID: 64-bit FNV-1a of `text`. Not a
/// secret; it only keeps identities out of profile documents.
pub(in crate::app) fn opaque_id(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

pub(in crate::app) async fn change_bank_entry<S: StateStore>(
    bank: &S,
    asked: &AskedQuestion,
    change: impl Fn(&mut BankedQuestion),
) -> Result<()> {
    let Some(reference) = asked.bank() else {
        return Ok(());
    };
    for _ in 0..100 {
        let entries = bank.bank_questions(&asked.question.concept).await?;
        let Some(previous) = entries.iter().find(|e| *e.id() == reference.id) else {
            return Ok(());
        };
        let mut entry = previous.clone();
        change(&mut entry);
        if entry == *previous
            || bank
                .save_bank_question_if_current(&entry, Some(previous))
                .await?
        {
            return Ok(());
        }
    }
    Err(Error::Port("bank question kept changing".into()))
}
