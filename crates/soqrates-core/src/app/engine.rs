//! The Soqrates engine: orchestrates the aggregates (load, call, save,
//! emit events). Holds no domain rules (ADR-0002 D3).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::app::events::DomainEvent;
use crate::app::ports::{Clock, Examiner, StateStore};
use crate::assessment::{Answer, Concept, ConceptScope, Diff, Grade};
use crate::delivery::{Mode, Session};
use crate::ids::{AuthorId, ConceptId, Identity, LearnerId, QuestionId, SessionKey};
use crate::learner::{IdentityLink, LearnerProfile, Level, LinkCode, LinkRejection, policy};
use crate::{Error, Result};

/// The result of grading a session, as returned by [`Soqrates::submit`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionOutcome {
    /// One grade per question, in question order.
    pub grades: Vec<(QuestionId, Grade)>,
    pub passed: bool,
    pub level_after: Level,
    /// What happened, in order (ADR-0002 D7).
    pub events: Vec<DomainEvent>,
    /// Where the general reviews were stored (ADR-0003 D4).
    pub profile_location: ProfileLocation,
}

/// Where a learner's profile was read from and written to (ADR-0003 D4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProfileLocation {
    /// The shared learner store.
    LearnerStore,
    /// The learner store was unreachable; a profile local to this forge host
    /// was used instead.
    Local { host: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    /// Upper bound on questions per session, due reviews included.
    pub max_questions: usize,
    /// Fraction of correct answers needed to pass, in `0.0..=1.0`.
    pub pass_threshold: f64,
    pub mode: Mode,
    /// Forge host -> the organisation that administers it (ADR-0003 D2, D5).
    pub forges: BTreeMap<String, String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_questions: 3,
            pass_threshold: 0.6,
            mode: Mode::Advisory,
            forges: BTreeMap::new(),
        }
    }
}

pub struct Soqrates<S, E, C> {
    /// The learner store (ADR-0003 D4): profiles, links, link codes, sessions.
    pub store: S,
    /// Per-organisation stores for reviews of project concepts (ADR-0003 D5),
    /// and for profiles local to a forge when the learner store is
    /// unreachable (D4). Keyed by organisation.
    pub org_stores: BTreeMap<String, S>,
    pub examiner: E,
    pub clock: C,
    pub config: Config,
}

/// Where one learner's data lives for one interaction (ADR-0003 D4, D5).
struct Placement<'a, S> {
    learner: LearnerId,
    /// Holds the profile with general reviews (and links).
    primary: &'a S,
    /// Holds the profile with project reviews: the organisation's store.
    /// `None` when general and project reviews share `primary`.
    project: Option<&'a S>,
    location: ProfileLocation,
}

impl<S: StateStore, E: Examiner, C: Clock> Soqrates<S, E, C> {
    pub fn new(store: S, examiner: E, clock: C, config: Config) -> Self {
        Self {
            store,
            org_stores: BTreeMap::new(),
            examiner,
            clock,
            config,
        }
    }

    /// Add the store of `organisation` (ADR-0003 D5).
    pub fn with_org_store(mut self, organisation: impl Into<String>, store: S) -> Self {
        self.org_stores.insert(organisation.into(), store);
        self
    }

    // --- placement ----------------------------------------------------------

    /// The organisation administering `identity`'s forge, if configured.
    fn organisation_of(&self, identity: &Identity) -> Option<&str> {
        identity
            .host()
            .and_then(|h| self.config.forges.get(h))
            .map(String::as_str)
    }

    /// The store for project concepts and sessions of `identity`'s forge:
    /// its organisation's store. Without configured forges, everything lives
    /// in the learner store (a single-organisation deployment). A configured
    /// organisation without a store is a configuration error: project data
    /// must never fall into the shared learner store (ADR-0003 D5).
    fn org_store_for(&self, identity: &Identity) -> Result<Option<&S>> {
        match self.organisation_of(identity) {
            None => Ok(None),
            Some(org) => self
                .org_stores
                .get(org)
                .map(Some)
                .ok_or_else(|| Error::Invalid(format!("no store for organisation {org:?}"))),
        }
    }

