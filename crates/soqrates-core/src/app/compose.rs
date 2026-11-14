//! Composing a session: which questions to ask, from due reviews, new
//! concepts and the question bank, and opening the session (ADR-0002 D5,
//! D9, D10; ADR-0009).

use std::collections::BTreeSet;

use crate::app::engine::Soqrates;
use crate::app::grading::SessionOutcome;
use crate::app::ports::{Clock, Codebase, Examiner, StateStore};
use crate::app::stores::{Placement, load_or_new};
use crate::assessment::{
    Answer, Concept, Diff, Question, QuestionBrief, QuestionFormat, QuestionKind, Weight,
};
use crate::delivery::{BankedQuestion, FollowUp, PrRef, PrRounds, SeenQuestions, Session, pick};
use crate::ids::{AuthorId, BankId, ConceptId, SessionKey};
use crate::learner::{Level, policy};
use crate::{Error, Result};

impl<S: StateStore, E: Examiner, C: Clock> Soqrates<S, E, C> {
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
        let gap = self.config.reask_after_rounds;
        for _ in 0..100 {
            let bank = store.bank_questions(&concept.id).await?;
            let Some(i) = pick(&bank, brief, seen, round, gap, in_round) else {
                break;
            };
            let mut entry = bank[i].clone();
            let mut q = entry.take();
            q.scope = concept.scope;
            if store
                .save_bank_question_if_current(&entry, Some(&bank[i]))
                .await?
            {
                seen.show(entry.id(), round);
                return Ok(q);
            }
        }
        // A question that was retired, or is disputed, is not asked again
        // by being written again word for word (ADR-0011 D6, D7): another
        // one is written instead.
        const WRITES: usize = 3;
        'write: for _ in 0..WRITES {
            let mut q = self.written(concept, brief, code).await?;
            let Some(new) = BankedQuestion::new(&q, level, self.clock.now()) else {
                return Ok(q);
            };
            for _ in 0..100 {
                let bank = store.bank_questions(&concept.id).await?;
                // The same question written again is banked once.
                let previous = bank.iter().find(|e| e.id() == new.id());
                let entry = match previous {
                    Some(existing) if existing.is_retired() || existing.is_disputed() => {
                        continue 'write;
                    }
                    Some(existing) => {
                        let mut entry = existing.clone();
                        entry.take();
                        entry
                    }
                    None => new.clone(),
                };
                if store
                    .save_bank_question_if_current(&entry, previous)
                    .await?
                {
                    seen.show(entry.id(), round);
                    q.banked = Some(entry.reference());
                    return Ok(q);
                }
            }
            return Err(Error::Port("bank question kept changing".into()));
        }
        Err(Error::Port(format!(
            "examiner: wrote a retired or disputed question about {:?} again",
            concept.id.0
        )))
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

    /// Open and persist the follow-up round after `session`, graded with
    /// `outcome` (ADR-0010): one new multiple-choice question about each
    /// concept answered wrongly, written from another angle than the
    /// question it follows. `None` when every answer was right, or the
    /// session was the last round `config.max_follow_ups` allows. The
    /// examiner may read `code`, the codebase the session was asked about.
    pub async fn follow_up(
        &self,
        session: &Session,
        outcome: &SessionOutcome,
        code: &impl Codebase,
    ) -> Result<Option<Session>> {
        let (first, round) = session.key().round();
        if round > self.config.max_follow_ups {
            return Ok(None);
        }
        let missed: Vec<(&Question, Option<Answer>)> = session
            .questions()
            .iter()
            .filter_map(|q| {
                let result = outcome.results.iter().find(|r| r.question == q.id)?;
                (!result.is_correct()).then(|| {
                    let answer = result.answer.clone().map(|body| Answer {
                        question: q.id.clone(),
                        body,
                    });
                    (q, answer)
                })
            })
            .collect();
        if missed.is_empty() {
            return Ok(None);
        }
        let stores = self.stores().strict().await?;
        let place = stores.place(session.author()).await?;
        let level = load_or_new(place.primary, &place.learner).await?.level();
        let mut questions: Vec<Question> = Vec::new();
        for (previous, answer) in missed {
            let concept = Concept {
                id: previous.concept.clone(),
                title: previous.concept.0.clone(),
                summary: String::new(),
                scope: previous.scope,
                weight: Weight::Core,
                locations: Vec::new(),
            };
            // Always multiple choice, from another angle: the earlier kind
            // goes to the back (ADR-0010 D3).
            let format = QuestionFormat::MultipleChoice;
            let asked: Vec<QuestionKind> = std::iter::once(previous.kind)
                .chain(questions.iter().map(|q| q.kind))
                .collect();
            let brief =
                QuestionBrief::new(format, &policy::kinds_for(level, format, &asked), vec![]);
            let mut q = self
                .examiner
                .write_follow_up(&concept, &brief, previous, answer.as_ref(), code)
                .await?;
            brief
                .check(&q)
                .map_err(|why| Error::Port(format!("examiner: {why}")))?;
            if q.concept != previous.concept {
                return Err(Error::Port(format!(
                    "examiner: a follow-up about {:?} asks about {:?}",
                    previous.concept.0, q.concept.0
                )));
            }
            if same_text(q.body.prompt(), previous.body.prompt()) {
                return Err(Error::Port(format!(
                    "examiner: the follow-up about {:?} repeats the question",
                    previous.concept.0
                )));
            }
            q.scope = previous.scope;
            questions.push(q);
        }
        let (settled, total) = match session.follow_up() {
            Some(f) => (f.settled + outcome.correct(), f.total),
            None => (outcome.correct(), session.questions().len()),
        };
        let follow_up = Session::open_follow_up(
            first.follow_up(round + 1),
            session.author().clone(),
            questions,
            FollowUp {
                round: round + 1,
                settled,
                total,
            },
            self.clock.now(),
        )?;
        stores
            .session_store(session.author())?
            .save_session(&follow_up)
            .await?;
        Ok(Some(follow_up))
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
}

/// Whether `a` and `b` are the same text, ignoring case and whitespace.
fn same_text(a: &str, b: &str) -> bool {
    let words = |s: &str| {
        s.split_whitespace()
            .map(str::to_lowercase)
            .collect::<Vec<_>>()
    };
    words(a) == words(b)
}
