//! [`StateStore`] on top of a [`DocumentStore`], once for every backend
//! (ADR-0001 D5): records are serialised to JSON documents here and nowhere
//! else.

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::Error;
use crate::Result;
use crate::app::ports::{Decision, Doc, DocumentStore, Kind, Removal, StateStore};
use crate::delivery::{
    AskedSession, BankedQuestion, GradedSession, GradingJournal, PrRef, PrRounds, SeenQuestions,
    Session,
};
use crate::ids::{BankId, ConceptId, Identity, LearnerId, SessionKey};
use crate::learner::{IdentityLink, LearnerProfile, LinkCode};

fn encode<T: Serialize>(kind: Kind, record: &T) -> Result<Doc> {
    serde_json::to_value(record).map_err(|e| Error::Port(format!("{}: {e}", kind.label())))
}

fn decode<T: DeserializeOwned>(kind: Kind, key: &str, doc: Doc) -> Result<T> {
    serde_json::from_value(doc)
        .map_err(|e| Error::Port(format!("corrupt {} {key:?}: {e}", kind.label())))
}

async fn get<D: DocumentStore, T: DeserializeOwned>(
    d: &D,
    kind: Kind,
    key: &str,
) -> Result<Option<T>> {
    d.load(kind, key)
        .await?
        .map(|doc| decode(kind, key, doc))
        .transpose()
}

async fn put<D: DocumentStore, T: Serialize>(
    d: &D,
    kind: Kind,
    key: &str,
    record: &T,
) -> Result<()> {
    d.save(kind, key, &encode(kind, record)?).await
}

async fn all<D: DocumentStore, T: DeserializeOwned>(
    d: &D,
    kind: Kind,
    group: Option<&str>,
) -> Result<Vec<T>> {
    d.list(kind, group)
        .await?
        .into_iter()
        .map(|doc| decode(kind, "", doc))
        .collect()
}

/// How many times an update may lose to another writer before it gives up.
/// Each attempt reloads at once: the core has no clock to back off with
/// (ADR-0001 D2), and contention on one document is rare and brief.
const UPDATE_ATTEMPTS: usize = 100;

/// The one way a record that several writers may change is changed: load
/// it, let `change` decide, and save what it puts only if the stored
/// document is still the one loaded ([`DocumentStore::swap`]). Lost races
/// reload and decide again. See [`Decision`].
async fn update<D, T, R>(
    d: &D,
    kind: Kind,
    key: &str,
    mut change: impl FnMut(Option<T>) -> Result<Decision<T, R>> + Send,
) -> Result<R>
where
    D: DocumentStore,
    T: Serialize + DeserializeOwned + Send,
    R: Send,
{
    for _ in 0..UPDATE_ATTEMPTS {
        let stored = d.load(kind, key).await?;
        let current = stored
            .clone()
            .map(|doc| decode::<T>(kind, key, doc))
            .transpose()?;
        match change(current)? {
            Decision::Keep(result) => return Ok(result),
            Decision::Put(record, result) => {
                if d.swap(kind, key, stored.as_ref(), &encode(kind, &record)?)
                    .await?
                {
                    return Ok(result);
                }
            }
        }
    }
    Err(Error::Port(format!(
        "{} {key:?} kept changing",
        kind.label()
    )))
}

impl<D: DocumentStore> StateStore for D {
    async fn update_bank_question<R: Send>(
        &self,
        id: &BankId,
        mut change: impl FnMut(Option<BankedQuestion>) -> Result<Decision<BankedQuestion, R>> + Send,
    ) -> Result<R> {
        update(self, Kind::Bank, &id.0, |current| {
            let decision = change(current)?;
            if let Decision::Put(question, _) = &decision
                && question.id() != id
            {
                return Err(Error::Invalid(format!(
                    "an update of bank question {:?} put {:?}",
                    id.0,
                    question.id().0
                )));
            }
            Ok(decision)
        })
        .await
    }
    async fn update_asked<R: Send>(
        &self,
        key: &SessionKey,
        mut change: impl FnMut(Option<AskedSession>) -> Result<Decision<AskedSession, R>> + Send,
    ) -> Result<R> {
        update(self, Kind::Asked, &key.0, |current| {
            let decision = change(current)?;
            if let Decision::Put(asked, _) = &decision
                && asked.key() != key
            {
                return Err(Error::Invalid(format!(
                    "an update of asked session {:?} put {:?}",
                    key.0,
                    asked.key().0
                )));
            }
            Ok(decision)
        })
        .await
    }