    /// The store that holds sessions (question text, answers) for `author`:
    /// never the shared learner store when an organisation store exists.
    fn session_store_for(&self, author: &AuthorId) -> Result<&S> {
        Ok(self
            .org_store_for(&author.identity()?)?
            .unwrap_or(&self.store))
    }

    /// A learner ID for `identity` that `store` does not use yet.
    async fn fresh_learner_id(&self, identity: &Identity, store: &S) -> Result<LearnerId> {
        let mut n = 1;
        loop {
            let id = if n == 1 {
                LearnerId(format!("learner:{identity}"))
            } else {
                LearnerId(format!("learner:{identity}#{n}"))
            };
            if store.load_profile(&id).await?.is_none() && store.links_of(&id).await?.is_empty() {
                return Ok(id);
            }
            n += 1;
        }
    }

    /// Resolve `identity` in `store`, creating and linking a new learner the
    /// first time it is seen (ADR-0003 D1).
    async fn resolve_in(&self, identity: &Identity, store: &S) -> Result<LearnerId> {
        if let Some(learner) = store.learner_of(identity).await? {
            return Ok(learner);
        }
        let learner = self.fresh_learner_id(identity, store).await?;
        store
            .save_link(&IdentityLink {
                identity: identity.clone(),
                learner: learner.clone(),
                since: self.clock.now(),
            })
            .await?;
        store
            .save_profile(&LearnerProfile::new(learner.clone()))
            .await?;
        Ok(learner)
    }

