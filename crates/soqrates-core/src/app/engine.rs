//! The Soqrates engine: orchestrates the aggregates (load, call, save,
//! emit events). Holds no domain rules (ADR-0002 D3).

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::app::events::DomainEvent;
use crate::app::ports::{Clock, Codebase, Examiner, StateStore};
use crate::assessment::{
    Answer, AnswerBody, Concept, ConceptScope, Diff, Grade, Question, QuestionBody, QuestionBrief,
    QuestionKind, Weight,
};
use crate::delivery::{
    BankedQuestion, GradedSession, GradingJournal, JournaledReview, Mode, PrRef, PrRounds,
    QuestionResult, SeenQuestions, Session, StoreId, pick,
};
use crate::ids::{AuthorId, BankId, ConceptId, Identity, LearnerId, QuestionId, SessionKey};
use crate::learner::{
    GradedAnswer, IdentityLink, LearnerEvent, LearnerProfile, Level, LinkCode, LinkRejection,
    policy,
};
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

/// What recording the reviews of one grading did to the profiles.
#[derive(Debug, Default)]
struct Recorded {
    /// The events of each review, indexed like `GradingJournal::reviews`.
    by_review: BTreeMap<usize, Vec<LearnerEvent>>,
    /// The level of each profile after the reviews.
    levels: Vec<(StoreId, Level)>,
}

impl Recorded {
    /// Add the `events` of the reviews at `indices`, one entry per review.
    fn add(&mut self, indices: &[usize], events: Vec<Vec<LearnerEvent>>) {
        self.by_review.extend(indices.iter().copied().zip(events));
    }

    /// The events of all `count` reviews, in review order.
    fn events_in_order(&self, count: usize) -> Vec<DomainEvent> {
        (0..count)
            .filter_map(|i| self.by_review.get(&i))
            .flatten()
            .cloned()
            .map(DomainEvent::from)
            .collect()
    }

    fn level_of(&self, store: &StoreId) -> Option<Level> {
        self.levels
            .iter()
            .find(|(id, _)| id == store)
            .map(|(_, l)| *l)
    }
}

impl SessionOutcome {
    /// Replace what was worked out at grading time by what `recorded`
    /// says happened: the level of the profile in `primary`, the review
    /// events and the next review dates. The grades and the session's own
    /// event stay.
    fn correct_by(&mut self, recorded: Recorded, primary: &StoreId) {
        let concluded = self.events.pop();
        let count = recorded.by_review.keys().max().map_or(0, |last| last + 1);
        self.events = recorded.events_in_order(count);
        self.events.extend(concluded);
        if let Some(level) = recorded.level_of(primary) {
            self.level_after = level;
        }
        for r in &mut self.results {
            r.next_review = next_review_of(&self.events, &r.concept);
        }
    }
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
    /// Upper bound on questions per session, due reviews included. A cap,
    /// not a target (ADR-0002 D10).
    pub max_questions: usize,
    /// Fraction of correct answers needed to pass, in `0.0..=1.0`.
    pub pass_threshold: f64,
    pub mode: Mode,
    /// Whether experts get open questions (ADR-0002 D5). Off: everyone
    /// gets multiple choice.
    #[serde(default)]
    pub open_questions: bool,
    /// Forge host -> the organisation that administers it (ADR-0003 D2, D5).
    pub forges: BTreeMap<String, String>,
    /// Whether rounds reuse questions from the question bank (ADR-0009).
    #[serde(default = "default_true")]
    pub question_bank: bool,
    /// A learner is shown a bank question at most once within this many
    /// rounds (ADR-0009 D5).
    #[serde(default = "default_reask_after_rounds")]
    pub reask_after_rounds: u64,
}

fn default_true() -> bool {
    true
}

fn default_reask_after_rounds() -> u64 {
    10
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_questions: 5,
            pass_threshold: 0.6,
            mode: Mode::Advisory,
            open_questions: false,
            forges: BTreeMap::new(),
            question_bank: true,
            reask_after_rounds: default_reask_after_rounds(),
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
    /// A question bank shared across organisations, if the deployment has
    /// one; otherwise each organisation's store holds its own (ADR-0009 D2).
    pub shared_bank: Option<S>,
    pub examiner: E,
    pub clock: C,
    pub config: Config,
}

