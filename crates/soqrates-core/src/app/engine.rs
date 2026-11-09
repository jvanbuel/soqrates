//! The Soqrates engine: orchestrates the aggregates (load, call, save,
//! emit events). Holds no domain rules (ADR-0002 D3).

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::app::events::DomainEvent;
use crate::app::ports::{Clock, Codebase, Examiner, StateStore};
pub use crate::app::stores::ProfileLocation;
use crate::app::stores::{Access, Placement, Recorded, Stores, grading_mark, load_or_new};
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

/// Where a session is in its life (ADR-0002 D6).
#[derive(Debug, Clone)]
pub enum SessionState {
    /// No such session, or one that was discarded.
    Absent,
    /// Asked, not graded yet.
    Pending(Session),
    /// Its answers were graded but not all results are recorded yet.
    Grading,
    /// Graded.
    Graded(GradedSession),
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

    /// The router over this engine's stores.
    fn stores(&self) -> Stores<'_, S, C> {
        Stores::new(
            &self.store,
            &self.org_stores,
            self.shared_bank.as_ref(),
            &self.config.forges,
            &self.clock,
        )
    }

    // --- profiles -----------------------------------------------------------

    /// The profile of `author`'s learner, or a fresh `Novice` profile if the
    /// learner is new (ADR-0001 D5, ADR-0003 D1).
    pub async fn profile(&self, author: &AuthorId) -> Result<LearnerProfile> {
        let stores = self.stores().strict().await?;
        let place = stores.place(author).await?;
        load_or_new(place.primary, &place.learner).await
    }

    /// Due concepts with the scope of the store their card lives in, earliest
    /// due first, ties by concept ID (ADR-0002 D8, ADR-0003 D5).
    async fn due_with_scope(
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
        let stores = self.stores().strict().await?;
        let mut questions = carried.to_vec();
        if diff.is_empty() {
            return Ok(questions);
        }
        let open = self.config.max_questions.saturating_sub(carried.len());
        if open == 0 {
            return Ok(questions);
        }
        let place = stores.place(author).await?;
        let level = load_or_new(place.primary, &place.learner).await?.level();
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
        let bank_store = match self.config.question_bank {
            true => stores.bank_store(author)?,
            false => None,
        };
        let mut bank = match bank_store {
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
        let mut profiles = vec![load_or_new(place.primary, &place.learner).await?];
        if let Some(project) = place.project {
            profiles.push(load_or_new(project, &place.learner).await?);
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
        let stores = self.stores().strict().await?;
        let cap = self.config.max_questions.max(carried);
        let session = Session::open(key, author.clone(), questions, cap, self.clock.now())?;
        stores.session_store(author)?.save_session(&session).await?;
        Ok(session)
    }

    /// The rounds asked so far on `pr`, authored by `author` (ADR-0002 D9).
    /// Kept with the author's sessions, never in the shared learner store
    /// when an organisation store exists (ADR-0003 D5).
    pub async fn rounds(&self, pr: &PrRef, author: &AuthorId) -> Result<PrRounds> {
        let stores = self.stores().without_recovery();
        Ok(stores
            .session_store(author)?
            .load_rounds(pr)
            .await?
            .unwrap_or_else(|| PrRounds::new(pr.clone())))
    }

    /// Persist `rounds` of a pull request by `author`.
    pub async fn save_rounds(&self, author: &AuthorId, rounds: &PrRounds) -> Result<()> {
        let stores = self.stores().without_recovery();
        stores.session_store(author)?.save_rounds(rounds).await
    }

    /// Remove the pending session `key` without grading it, e.g. when its
    /// questions could not be posted.
    pub async fn discard_session(&self, key: &SessionKey) -> Result<()> {
        let stores = self.stores().best_effort().await;
        if let Some((_, store)) = stores.find_session(key).await? {
            store.delete_session(key).await?;
        }
        Ok(())
    }

    /// Where the session `key` is in its life (ADR-0002 D6). The one query
    /// for it: the lookups run in the order a grading moves through, pending,
    /// then being graded, then graded, so a grading by another run is never
    /// missed and no new session is opened for its key.
    pub async fn session_state(&self, key: &SessionKey) -> Result<SessionState> {
        let stores = self.stores().best_effort().await;
        if let Some((session, _)) = stores.find_session(key).await? {
            return Ok(SessionState::Pending(session));
        }
        // The journal is looked up before the graded record, never after:
        // a grading that finishes in between is then found as graded. A
        // journal left behind next to a graded record only needs removing,
        // so the record wins.
        let grading = stores.grading_recorded(key).await?;
        Ok(match stores.graded_record(key).await? {
            Some(graded) => SessionState::Graded(graded),
            None if grading => SessionState::Grading,
            None => SessionState::Absent,
        })
    }

    /// The record of the graded session `key`, if it was graded.
    pub async fn graded(&self, key: &SessionKey) -> Result<Option<GradedSession>> {
        // Best effort: a grading that can't be finished yet is reported as
        // being graded (ADR-0002 D6).
        let stores = self.stores().best_effort().await;
        stores.graded_record(key).await
    }

    /// The pending (not yet graded) session `key`, if any.
    pub async fn pending_session(&self, key: &SessionKey) -> Result<Option<Session>> {
        Ok(match self.session_state(key).await? {
            SessionState::Pending(session) => Some(session),
            _ => None,
        })
    }

    /// Whether the session `key` is being graded: its answers were graded,
    /// but not all of the results are recorded yet (ADR-0002 D6).
    pub async fn being_graded(&self, key: &SessionKey) -> Result<bool> {
        let stores = self.stores().best_effort().await;
        stores.grading_recorded(key).await
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
        let stores = self.stores().strict().await?;
        let not_found = || Error::NotFound(format!("session {:?}", key.0));
        let (session, session_store) = stores.find_session(key).await?.ok_or_else(not_found)?;
        let (journal, mut outcome, primary) = self.grade(&stores, session, answers, code).await?;
        // Of several concurrent submissions, exactly one claims the session.
        if !session_store.begin_grading(&journal).await? {
            return Err(not_found());
        }
        if let Some(recorded) = stores.apply(session_store, &journal).await? {
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
        stores: &Access<'_, S, C>,
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

        let place = stores.place(&author).await?;
        let identity = author.identity()?;
        let primary = load_or_new(place.primary, &place.learner).await?;
        let project = match place.project {
            Some(store) => Some(load_or_new(store, &place.learner).await?),
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

    // --- identities (ADR-0003) ----------------------------------------------

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