    /// Where `author`'s data lives. Falls back to the organisation's store
    /// when the learner store is unreachable (ADR-0003 D4).
    async fn place(&self, author: &AuthorId) -> Result<Placement<'_, S>> {
        let identity = author.identity()?;
        let org_store = self.org_store_for(&identity)?;
        match self.resolve_in(&identity, &self.store).await {
            Ok(learner) => Ok(Placement {
                learner,
                primary: &self.store,
                project: org_store,
                location: ProfileLocation::LearnerStore,
            }),
            Err(Error::Port(_)) if org_store.is_some() => {
                let local = org_store.expect("checked");
                Ok(Placement {
                    learner: self.resolve_in(&identity, local).await?,
                    primary: local,
                    project: None,
                    location: ProfileLocation::Local {
                        host: identity.host().unwrap_or_default().to_string(),
                    },
                })
            }
            Err(e) => Err(e),
        }
    }

    async fn load_or_new(store: &S, learner: &LearnerId) -> Result<LearnerProfile> {
        Ok(store
            .load_profile(learner)
            .await?
            .unwrap_or_else(|| LearnerProfile::new(learner.clone())))
    }

    // --- profiles -----------------------------------------------------------

    /// The profile of `author`'s learner, or a fresh `Novice` profile if the
    /// learner is new (ADR-0001 D5, ADR-0003 D1).
    pub async fn profile(&self, author: &AuthorId) -> Result<LearnerProfile> {
        let place = self.place(author).await?;
        Self::load_or_new(place.primary, &place.learner).await
    }

    /// Due concepts with the scope of the store their card lives in, earliest
    /// due first, ties by concept ID (ADR-0002 D8, ADR-0003 D5).
    async fn due_with_scope(
        &self,
        place: &Placement<'_, S>,
    ) -> Result<Vec<(ConceptId, ConceptScope)>> {
        let now = self.clock.now();
        let mut due = Vec::new();
        let primary = Self::load_or_new(place.primary, &place.learner).await?;
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
            let profile = Self::load_or_new(project, &place.learner).await?;
            for c in profile.due_reviews(now) {
                let at = profile.card(&c).map(|k| k.due());
                due.push((at, c, ConceptScope::Project));
            }
        }
        due.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
        Ok(due.into_iter().map(|(_, c, s)| (c, s)).collect())
    }

    /// Concepts whose review is due for `author` (ADR-0001 D6, ADR-0002 D8).
    pub async fn due_reviews(&self, author: &AuthorId) -> Result<Vec<ConceptId>> {
        let place = self.place(author).await?;
        Ok(self
            .due_with_scope(&place)
            .await?
            .into_iter()
            .map(|(c, _)| c)
            .collect())
    }

    // --- sessions -----------------------------------------------------------

    /// Create and persist a session for `author` about `diff`: due reviews
    /// first, then new concepts from the diff, in the format the author's
    /// level calls for, at most `config.max_questions` questions (ADR-0001
    /// D6, D7; ADR-0002 D5, D6). An empty diff asks nothing (ADR-0001 D9).
    pub async fn start_session(
        &self,
        key: SessionKey,
        author: &AuthorId,
        diff: &Diff,
    ) -> Result<Session> {
        let now = self.clock.now();
        let mut concepts: Vec<Concept> = Vec::new();
        let mut level = Level::Novice;
        if !diff.is_empty() {
            let place = self.place(author).await?;
            level = Self::load_or_new(place.primary, &place.learner)
                .await?
                .level();
            for (id, scope) in self.due_with_scope(&place).await? {
                concepts.push(Concept {
                    title: id.0.clone(),
                    summary: String::new(),
                    id,
                    scope,
                });
            }
            for c in self.examiner.extract_concepts(diff).await? {
                if !concepts.iter().any(|k| k.id == c.id) {
                    concepts.push(c);
                }
            }
        }
        concepts.truncate(self.config.max_questions);

        let mut questions = Vec::with_capacity(concepts.len());
        for (i, concept) in concepts.iter().enumerate() {
            let mut q = self
                .examiner
                .write_question(concept, policy::format_for(level, i))
                .await?;
            q.scope = concept.scope;
            questions.push(q);
        }

        let session = Session::open(
            key,
            author.clone(),
            questions,
            self.config.max_questions,
            now,
        )?;
        self.session_store_for(author)?
            .save_session(&session)
            .await?;
        Ok(session)
    }

    /// The pending session `key` and the store it lives in.
    async fn find_session(&self, key: &SessionKey) -> Result<Option<(Session, &S)>> {
        if let Some(s) = self.store.load_session(key).await? {
            return Ok(Some((s, &self.store)));
        }
        for store in self.org_stores.values() {
            if let Some(s) = store.load_session(key).await? {
                return Ok(Some((s, store)));
            }
        }
        Ok(None)
    }

    /// The pending (not yet graded) session `key`, if any.
    pub async fn pending_session(&self, key: &SessionKey) -> Result<Option<Session>> {
        Ok(self.find_session(key).await?.map(|(s, _)| s))
    }

    /// Grade `answers` for the session `key`, record each review on the
    /// author's profile, persist it and remove the session. Fails with
    /// `NotFound` for an unknown or already graded session and `Invalid`
    /// for answers that don't fit the session (ADR-0002 D6).
    pub async fn submit(&self, key: &SessionKey, answers: &[Answer]) -> Result<SessionOutcome> {
        let now = self.clock.now();
        let (session, session_store) = self
            .find_session(key)
            .await?
            .ok_or_else(|| Error::NotFound(format!("session {:?}", key.0)))?;
        session.validate_answers(answers)?;

        let mut grades = Vec::new();
        for q in session.questions() {
            if let Some(a) = answers.iter().find(|a| a.question == q.id) {
                grades.push((q.id.clone(), self.examiner.grade(q, a).await?));
            }
        }
        let author = session.author().clone();
        let questions = session.questions().to_vec();
        let result = session.conclude(grades, self.config.pass_threshold, now);

        let place = self.place(&author).await?;
        let identity = author.identity()?;
        let mut primary = Self::load_or_new(place.primary, &place.learner).await?;
        let mut project = match place.project {
            Some(store) => Some(Self::load_or_new(store, &place.learner).await?),
            None => None,
        };
        let mut events: Vec<DomainEvent> = Vec::new();
        for (q, (_, grade)) in questions.iter().zip(&result.grades) {
            let profile = match (&mut project, q.scope) {
                (Some(p), ConceptScope::Project) => p,
                _ => &mut primary,
            };
            events.extend(
                profile
                    .record(&q.concept, q.format(), *grade, now, &identity)
                    .into_iter()
                    .map(DomainEvent::from),
            );
        }
        events.push(result.event.into());

        place.primary.save_profile(&primary).await?;
        if let (Some(store), Some(p)) = (place.project, &project) {
            store.save_profile(p).await?;
        }
        session_store.delete_session(key).await?;

        Ok(SessionOutcome {
            grades: result.grades,
            passed: result.passed,
            level_after: primary.level(),
            events,
            profile_location: place.location,
        })
    }

    // --- identities (ADR-0003) ----------------------------------------------

    /// Every store that may hold data about learners.
    fn all_stores(&self) -> impl Iterator<Item = &S> {
        std::iter::once(&self.store).chain(self.org_stores.values())
    }

    /// The learner behind `author`'s identity. The first time an identity is
    /// seen, a new learner is created and linked to it (D1).
    pub async fn learner_of(&self, author: &AuthorId) -> Result<LearnerId> {
        Ok(self.place(author).await?.learner)
    }

    /// Issue the link code `code` for `author`'s learner (D2). The caller
    /// supplies an unguessable `code`, e.g. from the OS random source.
    pub async fn request_link_code(&self, author: &AuthorId, code: String) -> Result<LinkCode> {
        let identity = author.identity()?;
        let learner = self.resolve_in(&identity, &self.store).await?;
        let code = LinkCode::issue(code, identity, learner, self.clock.now());
        self.store.save_link_code(&code).await?;
        Ok(code)
    }

    /// Present `code` as `author`: links `author`'s identity to the learner
    /// that requested the code, merging the two learners (D2, D3). Fails
    /// with [`Error::Link`] when the code is rejected.
    pub async fn present_link_code(&self, author: &AuthorId, code: &str) -> Result<LearnerId> {
        let presenter = author.identity()?;
        let mut link_code = self
            .store
            .load_link_code(code)
            .await?
            .ok_or(LinkRejection::UnknownCode)?;
        link_code.redeem(&presenter, self.clock.now())?;
        self.store.save_link_code(&link_code).await?;
        let target = self
            .store
            .learner_of(link_code.requested_by())
            .await?
            .unwrap_or_else(|| link_code.learner().clone());
        self.link_into(&target, &presenter).await
    }

    /// An administrator of `organisation` declares that `a` and `b` are the
    /// same learner. Both hosts must be administered by `organisation` (D2).
    pub async fn declare_link(
        &self,
        organisation: &str,
        a: &AuthorId,
        b: &AuthorId,
    ) -> Result<LearnerId> {
        let (a, b) = (a.identity()?, b.identity()?);
        for identity in [&a, &b] {
            if self.organisation_of(identity) != Some(organisation) {
                return Err(LinkRejection::NotAdministered {
                    host: identity.host().unwrap_or("e-mail").to_string(),
                    organisation: organisation.to_string(),
                }
                .into());
            }
        }
        let target = self.resolve_in(&a, &self.store).await?;
        self.link_into(&target, &b).await
    }

    /// Link `identity` to `target`, merging its learner in if that learner
    /// has no other identities; reject otherwise (D2, D3).
    async fn link_into(&self, target: &LearnerId, identity: &Identity) -> Result<LearnerId> {
        let other = self.resolve_in(identity, &self.store).await?;
        if &other == target {
            return Ok(other);
        }
        if self.store.links_of(&other).await?.len() > 1 {
            return Err(LinkRejection::BelongsToAnotherLearner(identity.clone()).into());
        }
        for store in self.all_stores() {
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
        self.store
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
        let identity = author.identity()?;
        let learner = self.resolve_in(&identity, &self.store).await?;
        if self.store.links_of(&learner).await?.len() <= 1 {
            return Ok(learner);
        }
        let new_learner = self.fresh_learner_id(&identity, &self.store).await?;
        for store in self.all_stores() {
            if let Some(profile) = store.load_profile(&learner).await? {
                let (remaining, split_off) = profile.split(&identity, new_learner.clone());
                store.save_profile(&remaining).await?;
                store.save_profile(&split_off).await?;
            }
        }
        self.store
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
        let learner = self.resolve_in(&author.identity()?, &self.store).await?;
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
        let learner = self.resolve_in(&author.identity()?, &self.store).await?;
        for store in self.all_stores() {
            store.delete_learner(&learner).await?;
        }
        Ok(())
    }
}