/// Where one learner's data lives for one interaction (ADR-0003 D4, D5).
struct Placement<'a, S> {
    learner: LearnerId,
    /// Holds the profile with general reviews (and links).
    primary: &'a S,
    primary_id: StoreId,
    /// Holds the profile with project reviews: the organisation's store.
    /// `None` when general and project reviews share `primary`.
    project: Option<&'a S>,
    project_id: Option<StoreId>,
    location: ProfileLocation,
}

impl<S: StateStore, E: Examiner, C: Clock> Soqrates<S, E, C> {
    pub fn new(store: S, examiner: E, clock: C, config: Config) -> Self {
        Self {
            store,
            org_stores: BTreeMap::new(),
            shared_bank: None,
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

    /// Keep the question bank in `store`, shared across organisations,
    /// instead of in each organisation's store (ADR-0009 D2).
    pub fn with_shared_bank(mut self, store: S) -> Self {
        self.shared_bank = Some(store);
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

    /// The store holding the question bank for `author`'s rounds (ADR-0009
    /// D2): the shared bank if there is one, else the store of `author`'s
    /// sessions. Never the shared learner store: with organisation stores
    /// configured, an author outside every organisation gets no bank.
    /// `None` when the bank is off.
    fn bank_store_for(&self, author: &AuthorId) -> Result<Option<&S>> {
        if !self.config.question_bank {
            return Ok(None);
        }
        if let Some(shared) = &self.shared_bank {
            return Ok(Some(shared));
        }
        match self.org_store_for(&author.identity()?)? {
            Some(store) => Ok(Some(store)),
            None if self.org_stores.is_empty() => Ok(Some(&self.store)),
            None => Ok(None),
        }
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
        let org_id = self
            .organisation_of(&identity)
            .map(|org| StoreId::Organisation(org.to_string()));
        match self.resolve_in(&identity, &self.store).await {
            Ok(learner) => Ok(Placement {
                learner,
                primary: &self.store,
                primary_id: StoreId::LearnerStore,
                project: org_store,
                project_id: org_id,
                location: ProfileLocation::LearnerStore,
            }),
            Err(Error::Port(_)) if org_store.is_some() => {
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
        self.recover().await?;
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
        self.recover().await?;
        let place = self.place(author).await?;
        Ok(self
            .due_with_scope(&place)
            .await?
            .into_iter()
            .map(|(c, _)| c)
            .collect())
    }

    // --- sessions -----------------------------------------------------------

    /// Create and persist a session for `author` about `diff`, sized to the
    /// change and the author's knowledge (ADR-0002 D10): due reviews and new
    /// concepts from the diff, in the format the author's level calls for,
    /// at most `config.max_questions` questions (ADR-0001 D6, D7; ADR-0002
    /// D5, D6). An empty diff asks nothing (ADR-0001 D9).
    pub async fn start_session(
        &self,
        key: SessionKey,
        author: &AuthorId,
        diff: &Diff,
        code: &impl Codebase,
    ) -> Result<Session> {
        let questions = self
            .compose_questions(author, diff, &BTreeSet::new(), &[], code)
            .await?;
        self.open_session(key, author, questions, 0).await
    }

    /// The questions a session for `author` about `diff` asks (ADR-0002
    /// D10): first `carried`, the unanswered questions of an earlier round
    /// (D9), as written; then due reviews and new concepts, sharing the
    /// remaining slots. New concepts in `skip`, or that the author knows and
    /// isn't due to review, are left out, as are incidental ones; concepts
    /// in `skip` whose review is due are still asked. The examiner may read
    /// `code`, the codebase of the change (ADR-0008 D2).
    ///
    /// Each question comes from the question bank if it has one that suits
    /// and that the author may be shown; only otherwise is one written, and
    /// banked if it may be (ADR-0009 D4). Persists the bank and what the
    /// author was shown (D5), nothing else.
    pub async fn compose_questions(
        &self,
        author: &AuthorId,
        diff: &Diff,
        skip: &BTreeSet<ConceptId>,
        carried: &[Question],
        code: &impl Codebase,
    ) -> Result<Vec<Question>> {
        self.recover().await?;
        let mut questions = carried.to_vec();
        if diff.is_empty() {
            return Ok(questions);
        }
        let open = self.config.max_questions.saturating_sub(carried.len());
        if open == 0 {
            return Ok(questions);
        }
        let place = self.place(author).await?;
        let level = Self::load_or_new(place.primary, &place.learner)
            .await?
            .level();
        let taken = |id: &ConceptId| carried.iter().any(|q| q.concept == *id);
        let reviews: Vec<Concept> = self
            .due_with_scope(&place)
            .await?
            .into_iter()
            .filter(|(id, _)| !taken(id))
            .map(|(id, scope)| Concept {
                title: id.0.clone(),
                summary: String::new(),
                id,
                scope,
                weight: Weight::Core,
                locations: Vec::new(),
            })
            .collect();
        let known = self.known_concepts(&place).await?;
        let mut new: Vec<Concept> = Vec::new();
        for c in self.examiner.extract_concepts(diff, code).await? {
            let asked = taken(&c.id)
                || reviews.iter().any(|k| k.id == c.id)
                || new.iter().any(|k| k.id == c.id);
            if c.weight != Weight::Incidental
                && !asked
                && !skip.contains(&c.id)
                && !known.contains(&c.id)
            {
                new.push(c);
            }
        }
        // Core before supporting, each in the examiner's order.
        new.sort_by_key(|c| c.weight);

        let (n_reviews, n_new) = policy::split_slots(open, reviews.len(), new.len());
        let concepts = reviews
            .into_iter()
            .take(n_reviews)
            .chain(new.into_iter().take(n_new));
        let mut bank = match self.bank_store_for(author)? {
            Some(store) => {
                let seen = store
                    .load_seen(&place.learner)
                    .await?
                    .unwrap_or_else(|| SeenQuestions::new(place.learner.clone()));
                Some((store, seen))
            }
            None => None,
        };
        let round = bank.as_ref().map_or(0, |(_, seen)| seen.next_round());
        for concept in concepts {
            // The format follows the level (ADR-0002 D5); the kind is the
            // examiner's choice among those the level allows, and the
            // question must be what was asked for (ADR-0007 D2, D4).
            let format = policy::format_for(level, self.config.open_questions);
            let asked: Vec<QuestionKind> = questions.iter().map(|q| q.kind).collect();
            let brief = QuestionBrief::new(
                format,
                &policy::kinds_for(level, format, &asked),
                diff.hunks_at(&concept.locations),
            );
            let q = match &mut bank {
                Some((store, seen)) => {
                    let in_round: Vec<BankId> = questions
                        .iter()
                        .filter_map(|q| q.banked.as_ref().map(|b| b.id.clone()))
                        .collect();
                    self.banked_or_written(
                        *store, seen, round, &in_round, &concept, &brief, level, code,
                    )
                    .await?
                }
                None => self.written(&concept, &brief, code).await?,
            };
            questions.push(q);
        }
        if let Some((store, mut seen)) = bank {
            seen.end_round(round);
            store.save_seen(&seen).await?;
        }
        Ok(questions)
    }

    /// A question about `concept` that the examiner writes as `brief` asks,
    /// checked to be what was asked for (ADR-0007 D2, D4).
    async fn written(
        &self,
        concept: &Concept,
        brief: &QuestionBrief,
        code: &impl Codebase,
    ) -> Result<Question> {
        let mut q = self.examiner.write_question(concept, brief, code).await?;
        brief
            .check(&q)
            .map_err(|why| Error::Port(format!("examiner: {why}")))?;
        q.scope = concept.scope;
        Ok(q)
    }

    /// A question about `concept` for round `round`: from the bank in
    /// `store` if one suits `brief` and the learner may be shown it (not one
    /// of `in_round`), else written and banked if it may be (ADR-0009 D3,
    /// D4). Records in `seen` that the learner was shown it (D5).
    #[allow(clippy::too_many_arguments)]
    async fn banked_or_written(
        &self,
        store: &S,
        seen: &mut SeenQuestions,
        round: u64,
        in_round: &[BankId],
        concept: &Concept,
        brief: &QuestionBrief,
        level: Level,
        code: &impl Codebase,
    ) -> Result<Question> {
        let mut bank = store.bank_questions(&concept.id).await?;
        let gap = self.config.reask_after_rounds;
        if let Some(i) = pick(&bank, brief, seen, round, gap, in_round) {
            let entry = &mut bank[i];
            // Asked as it is: the stored question, in this concept's scope.
            let mut q = entry.take();
            q.scope = concept.scope;
            store.save_bank_question(entry).await?;
            seen.show(entry.id(), round);
            return Ok(q);
        }
        let mut q = self.written(concept, brief, code).await?;
        let Some(new) = BankedQuestion::new(&q, level, self.clock.now()) else {
            return Ok(q);
        };
        // The same question written again is banked once.
        let entry = match bank.iter_mut().find(|e| e.id() == new.id()) {
            Some(existing) if existing.is_retired() => return Ok(q),
            Some(existing) => {
                existing.take();
                existing.clone()
            }
            None => new,
        };
        store.save_bank_question(&entry).await?;
        seen.show(entry.id(), round);
        q.banked = Some(entry.reference());
        Ok(q)
    }

    /// Concepts `place`'s learner has a review card for, in either store,
    /// that isn't due: the author knows them for now (ADR-0002 D10).
    async fn known_concepts(&self, place: &Placement<'_, S>) -> Result<BTreeSet<ConceptId>> {
        let now = self.clock.now();
        let mut known = BTreeSet::new();
        let mut profiles = vec![Self::load_or_new(place.primary, &place.learner).await?];
        if let Some(project) = place.project {
            profiles.push(Self::load_or_new(project, &place.learner).await?);
        }
        for profile in &profiles {
            for (concept, card) in profile.cards() {
                if !card.is_due(now) {
                    known.insert(concept.clone());
                }
            }
        }
        // A card that is due in one store is due: don't count it as known.
        for profile in &profiles {
            for concept in profile.due_reviews(now) {
                known.remove(&concept);
            }
        }
        Ok(known)
    }

    /// Open and persist the session `key` for `author` with `questions`,
    /// the first `carried` of which are carried from an earlier round
    /// (ADR-0002 D9). At most `config.max_questions` questions, or
    /// `carried` if that is more: carried questions are never dropped, even
    /// if the cap was lowered since they were asked (ADR-0002 D6, D10).
    pub async fn open_session(
        &self,
        key: SessionKey,
        author: &AuthorId,
        questions: Vec<Question>,
        carried: usize,
    ) -> Result<Session> {
        self.recover().await?;
        let cap = self.config.max_questions.max(carried);
        let session = Session::open(key, author.clone(), questions, cap, self.clock.now())?;
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

    /// The pending session `key` and the store it lives in. A session whose
    /// grading journal is stored is being graded, not pending.
    async fn find_session(&self, key: &SessionKey) -> Result<Option<(Session, &S)>> {
        // An unreachable learner store must not hide sessions kept in the
        // organisation stores (ADR-0003 D4).
        let mut found = None;
        match self.store.load_session(key).await {
            Ok(Some(s)) => found = Some((s, &self.store)),
            Ok(None) => {}
            Err(Error::Port(_)) if !self.org_stores.is_empty() => {}
            Err(e) => return Err(e),
        }
        if found.is_none() {
            for store in self.org_stores.values() {
                if let Some(s) = store.load_session(key).await? {
                    found = Some((s, store));
                    break;
                }
            }
        }
        match found {
            Some((_, store)) if store.load_grading(key).await?.is_some() => Ok(None),
            found => Ok(found),
        }
    }

    /// Remove the pending session `key` without grading it, e.g. when its
    /// questions could not be posted.
    pub async fn discard_session(&self, key: &SessionKey) -> Result<()> {
        let _ = self.recover().await;
        if let Some((_, store)) = self.find_session(key).await? {
            store.delete_session(key).await?;
        }
        Ok(())
    }

    /// The record of the graded session `key`, if it was graded.
    pub async fn graded(&self, key: &SessionKey) -> Result<Option<GradedSession>> {
        // Best effort: a grading that can't be finished yet is reported as
        // being graded (ADR-0002 D6).
        let _ = self.recover().await;
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
        let _ = self.recover().await;
        Ok(self.find_session(key).await?.map(|(s, _)| s))
    }

    /// Whether the session `key` is being graded: its answers were graded,
    /// but not all of the results are recorded yet (ADR-0002 D6).
    pub async fn being_graded(&self, key: &SessionKey) -> Result<bool> {
        let _ = self.recover().await;
        match self.store.load_grading(key).await {
            Ok(Some(_)) => return Ok(true),
            Ok(None) => {}
            Err(Error::Port(_)) if !self.org_stores.is_empty() => {}
            Err(e) => return Err(e),
        }
        for store in self.org_stores.values() {
            if store.load_grading(key).await?.is_some() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Grade `answers` for the session `key`, record each review on the
    /// author's profile, persist it and remove the session. Fails with
    /// `NotFound` for an unknown or already graded session and `Invalid`
    /// for answers that don't fit the session (ADR-0002 D6).
    ///
    /// Grading is written ahead: claiming the session and journaling the
    /// reviews to record are one atomic step, then the journal is applied.
    /// If applying fails, the journal stays and a later operation finishes
    /// it.
    ///
    /// The grader may read `code`, the codebase the session was asked
    /// about (ADR-0008 D4).
    pub async fn submit(
        &self,
        key: &SessionKey,
        answers: &[Answer],
        code: &impl Codebase,
    ) -> Result<SessionOutcome> {
        self.recover().await?;
        let not_found = || Error::NotFound(format!("session {:?}", key.0));
        let (session, session_store) = self.find_session(key).await?.ok_or_else(not_found)?;
        let (journal, mut outcome, primary) = self.grade(session, answers, code).await?;
        // Of several concurrent submissions, exactly one claims the session.
        if !session_store.begin_grading(&journal).await? {
            return Err(not_found());
        }
        if let Some(recorded) = self.apply(session_store, &journal).await? {
            // Applying recorded the reviews on the profiles as they are now,
            // which may differ from the ones grading saw if another session
            // of the learner was graded meanwhile: report what happened.
            outcome.correct_by(recorded, &primary);
        }
        Ok(outcome)
    }

    /// Grade `answers` to `session`: the journal of everything grading
    /// writes, the outcome as the profiles look now, and the store of the
    /// general reviews. Writes no reviews.
    async fn grade(
        &self,
        session: Session,
        answers: &[Answer],
        code: &impl Codebase,
    ) -> Result<(GradingJournal, SessionOutcome, StoreId)> {
        let now = self.clock.now();
        session.validate_answers(answers)?;

        let mut grades = Vec::new();
        let mut feedback: Vec<(QuestionId, String)> = Vec::new();
        for q in session.questions() {
            if let Some(a) = answers.iter().find(|a| a.question == q.id) {
                let grading = self.examiner.grade(q, a, code).await?;
                grades.push((q.id.clone(), grading.grade));
                feedback.push((q.id.clone(), grading.feedback));
            }
        }
        let claimed = session.clone();
        let key = session.key().clone();
        let author = session.author().clone();
        let questions = session.questions().to_vec();
        let result = session.conclude(grades, self.config.pass_threshold, now);

        let place = self.place(&author).await?;
        let identity = author.identity()?;
        let primary = Self::load_or_new(place.primary, &place.learner).await?;
        let project = match place.project {
            Some(store) => Some(Self::load_or_new(store, &place.learner).await?),
            None => None,
        };
        let reviews: Vec<JournaledReview> = questions
            .iter()
            .zip(&result.grades)
            .map(|(q, (_, grade))| {
                let store = match (&place.project_id, q.scope) {
                    (Some(id), ConceptScope::Project) => id.clone(),
                    _ => place.primary_id.clone(),
                };
                JournaledReview {
                    store,
                    learner: place.learner.clone(),
                    review: GradedAnswer {
                        concept: q.concept.clone(),
                        format: q.format(),
                        grade: *grade,
                        at: now,
                        via: identity.clone(),
                    },
                }
            })
            .collect();
        let graded = GradedSession {
            key: key.clone(),
            author,
            correct: result.grades.iter().filter(|(_, g)| g.is_correct()).count(),
            total: result.grades.len(),
            passed: result.passed,
            at: now,
        };
        let journal = GradingJournal {
            session: claimed,
            graded,
            reviews,
            profiles: Vec::new(),
        };

        // What the reviews will do to the profiles as they are now; the
        // profiles are not saved, applying the journal does that.
        let mut profiles = vec![(place.primary_id.clone(), primary)];
        if let (Some(id), Some(p)) = (place.project_id.clone(), project) {
            profiles.push((id, p));
        }
        let mut recorded = Recorded::default();
        for (id, profile) in profiles {
            let indices = journal.review_indices(&id);
            let reviews: Vec<GradedAnswer> = indices
                .iter()
                .map(|&i| journal.reviews[i].review.clone())
                .collect();
            let next = profile
                .with_session_reviews(&grading_mark(&key), &reviews)
                .map_or(profile, |(next, events)| {
                    recorded.add(&indices, events);
                    next
                });
            recorded.levels.push((id, next.level()));
        }
        let mut events = recorded.events_in_order(journal.reviews.len());
        events.push(result.event.into());
        let results = question_results(&questions, answers, &result.grades, &feedback, &events);
        let level_after = recorded.level_of(&place.primary_id).unwrap_or_default();
        let outcome = SessionOutcome {
            grades: result.grades,
            passed: result.passed,
            level_after,
            events,
            profile_location: place.location,
            results,
        };
        Ok((journal, outcome, place.primary_id))
    }

    /// The store `id` names.
    fn store_by_id(&self, id: &StoreId) -> Result<&S> {
        match id {
            StoreId::LearnerStore => Ok(&self.store),
            StoreId::Organisation(org) => self
                .org_stores
                .get(org)
                .ok_or_else(|| Error::Invalid(format!("no store for organisation {org:?}"))),
        }
    }

    /// Make the writes of `journal`, kept in `store`, then remove it. Every
    /// write is idempotent, so this can run any number of times, also
    /// concurrently (ADR-0002 D6). Returns what recording the reviews did,
    /// or `None` if the reviews were on the profiles already.
    async fn apply(&self, store: &S, journal: &GradingJournal) -> Result<Option<Recorded>> {
        let key = journal.key();
        let mut recorded = None;
        // The graded record is written last: once it exists, the reviews
        // are recorded. Without it, recording them again is safe all the
        // same, since the profiles say which sessions they have.
        if store.load_graded(key).await?.is_none() {
            if journal.reviews.is_empty() && !journal.profiles.is_empty() {
                // Written before reviews were journaled: it holds whole
                // profiles. Saving them again could undo later reviews, but
                // the graded record, written last, tells whether it was.
                for (id, profile) in &journal.profiles {
                    self.store_by_id(id)?.save_profile(profile).await?;
                }
            } else {
                recorded = self.record_reviews(journal).await?;
            }
            store.delete_session(key).await?;
            store.save_graded(&journal.graded).await?;
        }
        store.end_grading(key).await?;
        Ok(recorded)
    }

    /// Record the reviews of `journal` on the profiles they belong to. Each
    /// profile is loaded fresh and saved only if nobody saved it since: when
    /// another session of the learner got there first, the reviews are
    /// recorded on top of theirs (ADR-0002 D6). `None` if every profile
    /// had the reviews already.
    async fn record_reviews(&self, journal: &GradingJournal) -> Result<Option<Recorded>> {
        /// How often one profile may change under us before giving up.
        const ATTEMPTS: usize = 100;
        let mark = grading_mark(journal.key());
        let key = journal.key();
        let mut recorded = Recorded::default();
        let mut any = false;
        for (id, learner) in journal.review_targets() {
            let store = self.store_by_id(&id)?;
            let indices = journal.review_indices(&id);
            let reviews: Vec<GradedAnswer> = indices
                .iter()
                .map(|&i| journal.reviews[i].review.clone())
                .collect();
            let mut done = false;
            for _ in 0..ATTEMPTS {
                let profile = Self::load_or_new(store, &learner).await?;
                let Some((next, events)) = profile.with_session_reviews(&mark, &reviews) else {
                    done = true;
                    break;
                };
                if store.save_profile_if_current(&next).await? {
                    recorded.add(&indices, events);
                    recorded.levels.push((id.clone(), next.level()));
                    any = true;
                    done = true;
                    break;
                }
            }
            if !done {
                return Err(Error::Port(format!(
                    "the profile of {:?} kept changing while grading {:?}",
                    learner.0, key.0
                )));
            }
        }
        Ok(any.then_some(recorded))
    }

    /// Finish every grading left behind, e.g. by a crash or a failed write
    /// (ADR-0002 D6). Runs at the start of every operation that touches a
    /// learner or a session. Fails with the first error, after trying all.
    async fn recover(&self) -> Result<()> {
        let mut failed = None;
        for (i, store) in self.all_stores().enumerate() {
            let journals = match store.gradings().await {
                Ok(journals) => journals,
                // The learner store may be unreachable (ADR-0003 D4).
                Err(Error::Port(_)) if i == 0 && !self.org_stores.is_empty() => continue,
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
        }
        failed.map_or(Ok(()), Err)
    }

    // --- identities (ADR-0003) ----------------------------------------------

    /// Every store that may hold data about learners.
    fn all_stores(&self) -> impl Iterator<Item = &S> {
        std::iter::once(&self.store).chain(self.org_stores.values())
    }

    /// Where `author`'s profile lives right now (ADR-0003 D4).
    pub async fn profile_location(&self, author: &AuthorId) -> Result<ProfileLocation> {
        self.recover().await?;
        Ok(self.place(author).await?.location)
    }

    /// The learner behind `author`'s identity. The first time an identity is
    /// seen, a new learner is created and linked to it (D1).
    pub async fn learner_of(&self, author: &AuthorId) -> Result<LearnerId> {
        self.recover().await?;
        Ok(self.place(author).await?.learner)
    }

    /// Issue the link code `code` for `author`'s learner (D2). The caller
    /// supplies an unguessable `code`, e.g. from the OS random source.
    pub async fn request_link_code(&self, author: &AuthorId, code: String) -> Result<LinkCode> {
        self.recover().await?;
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
        self.recover().await?;
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
        self.recover().await?;
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
        self.recover().await?;
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
        self.recover().await?;
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
        self.recover().await?;
        let learner = self.resolve_in(&author.identity()?, &self.store).await?;
        for store in self.all_stores().chain(self.shared_bank.iter()) {
            store.delete_learner(&learner).await?;
        }
        Ok(())
    }
}

/// When `concept` is next due, by the last review scheduled in `events`.
fn next_review_of(events: &[DomainEvent], concept: &ConceptId) -> Option<DateTime<Utc>> {
    events.iter().rev().find_map(|e| match e {
        DomainEvent::Learner(LearnerEvent::ReviewScheduled {
            concept: c, due, ..
        }) if c == concept => Some(*due),
        _ => None,
    })
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
                    ..
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
                .unwrap_or_else(|| match (&answer, correct_option) {
                    (None, _) => format!("Not answered. {}", q.explanation),
                    // A wrong option first gets its own reason, so the author
                    // learns why their pick was wrong, then why the correct
                    // one is right.
                    (Some(AnswerBody::Choice(picked)), Some(correct)) if *picked != correct => {
                        match q.body.reason(*picked) {
                            Some(why) => format!("{why} {}", q.explanation),
                            None => q.explanation.clone(),
                        }
                    }
                    _ => q.explanation.clone(),
                });
            let next_review = next_review_of(events, &q.concept);
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

/// The mark a profile keeps of a graded session (ADR-0002 D6): opaque, so
/// that the learner store holds no PR reference (ADR-0003 D5), and 128 bits
/// so that two sessions of one learner don't share one.
fn grading_mark(key: &SessionKey) -> String {
    format!(
        "{}{}",
        opaque_id(&key.0),
        opaque_id(&format!("grading:{}", key.0))
    )
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