    async fn load_profile(&self, learner: &LearnerId) -> Result<Option<LearnerProfile>> {
        get(self, Kind::Profile, &learner.0).await
    }
    async fn save_profile(&self, profile: &LearnerProfile) -> Result<()> {
        put(self, Kind::Profile, &profile.learner().0, profile).await
    }
    async fn update_profile<R: Send>(
        &self,
        learner: &LearnerId,
        mut change: impl FnMut(Option<LearnerProfile>) -> Result<Decision<LearnerProfile, R>> + Send,
    ) -> Result<R> {
        update(self, Kind::Profile, &learner.0, |current| {
            let version = current.as_ref().map_or(0, LearnerProfile::version);
            Ok(match change(current)? {
                Decision::Put(profile, result) => {
                    if profile.learner() != learner {
                        return Err(Error::Invalid(format!(
                            "an update of the profile of {:?} put that of {:?}",
                            learner.0,
                            profile.learner().0
                        )));
                    }
                    Decision::Put(profile.with_version(version + 1), result)
                }
                keep => keep,
            })
        })
        .await
    }
    async fn delete_learner(&self, learner: &LearnerId) -> Result<()> {
        self.remove(&[
            Removal::Group(Kind::Link, &learner.0),
            Removal::Key(Kind::Seen, &learner.0),
            Removal::Group(Kind::Asked, &learner.0),
            Removal::Key(Kind::Profile, &learner.0),
        ])
        .await
    }

    async fn learner_of(&self, identity: &Identity) -> Result<Option<LearnerId>> {
        let link: Option<IdentityLink> = get(self, Kind::Link, &identity.to_string()).await?;
        Ok(link.map(|l| l.learner))
    }
    async fn save_link(&self, link: &IdentityLink) -> Result<()> {
        put(self, Kind::Link, &link.identity.to_string(), link).await
    }
    async fn delete_link(&self, identity: &Identity) -> Result<()> {
        self.remove(&[Removal::Key(Kind::Link, &identity.to_string())])
            .await
    }
    async fn links_of(&self, learner: &LearnerId) -> Result<Vec<IdentityLink>> {
        let mut links: Vec<IdentityLink> = all(self, Kind::Link, Some(&learner.0)).await?;
        links.sort_by_key(|l| l.identity.to_string());
        Ok(links)
    }

    async fn save_link_code(&self, code: &LinkCode) -> Result<()> {
        put(self, Kind::LinkCode, code.code(), code).await
    }
    async fn load_link_code(&self, code: &str) -> Result<Option<LinkCode>> {
        get(self, Kind::LinkCode, code).await
    }

    async fn load_session(&self, key: &SessionKey) -> Result<Option<Session>> {
        get(self, Kind::Session, &key.0).await
    }
    async fn save_session(&self, session: &Session) -> Result<()> {
        put(self, Kind::Session, &session.key().0, session).await
    }
    async fn delete_session(&self, key: &SessionKey) -> Result<()> {
        self.remove(&[Removal::Key(Kind::Session, &key.0)]).await
    }
    async fn sessions(&self) -> Result<Vec<Session>> {
        all(self, Kind::Session, None).await
    }
    async fn begin_grading(&self, journal: &GradingJournal) -> Result<bool> {
        let doc = encode(Kind::Grading, journal)?;
        self.claim(Kind::Session, Kind::Grading, &journal.key().0, &doc)
            .await
    }
    async fn load_grading(&self, key: &SessionKey) -> Result<Option<GradingJournal>> {
        get(self, Kind::Grading, &key.0).await
    }
    async fn gradings(&self) -> Result<Vec<GradingJournal>> {
        let mut journals: Vec<GradingJournal> = all(self, Kind::Grading, None).await?;
        journals.sort_by(|a, b| a.key().cmp(b.key()));
        Ok(journals)
    }
    async fn end_grading(&self, key: &SessionKey) -> Result<()> {
        self.remove(&[Removal::Key(Kind::Grading, &key.0)]).await
    }

    async fn save_graded(&self, graded: &GradedSession) -> Result<()> {
        put(self, Kind::Graded, &graded.key.0, graded).await
    }
    async fn load_graded(&self, key: &SessionKey) -> Result<Option<GradedSession>> {
        get(self, Kind::Graded, &key.0).await
    }

    async fn load_rounds(&self, pr: &PrRef) -> Result<Option<PrRounds>> {
        get(self, Kind::Rounds, &pr.id()).await
    }
    async fn save_rounds(&self, rounds: &PrRounds) -> Result<()> {
        put(self, Kind::Rounds, &rounds.pr().id(), rounds).await
    }

    async fn bank_questions(&self, concept: &ConceptId) -> Result<Vec<BankedQuestion>> {
        let mut questions: Vec<BankedQuestion> = all(self, Kind::Bank, Some(&concept.0)).await?;
        questions.sort_by(|a, b| (a.added_at(), a.id()).cmp(&(b.added_at(), b.id())));
        Ok(questions)
    }
    async fn save_bank_question(&self, question: &BankedQuestion) -> Result<()> {
        put(self, Kind::Bank, &question.id().0, question).await
    }
    async fn load_seen(&self, learner: &LearnerId) -> Result<Option<SeenQuestions>> {
        get(self, Kind::Seen, &learner.0).await
    }
    async fn load_asked(&self, key: &SessionKey) -> Result<Option<AskedSession>> {
        get(self, Kind::Asked, &key.0).await
    }
    async fn save_asked(&self, asked: &AskedSession) -> Result<()> {
        put(self, Kind::Asked, &asked.key().0, asked).await
    }
    async fn asked_sessions(&self) -> Result<Vec<AskedSession>> {
        all(self, Kind::Asked, None).await
    }

    async fn save_seen(&self, seen: &SeenQuestions) -> Result<()> {
        put(self, Kind::Seen, &seen.learner().0, seen).await
    }
}
