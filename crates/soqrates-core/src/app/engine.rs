//! The Soqrates engine: orchestrates the aggregates (load, call, save,
//! emit events). Holds no domain rules (ADR-0002 D3).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::app::events::DomainEvent;
use crate::app::ports::{Clock, Examiner, StateStore};
use crate::assessment::{
    Answer, AnswerBody, Concept, ConceptScope, Diff, Grade, Question, QuestionBody,
};
use crate::delivery::{GradedSession, Mode, PrRef, PrRounds, QuestionResult, Session};
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
    /// Per-question feedback, in question order (ADR-0004 D1).
    pub results: Vec<QuestionResult>,
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

    /// An opaque learner ID, derived from `identity` but not revealing it
    /// (ADR-0003 D1), that `store` does not use yet.
    async fn fresh_learner_id(&self, identity: &Identity, store: &S) -> Result<LearnerId> {
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
        let questions = self
            .compose_questions(author, diff, &BTreeSet::new())
            .await?;
        self.open_session(key, author, questions).await
    }

    /// The questions a session for `author` about `diff` asks, as in
    /// [`Soqrates::start_session`], leaving out new concepts in `skip`.
    /// Concepts in `skip` whose review is due are still asked (ADR-0002 D9).
    /// Persists nothing.
    pub async fn compose_questions(
        &self,
        author: &AuthorId,
        diff: &Diff,
        skip: &BTreeSet<ConceptId>,
    ) -> Result<Vec<Question>> {
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
                if !skip.contains(&c.id) && !concepts.iter().any(|k| k.id == c.id) {
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
        Ok(questions)
    }

    /// Open and persist the session `key` for `author` with `questions`.
    pub async fn open_session(
        &self,
        key: SessionKey,
        author: &AuthorId,
        questions: Vec<Question>,
    ) -> Result<Session> {
        let session = Session::open(
            key,
            author.clone(),
            questions,
            self.config.max_questions,
            self.clock.now(),
        )?;
        self.session_store_for(author)?
            .save_session(&session)
            .await?;
        Ok(session)
    }

    /// The rounds asked so far on `pr`, authored by `author` (ADR-0002 D9).
    /// Kept with the author's sessions, never in the shared learner store
    /// when an organisation store exists (ADR-0003 D5).
    pub async fn rounds(&self, pr: &PrRef, author: &AuthorId) -> Result<PrRounds> {
        Ok(self
            .session_store_for(author)?
            .load_rounds(pr)
            .await?
            .unwrap_or_else(|| PrRounds::new(pr.clone())))
    }

    /// Persist `rounds` of a pull request by `author`.
    pub async fn save_rounds(&self, author: &AuthorId, rounds: &PrRounds) -> Result<()> {
        self.session_store_for(author)?.save_rounds(rounds).await
    }

    /// The pending session `key` and the store it lives in.
    async fn find_session(&self, key: &SessionKey) -> Result<Option<(Session, &S)>> {
        // An unreachable learner store must not hide sessions kept in the
        // organisation stores (ADR-0003 D4).
        match self.store.load_session(key).await {
            Ok(Some(s)) => return Ok(Some((s, &self.store))),
            Ok(None) => {}
            Err(Error::Port(_)) if !self.org_stores.is_empty() => {}
            Err(e) => return Err(e),
        }
        for store in self.org_stores.values() {
            if let Some(s) = store.load_session(key).await? {
                return Ok(Some((s, store)));
            }
        }
        Ok(None)
    }

    /// Remove the pending session `key` without grading it, e.g. when its
    /// questions could not be posted.
    pub async fn discard_session(&self, key: &SessionKey) -> Result<()> {
        if let Some((_, store)) = self.find_session(key).await? {
            store.delete_session(key).await?;
        }
        Ok(())
    }

    /// The record of the graded session `key`, if it was graded.
    pub async fn graded(&self, key: &SessionKey) -> Result<Option<GradedSession>> {
        match self.store.load_graded(key).await {
            Ok(Some(g)) => return Ok(Some(g)),
            Ok(None) => {}
            Err(Error::Port(_)) if !self.org_stores.is_empty() => {}
            Err(e) => return Err(e),
        }
        for store in self.org_stores.values() {
            if let Some(g) = store.load_graded(key).await? {
                return Ok(Some(g));
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
        // Claim the session atomically: of several concurrent submissions,
        // exactly one gets it (ADR-0002 D6).
        let (session, session_store) = self
            .take_session(key)
            .await?
            .ok_or_else(|| Error::NotFound(format!("session {:?}", key.0)))?;
        let pending = session.clone();
        let outcome = self
            .grade_claimed(key, session, session_store, answers)
            .await;
        if outcome.is_err() {
            // Nothing was recorded: give the session back so the author can
            // answer again.
            let _ = session_store.save_session(&pending).await;
        }
        outcome
    }

    /// Atomically remove the pending session `key` from whichever store
    /// holds it.
    async fn take_session(&self, key: &SessionKey) -> Result<Option<(Session, &S)>> {
        match self.store.take_session(key).await {
            Ok(Some(s)) => return Ok(Some((s, &self.store))),
            Ok(None) => {}
            Err(Error::Port(_)) if !self.org_stores.is_empty() => {}
            Err(e) => return Err(e),
        }
        for store in self.org_stores.values() {
            if let Some(s) = store.take_session(key).await? {
                return Ok(Some((s, store)));
            }
        }
        Ok(None)
    }

    /// Grade a session this caller has claimed. On error nothing stays
    /// recorded; the caller restores the session.
    async fn grade_claimed(
        &self,
        key: &SessionKey,
        session: Session,
        session_store: &S,
        answers: &[Answer],
    ) -> Result<SessionOutcome> {
        let now = self.clock.now();
        session.validate_answers(answers)?;

        let mut grades = Vec::new();
        let mut feedback: Vec<(QuestionId, String)> = Vec::new();
        for q in session.questions() {
            if let Some(a) = answers.iter().find(|a| a.question == q.id) {
                let grading = self.examiner.grade(q, a).await?;
                grades.push((q.id.clone(), grading.grade));
                feedback.push((q.id.clone(), grading.feedback));
            }
        }
        let author = session.author().clone();
        let questions = session.questions().to_vec();
        let result = session.conclude(grades, self.config.pass_threshold, now);

        // Prepare every write before making any, so that a failure part-way
        // can be rolled back.
        let place = self.place(&author).await?;
        let identity = author.identity()?;
        let original_primary = Self::load_or_new(place.primary, &place.learner).await?;
        let original_project = match place.project {
            Some(store) => Some(Self::load_or_new(store, &place.learner).await?),
            None => None,
        };
        let mut primary = original_primary.clone();
        let mut project = original_project.clone();
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
        let results = question_results(&questions, answers, &result.grades, &feedback, &events);
        let correct = result.grades.iter().filter(|(_, g)| g.is_correct()).count();
        let graded = GradedSession {
            key: key.clone(),
            author: author.clone(),
            correct,
            total: result.grades.len(),
            passed: result.passed,
            at: now,
        };

        // Record that the session was graded, then save the reviews. If any
        // write fails, undo them all (ADR-0002 D6).
        let committed = async {
            session_store.save_graded(&graded).await?;
            place.primary.save_profile(&primary).await?;
            if let (Some(store), Some(p)) = (place.project, &project) {
                store.save_profile(p).await?;
            }
            Ok::<(), Error>(())
        }
        .await;
        if let Err(e) = committed {
            let _ = place.primary.save_profile(&original_primary).await;
            if let (Some(store), Some(p)) = (place.project, &original_project) {
                let _ = store.save_profile(p).await;
            }
            let _ = session_store.delete_graded(key).await;
            return Err(e);
        }

        Ok(SessionOutcome {
            grades: result.grades,
            passed: result.passed,
            level_after: primary.level(),
            events,
            profile_location: place.location,
            results,
        })
    }

    // --- identities (ADR-0003) ----------------------------------------------

    /// Every store that may hold data about learners.
    fn all_stores(&self) -> impl Iterator<Item = &S> {
        std::iter::once(&self.store).chain(self.org_stores.values())
    }

    /// Where `author`'s profile lives right now (ADR-0003 D4).
    pub async fn profile_location(&self, author: &AuthorId) -> Result<ProfileLocation> {
        Ok(self.place(author).await?.location)
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

/// Per-question results for the author (ADR-0004 D1): the answer, the
/// correct option, feedback, and when the concept is next due.
fn question_results(
    questions: &[crate::assessment::Question],
    answers: &[Answer],
    grades: &[(QuestionId, Grade)],
    feedback: &[(QuestionId, String)],
    events: &[DomainEvent],
) -> Vec<QuestionResult> {
    questions
        .iter()
        .zip(grades)
        .map(|(q, (_, grade))| {
            let (prompt, options, correct_option) = match &q.body {
                QuestionBody::MultipleChoice {
                    prompt,
                    options,
                    correct,
                } => (prompt.clone(), options.clone(), Some(*correct)),
                QuestionBody::Open { prompt, .. } => (prompt.clone(), Vec::new(), None),
            };
            let answer: Option<AnswerBody> = answers
                .iter()
                .find(|a| a.question == q.id)
                .map(|a| a.body.clone());
            let feedback = feedback
                .iter()
                .find(|(id, _)| *id == q.id)
                .map(|(_, f)| f.clone())
                .filter(|f| !f.trim().is_empty())
                .unwrap_or_else(|| {
                    if answer.is_none() {
                        format!("Not answered. {}", q.explanation)
                    } else {
                        q.explanation.clone()
                    }
                });
            let next_review = events.iter().rev().find_map(|e| match e {
                DomainEvent::Learner(crate::learner::LearnerEvent::ReviewScheduled {
                    concept,
                    due,
                    ..
                }) if *concept == q.concept => Some(*due),
                _ => None,
            });
            QuestionResult {
                question: q.id.clone(),
                concept: q.concept.clone(),
                prompt,
                options,
                correct_option,
                answer,
                grade: *grade,
                feedback,
                next_review,
            }
        })
        .collect()
}

/// A stable, non-reversible-at-a-glance ID: 64-bit FNV-1a of `text`. Not a
/// secret; it only keeps identities out of profile documents.
fn opaque_id(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}
