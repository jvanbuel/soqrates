//! `StateStore` adapters (ADR-0001 D5, ADR-0003): in memory, JSON files,
//! and, with the `sqlite` or `postgres` feature, a SQL database (ADR-0005).

#[cfg(any(feature = "sqlite", feature = "postgres"))]
mod sql;
#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub use sql::{SCHEMA_VERSION, SqlStore};

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::Serialize;
use serde::de::DeserializeOwned;
use soqrates_core::ports::{LearnerStore, StateStore};
use soqrates_core::{
    AskedSession, BankId, BankedQuestion, ConceptId, Error, GradedSession, GradingJournal,
    Identity, IdentityLink, LearnerId, LearnerProfile, LinkCode, PrRef, PrRounds, Result,
    SeenQuestions, Session, SessionKey,
};

#[derive(Debug, Default)]
struct Data {
    profiles: BTreeMap<LearnerId, LearnerProfile>,
    links: BTreeMap<Identity, IdentityLink>,
    codes: BTreeMap<String, LinkCode>,
    sessions: BTreeMap<SessionKey, Session>,
    graded: BTreeMap<SessionKey, GradedSession>,
    gradings: BTreeMap<SessionKey, GradingJournal>,
    rounds: BTreeMap<String, PrRounds>,
    bank: BTreeMap<BankId, BankedQuestion>,
    seen: BTreeMap<LearnerId, SeenQuestions>,
    asked: BTreeMap<SessionKey, AskedSession>,
}

/// `questions` in the order they were banked (ADR-0009 D1).
fn in_bank_order(mut questions: Vec<BankedQuestion>) -> Vec<BankedQuestion> {
    questions.sort_by(|a, b| (a.added_at(), a.id()).cmp(&(b.added_at(), b.id())));
    questions
}

/// Keeps everything in memory. Useful for tests and for the MCP server's
/// ephemeral mode.
#[derive(Debug, Default)]
pub struct InMemoryStore {
    // Boxed, so a `Store` holding it stays small.
    data: Mutex<Box<Data>>,
}

impl InMemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn with<T>(&self, f: impl FnOnce(&mut Data) -> T) -> T {
        f(&mut self.data.lock().expect("store mutex poisoned"))
    }
}

impl StateStore for InMemoryStore {
    async fn load_profile(&self, learner: &LearnerId) -> Result<Option<LearnerProfile>> {
        Ok(self.with(|d| d.profiles.get(learner).cloned()))
    }
    async fn save_profile(&self, profile: &LearnerProfile) -> Result<()> {
        self.with(|d| {
            d.profiles
                .insert(profile.learner().clone(), profile.clone())
        });
        Ok(())
    }
    async fn save_profile_if_current(&self, profile: &LearnerProfile) -> Result<bool> {
        Ok(self.with(|d| {
            let stored = d.profiles.get(profile.learner()).map_or(0, |p| p.version());
            if stored != profile.version() {
                return false;
            }
            d.profiles.insert(
                profile.learner().clone(),
                profile.clone().with_version(stored + 1),
            );
            true
        }))
    }
    async fn delete_learner(&self, learner: &LearnerId) -> Result<()> {
        self.with(|d| {
            d.profiles.remove(learner);
            d.links.retain(|_, link| &link.learner != learner);
            d.seen.remove(learner);
            d.asked.retain(|_, a| a.learner() != learner);
        });
        Ok(())
    }
    async fn learner_of(&self, identity: &Identity) -> Result<Option<LearnerId>> {
        Ok(self.with(|d| d.links.get(identity).map(|l| l.learner.clone())))
    }
    async fn save_link(&self, link: &IdentityLink) -> Result<()> {
        self.with(|d| d.links.insert(link.identity.clone(), link.clone()));
        Ok(())
    }
    async fn delete_link(&self, identity: &Identity) -> Result<()> {
        self.with(|d| d.links.remove(identity));
        Ok(())
    }
    async fn links_of(&self, learner: &LearnerId) -> Result<Vec<IdentityLink>> {
        Ok(self.with(|d| {
            d.links
                .values()
                .filter(|l| &l.learner == learner)
                .cloned()
                .collect()
        }))
    }
    async fn save_link_code(&self, code: &LinkCode) -> Result<()> {
        self.with(|d| d.codes.insert(code.code().to_string(), code.clone()));
        Ok(())
    }
    async fn load_link_code(&self, code: &str) -> Result<Option<LinkCode>> {
        Ok(self.with(|d| d.codes.get(code).cloned()))
    }
    async fn load_session(&self, key: &SessionKey) -> Result<Option<Session>> {
        Ok(self.with(|d| d.sessions.get(key).cloned()))
    }
    async fn save_session(&self, session: &Session) -> Result<()> {
        self.with(|d| d.sessions.insert(session.key().clone(), session.clone()));
        Ok(())
    }
    async fn delete_session(&self, key: &SessionKey) -> Result<()> {
        self.with(|d| d.sessions.remove(key));
        Ok(())
    }
    async fn begin_grading(&self, journal: &GradingJournal) -> Result<bool> {
        let key = journal.key();
        Ok(self.with(|d| {
            if d.gradings.contains_key(key) || d.sessions.remove(key).is_none() {
                return false;
            }
            d.gradings.insert(key.clone(), journal.clone());
            true
        }))
    }
    async fn load_grading(&self, key: &SessionKey) -> Result<Option<GradingJournal>> {
        Ok(self.with(|d| d.gradings.get(key).cloned()))
    }
    async fn gradings(&self) -> Result<Vec<GradingJournal>> {
        Ok(self.with(|d| d.gradings.values().cloned().collect()))
    }
    async fn end_grading(&self, key: &SessionKey) -> Result<()> {
        self.with(|d| d.gradings.remove(key));
        Ok(())
    }
    async fn save_graded(&self, graded: &GradedSession) -> Result<()> {
        self.with(|d| d.graded.insert(graded.key.clone(), graded.clone()));
        Ok(())
    }
    async fn load_graded(&self, key: &SessionKey) -> Result<Option<GradedSession>> {
        Ok(self.with(|d| d.graded.get(key).cloned()))
    }
    async fn load_rounds(&self, pr: &PrRef) -> Result<Option<PrRounds>> {
        Ok(self.with(|d| d.rounds.get(&pr.id()).cloned()))
    }
    async fn save_rounds(&self, rounds: &PrRounds) -> Result<()> {
        self.with(|d| d.rounds.insert(rounds.pr().id(), rounds.clone()));
        Ok(())
    }
    async fn bank_questions(&self, concept: &ConceptId) -> Result<Vec<BankedQuestion>> {
        Ok(in_bank_order(self.with(|d| {
            d.bank
                .values()
                .filter(|q| q.concept() == concept)
                .cloned()
                .collect()
        })))
    }
    async fn save_bank_question(&self, question: &BankedQuestion) -> Result<()> {
        self.with(|d| d.bank.insert(question.id().clone(), question.clone()));
        Ok(())
    }
    async fn load_seen(&self, learner: &LearnerId) -> Result<Option<SeenQuestions>> {
        Ok(self.with(|d| d.seen.get(learner).cloned()))
    }
    async fn save_seen(&self, seen: &SeenQuestions) -> Result<()> {
        self.with(|d| d.seen.insert(seen.learner().clone(), seen.clone()));
        Ok(())
    }
    async fn load_asked(&self, key: &SessionKey) -> Result<Option<AskedSession>> {
        Ok(self.with(|d| d.asked.get(key).cloned()))
    }
    async fn save_asked(&self, asked: &AskedSession) -> Result<()> {
        self.with(|d| d.asked.insert(asked.key().clone(), asked.clone()));
        Ok(())
    }
    async fn asked_sessions(&self) -> Result<Vec<AskedSession>> {
        Ok(self.with(|d| d.asked.values().cloned().collect()))
    }
}

impl LearnerStore for InMemoryStore {}

/// One JSON document per profile, link, link code, pending session, grading
/// journal, graded session, pull request's rounds, bank question,
/// learner's seen questions and asked session, under a directory. File names are the
/// hex-encoded keys; bank questions are in a directory per concept.
#[derive(Debug, Clone)]
pub struct FileStore {
    pub root: PathBuf,
}

impl FileStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn path(&self, kind: &str, key: &str) -> PathBuf {
        self.root.join(kind).join(format!("{}.json", hex(key)))
    }

    /// The directory of the bank questions about `concept` (ADR-0009 D1).
    fn bank_dir(concept: &ConceptId) -> String {
        format!("bank/{}", hex(&concept.0))
    }

    fn read<T: DeserializeOwned>(&self, kind: &str, key: &str) -> Result<Option<T>> {
        match std::fs::read(self.path(kind, key)) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|e| Error::Port(format!("corrupt {kind} {key:?}: {e}"))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io(e)),
        }
    }

    fn write<T: Serialize>(&self, kind: &str, key: &str, value: &T) -> Result<()> {
        let path = self.path(kind, key);
        std::fs::create_dir_all(path.parent().expect("has parent")).map_err(io)?;
        let json = serde_json::to_vec_pretty(value).map_err(|e| Error::Port(e.to_string()))?;
        // Write then rename, so a crash never leaves a half-written file.
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json).map_err(io)?;
        std::fs::rename(&tmp, &path).map_err(io)
    }

    fn remove(&self, kind: &str, key: &str) -> Result<()> {
        match std::fs::remove_file(self.path(kind, key)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(io(e)),
            _ => Ok(()),
        }
    }

    fn all<T: DeserializeOwned>(&self, kind: &str) -> Result<Vec<T>> {
        let dir = self.root.join(kind);
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(io(e)),
        };
        let mut out = Vec::new();
        for entry in entries {
            let path = entry.map_err(io)?.path();
            if path.extension().is_some_and(|x| x == "json") {
                let bytes = std::fs::read(&path).map_err(io)?;
                out.push(
                    serde_json::from_slice(&bytes)
                        .map_err(|e| Error::Port(format!("corrupt {}: {e}", path.display())))?,
                );
            }
        }
        Ok(out)
    }
}

fn hex(key: &str) -> String {
    key.bytes().map(|b| format!("{b:02x}")).collect()
}

fn io(e: std::io::Error) -> Error {
    Error::Port(e.to_string())
}

impl StateStore for FileStore {
    async fn load_profile(&self, learner: &LearnerId) -> Result<Option<LearnerProfile>> {
        self.read("profiles", &learner.0)
    }
    async fn save_profile(&self, profile: &LearnerProfile) -> Result<()> {
        self.write("profiles", &profile.learner().0, profile)
    }
    async fn save_profile_if_current(&self, profile: &LearnerProfile) -> Result<bool> {
        // Between processes, an advisory lock makes checking the version
        // and writing one step. The OS releases it if the process stops.
        let dir = self.root.join("profiles");
        std::fs::create_dir_all(&dir).map_err(io)?;
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join(".lock"))
            .map_err(io)?;
        lock.lock().map_err(io)?;
        let learner = &profile.learner().0;
        let stored = self
            .read::<LearnerProfile>("profiles", learner)?
            .map_or(0, |p| p.version());
        if stored != profile.version() {
            return Ok(false);
        }
        self.write(
            "profiles",
            learner,
            &profile.clone().with_version(stored + 1),
        )?;
        Ok(true)
    }
    async fn delete_learner(&self, learner: &LearnerId) -> Result<()> {
        for link in self.links_of(learner).await? {
            self.delete_link(&link.identity).await?;
        }
        self.remove("seen", &learner.0)?;
        for asked in self.all::<AskedSession>("asked")? {
            if asked.learner() == learner {
                self.remove("asked", &asked.key().0)?;
            }
        }
        self.remove("profiles", &learner.0)
    }
    async fn learner_of(&self, identity: &Identity) -> Result<Option<LearnerId>> {
        Ok(self
            .read::<IdentityLink>("links", &identity.to_string())?
            .map(|l| l.learner))
    }
    async fn save_link(&self, link: &IdentityLink) -> Result<()> {
        self.write("links", &link.identity.to_string(), link)
    }
    async fn delete_link(&self, identity: &Identity) -> Result<()> {
        self.remove("links", &identity.to_string())
    }
    async fn links_of(&self, learner: &LearnerId) -> Result<Vec<IdentityLink>> {
        Ok(self
            .all::<IdentityLink>("links")?
            .into_iter()
            .filter(|l| &l.learner == learner)
            .collect())
    }
    async fn save_link_code(&self, code: &LinkCode) -> Result<()> {
        self.write("codes", code.code(), code)
    }
    async fn load_link_code(&self, code: &str) -> Result<Option<LinkCode>> {
        self.read("codes", code)
    }
    async fn load_session(&self, key: &SessionKey) -> Result<Option<Session>> {
        self.read("sessions", &key.0)
    }
    async fn save_session(&self, session: &Session) -> Result<()> {
        self.write("sessions", &session.key().0, session)
    }
    async fn delete_session(&self, key: &SessionKey) -> Result<()> {
        self.remove("sessions", &key.0)
    }
    async fn begin_grading(&self, journal: &GradingJournal) -> Result<bool> {
        let key = &journal.key().0;
        // Between processes, an advisory lock makes checking and claiming
        // one step. The OS releases it if the process stops.
        let dir = self.root.join("gradings");
        std::fs::create_dir_all(&dir).map_err(io)?;
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join(".lock"))
            .map_err(io)?;
        lock.lock().map_err(io)?;
        if self.path("gradings", key).exists() || !self.path("sessions", key).exists() {
            return Ok(false);
        }
        // Journal first: a stop before the session is removed leaves both,
        // and finishing the journal removes the session (ADR-0002 D6).
        self.write("gradings", key, journal)?;
        self.remove("sessions", key)?;
        Ok(true)
    }
    async fn load_grading(&self, key: &SessionKey) -> Result<Option<GradingJournal>> {
        self.read("gradings", &key.0)
    }
    async fn gradings(&self) -> Result<Vec<GradingJournal>> {
        self.all("gradings")
    }
    async fn end_grading(&self, key: &SessionKey) -> Result<()> {
        self.remove("gradings", &key.0)
    }
    async fn save_graded(&self, graded: &GradedSession) -> Result<()> {
        self.write("graded", &graded.key.0, graded)
    }
    async fn load_graded(&self, key: &SessionKey) -> Result<Option<GradedSession>> {
        self.read("graded", &key.0)
    }
    async fn load_rounds(&self, pr: &PrRef) -> Result<Option<PrRounds>> {
        self.read("rounds", &pr.id())
    }
    async fn save_rounds(&self, rounds: &PrRounds) -> Result<()> {
        self.write("rounds", &rounds.pr().id(), rounds)
    }
    async fn bank_questions(&self, concept: &ConceptId) -> Result<Vec<BankedQuestion>> {
        Ok(in_bank_order(self.all(&Self::bank_dir(concept))?))
    }
    async fn save_bank_question(&self, question: &BankedQuestion) -> Result<()> {
        self.write(
            &Self::bank_dir(question.concept()),
            &question.id().0,
            question,
        )
    }
    async fn load_seen(&self, learner: &LearnerId) -> Result<Option<SeenQuestions>> {
        self.read("seen", &learner.0)
    }
    async fn save_seen(&self, seen: &SeenQuestions) -> Result<()> {
        self.write("seen", &seen.learner().0, seen)
    }
    async fn load_asked(&self, key: &SessionKey) -> Result<Option<AskedSession>> {
        self.read("asked", &key.0)
    }
    async fn save_asked(&self, asked: &AskedSession) -> Result<()> {
        self.write("asked", &asked.key().0, asked)
    }
    async fn asked_sessions(&self) -> Result<Vec<AskedSession>> {
        self.all("asked")
    }
}

impl LearnerStore for FileStore {}

/// Any of the stores above, chosen at run time, e.g. from configuration.
#[derive(Debug)]
pub enum Store {
    Memory(InMemoryStore),
    File(FileStore),
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    Sql(SqlStore),
}

impl Store {
    /// The SQL store at `url` if one is given (ADR-0005 D5), else the file
    /// store in `state_dir`.
    pub async fn open(database_url: Option<&str>, state_dir: PathBuf) -> Result<Self> {
        match database_url {
            #[cfg(any(feature = "sqlite", feature = "postgres"))]
            Some(url) => Ok(Store::Sql(SqlStore::connect(url).await?)),
            #[cfg(not(any(feature = "sqlite", feature = "postgres")))]
            Some(_) => Err(Error::Port(
                "this build of Soqrates has no database support".into(),
            )),
            None => Ok(Store::File(FileStore::new(state_dir))),
        }
    }
}

macro_rules! delegate {
    ($self:ident, $method:ident($($arg:expr),*)) => {
        match $self {
            Store::Memory(s) => s.$method($($arg),*).await,
            Store::File(s) => s.$method($($arg),*).await,
            #[cfg(any(feature = "sqlite", feature = "postgres"))]
            Store::Sql(s) => s.$method($($arg),*).await,
        }
    };
}

impl StateStore for Store {
    async fn load_profile(&self, learner: &LearnerId) -> Result<Option<LearnerProfile>> {
        delegate!(self, load_profile(learner))
    }
    async fn save_profile(&self, profile: &LearnerProfile) -> Result<()> {
        delegate!(self, save_profile(profile))
    }
    async fn save_profile_if_current(&self, profile: &LearnerProfile) -> Result<bool> {
        delegate!(self, save_profile_if_current(profile))
    }
    async fn delete_learner(&self, learner: &LearnerId) -> Result<()> {
        delegate!(self, delete_learner(learner))
    }
    async fn learner_of(&self, identity: &Identity) -> Result<Option<LearnerId>> {
        delegate!(self, learner_of(identity))
    }
    async fn save_link(&self, link: &IdentityLink) -> Result<()> {
        delegate!(self, save_link(link))
    }
    async fn delete_link(&self, identity: &Identity) -> Result<()> {
        delegate!(self, delete_link(identity))
    }
    async fn links_of(&self, learner: &LearnerId) -> Result<Vec<IdentityLink>> {
        delegate!(self, links_of(learner))
    }
    async fn save_link_code(&self, code: &LinkCode) -> Result<()> {
        delegate!(self, save_link_code(code))
    }
    async fn load_link_code(&self, code: &str) -> Result<Option<LinkCode>> {
        delegate!(self, load_link_code(code))
    }
    async fn load_session(&self, key: &SessionKey) -> Result<Option<Session>> {
        delegate!(self, load_session(key))
    }
    async fn save_session(&self, session: &Session) -> Result<()> {
        delegate!(self, save_session(session))
    }
    async fn delete_session(&self, key: &SessionKey) -> Result<()> {
        delegate!(self, delete_session(key))
    }
    async fn begin_grading(&self, journal: &GradingJournal) -> Result<bool> {
        delegate!(self, begin_grading(journal))
    }
    async fn load_grading(&self, key: &SessionKey) -> Result<Option<GradingJournal>> {
        delegate!(self, load_grading(key))
    }
    async fn gradings(&self) -> Result<Vec<GradingJournal>> {
        delegate!(self, gradings())
    }
    async fn end_grading(&self, key: &SessionKey) -> Result<()> {
        delegate!(self, end_grading(key))
    }
    async fn save_graded(&self, graded: &GradedSession) -> Result<()> {
        delegate!(self, save_graded(graded))
    }
    async fn load_graded(&self, key: &SessionKey) -> Result<Option<GradedSession>> {
        delegate!(self, load_graded(key))
    }
    async fn load_rounds(&self, pr: &PrRef) -> Result<Option<PrRounds>> {
        delegate!(self, load_rounds(pr))
    }
    async fn save_rounds(&self, rounds: &PrRounds) -> Result<()> {
        delegate!(self, save_rounds(rounds))
    }
    async fn bank_questions(&self, concept: &ConceptId) -> Result<Vec<BankedQuestion>> {
        delegate!(self, bank_questions(concept))
    }
    async fn save_bank_question(&self, question: &BankedQuestion) -> Result<()> {
        delegate!(self, save_bank_question(question))
    }
    async fn load_seen(&self, learner: &LearnerId) -> Result<Option<SeenQuestions>> {
        delegate!(self, load_seen(learner))
    }
    async fn save_seen(&self, seen: &SeenQuestions) -> Result<()> {
        delegate!(self, save_seen(seen))
    }
    async fn load_asked(&self, key: &SessionKey) -> Result<Option<AskedSession>> {
        delegate!(self, load_asked(key))
    }
    async fn save_asked(&self, asked: &AskedSession) -> Result<()> {
        delegate!(self, save_asked(asked))
    }
    async fn asked_sessions(&self) -> Result<Vec<AskedSession>> {
        delegate!(self, asked_sessions())
    }
}

impl LearnerStore for Store {}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use soqrates_core::{AuthorId, Session, SessionKey, StoreId};

    use super::*;

    fn session() -> Session {
        Session::open(
            SessionKey("o/r#1@abc".into()),
            AuthorId("github.com:1".into()),
            Vec::new(),
            3,
            chrono::Utc::now(),
        )
        .unwrap()
    }

    /// A grading journal for `s`, saving the profile of `learner`.
    fn journal(s: &Session, learner: &str) -> GradingJournal {
        GradingJournal {
            session: s.clone(),
            graded: GradedSession {
                key: s.key().clone(),
                author: s.author().clone(),
                correct: 1,
                total: 1,
                passed: true,
                at: s.created_at(),
            },
            reviews: Vec::new(),
            profiles: vec![(
                StoreId::LearnerStore,
                LearnerProfile::new(LearnerId(learner.into())),
            )],
            asked: None,
        }
    }

    /// Of many concurrent claims on one session, exactly one wins
    /// (ADR-0002 D6).
    async fn exactly_one_claim<S: StateStore + 'static>(store: Arc<S>) {
        let key = session().key().clone();
        store.save_session(&session()).await.unwrap();
        let handles: Vec<_> = (0..16)
            .map(|i| {
                let store = store.clone();
                let journal = journal(&session(), &format!("learner-{i}"));
                tokio::spawn(async move { store.begin_grading(&journal).await.unwrap() })
            })
            .collect();
        let mut won = 0;
        for h in handles {
            if h.await.unwrap() {
                won += 1;
            }
        }
        assert_eq!(won, 1);
        assert!(store.load_session(&key).await.unwrap().is_none());
        assert_eq!(store.gradings().await.unwrap().len(), 1);
    }

    /// One graded answer, for the reviews of session `i`.
    fn review(i: usize) -> soqrates_core::GradedAnswer {
        soqrates_core::GradedAnswer {
            concept: ConceptId(format!("concept-{i}")),
            format: soqrates_core::QuestionFormat::MultipleChoice,
            grade: soqrates_core::Grade::Good,
            at: chrono::Utc::now(),
            via: Identity::Email("ada@example.com".into()),
        }
    }

    /// Record the review of session `i` the way the engine does: load the
    /// profile, record, save if nobody saved since, else load again.
    async fn record_session<S: StateStore>(store: &S, learner: &LearnerId, i: usize) -> usize {
        let mut conflicts = 0;
        loop {
            let profile = store
                .load_profile(learner)
                .await
                .unwrap()
                .unwrap_or_else(|| LearnerProfile::new(learner.clone()));
            let (next, _) = profile
                .with_session_reviews(&format!("session-{i}"), &[review(i)])
                .expect("each session is recorded once");
            if store.save_profile_if_current(&next).await.unwrap() {
                return conflicts;
            }
            conflicts += 1;
        }
    }

    /// A conditional save is refused once the profile was saved since it was
    /// loaded, and of many writers of one profile, each one's review ends up
    /// on it: none overwrites another (ADR-0002 D6).
    async fn concurrent_writers_keep_every_review<S: StateStore + 'static>(store: Arc<S>) {
        let ada = LearnerId("ada".into());

        // A profile is new at version 0; saving it makes the next one.
        let fresh = LearnerProfile::new(ada.clone());
        assert!(store.save_profile_if_current(&fresh).await.unwrap());
        let stored = store.load_profile(&ada).await.unwrap().unwrap();
        assert_eq!(stored.version(), 1);
        // The same version again: someone saved in between.
        assert!(!store.save_profile_if_current(&fresh).await.unwrap());
        assert_eq!(store.load_profile(&ada).await.unwrap().unwrap(), stored);
        // A learner who has no profile yet cannot be saved at version 1.
        let other = LearnerProfile::new(LearnerId("bob".into())).with_version(1);
        assert!(!store.save_profile_if_current(&other).await.unwrap());
        assert!(
            store
                .load_profile(&LearnerId("bob".into()))
                .await
                .unwrap()
                .is_none()
        );

        const SESSIONS: usize = 16;
        let handles: Vec<_> = (0..SESSIONS)
            .map(|i| {
                let store = store.clone();
                let ada = ada.clone();
                tokio::spawn(async move { record_session(&*store, &ada, i).await })
            })
            .collect();
        for h in handles {
            h.await.unwrap();
        }
        let profile = store.load_profile(&ada).await.unwrap().unwrap();
        assert_eq!(profile.history().len(), SESSIONS, "no review was lost");
        assert_eq!(profile.cards().len(), SESSIONS);
        assert_eq!(profile.version(), 1 + SESSIONS as u64, "one save each");
        for i in 0..SESSIONS {
            assert!(profile.has_applied(&format!("session-{i}")));
        }
    }

    fn json<T: Serialize>(v: &T) -> serde_json::Value {
        serde_json::to_value(v).unwrap()
    }

    fn forge(account: &str) -> Identity {
        Identity::Forge {
            host: "github.com".into(),
            account: account.into(),
        }
    }

    /// A checked question about `concept`, as banked at `at`.
    fn banked(concept: &str, prompt: &str, at: chrono::DateTime<chrono::Utc>) -> BankedQuestion {
        let question = soqrates_core::Question {
            id: soqrates_core::QuestionId(format!("{concept}/mc")),
            concept: ConceptId(concept.into()),
            scope: soqrates_core::ConceptScope::General,
            kind: soqrates_core::QuestionKind::Scenario,
            body: soqrates_core::QuestionBody::MultipleChoice {
                prompt: prompt.into(),
                code: None,
                options: vec!["a".into(), "b".into()],
                options_language: None,
                correct: 0,
                reasons: Vec::new(),
            },
            explanation: "because".into(),
            checked: true,
            banked: None,
        };
        BankedQuestion::new(&question, soqrates_core::Level::Novice, at).unwrap()
    }

    /// The behaviour every `StateStore` adapter shares: records round-trip,
    /// saving again replaces, deleting removes, and links are found by
    /// learner (ADR-0001 D5, ADR-0003, ADR-0005 D4).
    async fn conforms<S: StateStore>(store: &S) {
        let now = chrono::Utc::now();
        let ada = LearnerId("ada".into());
        let bob = LearnerId("bob".into());

        // Profiles.
        assert!(store.load_profile(&ada).await.unwrap().is_none());
        let profile = LearnerProfile::new(ada.clone());
        store.save_profile(&profile).await.unwrap();
        store.save_profile(&profile).await.unwrap();
        let loaded = store.load_profile(&ada).await.unwrap().unwrap();
        assert_eq!(json(&loaded), json(&profile));
        store
            .save_profile(&LearnerProfile::new(bob.clone()))
            .await
            .unwrap();

        // Links, at most one per identity.
        let link = |identity: Identity, learner: &LearnerId| IdentityLink {
            identity,
            learner: learner.clone(),
            since: now,
        };
        store.save_link(&link(forge("1"), &ada)).await.unwrap();
        store
            .save_link(&link(Identity::Email("ada@example.com".into()), &ada))
            .await
            .unwrap();
        store.save_link(&link(forge("2"), &ada)).await.unwrap();
        store.save_link(&link(forge("2"), &bob)).await.unwrap();
        assert_eq!(
            store.learner_of(&forge("1")).await.unwrap(),
            Some(ada.clone())
        );
        assert_eq!(
            store.learner_of(&forge("2")).await.unwrap(),
            Some(bob.clone())
        );
        assert_eq!(store.learner_of(&forge("3")).await.unwrap(), None);
        let mut of_ada: Vec<String> = store
            .links_of(&ada)
            .await
            .unwrap()
            .iter()
            .map(|l| l.identity.to_string())
            .collect();
        of_ada.sort();
        assert_eq!(of_ada, ["email:ada@example.com", "github.com:1"]);
        store.delete_link(&forge("1")).await.unwrap();
        store.delete_link(&forge("1")).await.unwrap();
        assert_eq!(store.links_of(&ada).await.unwrap().len(), 1);

        // Deleting a learner removes the profile and every link, and only
        // theirs (ADR-0003 D6).
        store.delete_learner(&ada).await.unwrap();
        assert!(store.load_profile(&ada).await.unwrap().is_none());
        assert!(store.links_of(&ada).await.unwrap().is_empty());
        assert!(store.load_profile(&bob).await.unwrap().is_some());
        assert_eq!(store.links_of(&bob).await.unwrap().len(), 1);

        // Link codes.
        let code = LinkCode::issue("K7Q-29XZ".into(), forge("2"), bob.clone(), now);
        store.save_link_code(&code).await.unwrap();
        let loaded = store.load_link_code("K7Q-29XZ").await.unwrap().unwrap();
        assert_eq!(loaded, code);
        assert!(store.load_link_code("nope").await.unwrap().is_none());

        // Sessions and graded records.
        let s = session();
        store.save_session(&s).await.unwrap();
        store.save_session(&s).await.unwrap();
        let loaded = store.load_session(s.key()).await.unwrap().unwrap();
        assert_eq!(json(&loaded), json(&s));
        store.delete_session(s.key()).await.unwrap();
        assert!(store.load_session(s.key()).await.unwrap().is_none());

        // Grading journals (ADR-0002 D6): beginning to grade claims the
        // session, once, and only a pending one.
        let j = journal(&s, "ada");
        assert!(!store.begin_grading(&j).await.unwrap());
        assert!(store.load_grading(s.key()).await.unwrap().is_none());
        store.save_session(&s).await.unwrap();
        assert!(store.begin_grading(&j).await.unwrap());
        assert!(store.load_session(s.key()).await.unwrap().is_none());
        assert_eq!(store.load_grading(s.key()).await.unwrap(), Some(j.clone()));
        assert_eq!(store.gradings().await.unwrap(), vec![j.clone()]);
        assert!(!store.begin_grading(&j).await.unwrap());
        // While it is being graded, the session can't be claimed again, even
        // if it is saved again.
        store.save_session(&s).await.unwrap();
        assert!(!store.begin_grading(&j).await.unwrap());
        assert!(store.load_session(s.key()).await.unwrap().is_some());
        store.delete_session(s.key()).await.unwrap();
        store.end_grading(s.key()).await.unwrap();
        store.end_grading(s.key()).await.unwrap();
        assert!(store.load_grading(s.key()).await.unwrap().is_none());
        assert!(store.gradings().await.unwrap().is_empty());

        let graded = GradedSession {
            key: s.key().clone(),
            author: s.author().clone(),
            correct: 2,
            total: 3,
            passed: true,
            at: now,
        };
        store.save_graded(&graded).await.unwrap();
        store.save_graded(&graded).await.unwrap();
        assert_eq!(store.load_graded(s.key()).await.unwrap(), Some(graded));

        // The rounds of a pull request (ADR-0002 D9).
        let pr = |repo: &str, number| PrRef {
            repo: repo.into(),
            number,
        };
        assert!(store.load_rounds(&pr("o/r", 1)).await.unwrap().is_none());
        let mut rounds = PrRounds::new(pr("o/r", 1));
        rounds.record(soqrates_core::Round {
            head: "abc".into(),
            session: s.key().clone(),
            concepts: vec![soqrates_core::ConceptId("rust.traits".into())],
            at: now,
        });
        store.save_rounds(&rounds).await.unwrap();
        store.save_rounds(&rounds).await.unwrap();
        assert_eq!(
            store.load_rounds(&pr("o/r", 1)).await.unwrap(),
            Some(rounds)
        );
        assert!(store.load_rounds(&pr("o/r", 11)).await.unwrap().is_none());
        assert!(store.load_rounds(&pr("O/R", 1)).await.unwrap().is_none());

        // The question bank, by concept, in the order banked (ADR-0009 D1).
        let traits = ConceptId("rust.traits".into());
        assert!(store.bank_questions(&traits).await.unwrap().is_empty());
        let first = banked("rust.traits", "First?", now);
        let second = banked("rust.traits", "Second?", now + chrono::Duration::seconds(1));
        let elsewhere = banked("rust.ownership", "Other?", now);
        for q in [&second, &first, &first, &elsewhere] {
            store.save_bank_question(q).await.unwrap();
        }
        assert_eq!(
            store.bank_questions(&traits).await.unwrap(),
            vec![first.clone(), second.clone()]
        );
        let mut retired = first.clone();
        retired.retire();
        store.save_bank_question(&retired).await.unwrap();
        assert_eq!(
            store.bank_questions(&traits).await.unwrap(),
            vec![retired, second.clone()],
            "saving again replaces"
        );

        // What a learner was shown, deleted with the learner (ADR-0009 D5).
        let cy = LearnerId("cy".into());
        assert!(store.load_seen(&cy).await.unwrap().is_none());
        let mut seen = SeenQuestions::new(cy.clone());
        seen.show(first.id(), 1);
        store.save_seen(&seen).await.unwrap();
        seen.show(second.id(), 2);
        store.save_seen(&seen).await.unwrap();
        assert_eq!(store.load_seen(&cy).await.unwrap(), Some(seen));
        store.delete_learner(&cy).await.unwrap();
        assert!(store.load_seen(&cy).await.unwrap().is_none());

        // Asked sessions, kept for feedback and deleted with their learner
        // (ADR-0011 D1, D8).
        let asked_by = |key: &str, learner: &LearnerId| {
            AskedSession::new(
                SessionKey(key.into()),
                learner.clone(),
                now,
                &[first.question()],
                &[],
            )
        };
        let dee = LearnerId("dee".into());
        let mine = asked_by("o/r#2@abc", &cy);
        let theirs = asked_by("o/r#2@def", &dee);
        assert!(store.load_asked(mine.key()).await.unwrap().is_none());
        store.save_asked(&mine).await.unwrap();
        store.save_asked(&mine).await.unwrap();
        store.save_asked(&theirs).await.unwrap();
        assert_eq!(
            store.load_asked(mine.key()).await.unwrap(),
            Some(mine.clone())
        );
        let mut all = store.asked_sessions().await.unwrap();
        all.sort_by(|a, b| a.key().cmp(b.key()));
        assert_eq!(all, vec![mine.clone(), theirs.clone()]);
        store.delete_learner(&cy).await.unwrap();
        assert!(store.load_asked(mine.key()).await.unwrap().is_none());
        assert_eq!(store.asked_sessions().await.unwrap(), vec![theirs]);

        // Keys are used verbatim: no collisions between similar keys.
        for key in ["o/r#1@abc", "o/r#1@abc ", "O/R#1@ABC", "o/r#1@ab'c"] {
            let mut s = session();
            s = Session::open(
                SessionKey(key.into()),
                s.author().clone(),
                Vec::new(),
                3,
                now,
            )
            .unwrap();
            store.save_session(&s).await.unwrap();
        }
        for key in ["o/r#1@abc", "o/r#1@abc ", "O/R#1@ABC", "o/r#1@ab'c"] {
            let got = store.load_session(&SessionKey(key.into())).await.unwrap();
            assert_eq!(got.map(|s| s.key().0.clone()).as_deref(), Some(key));
            store.delete_session(&SessionKey(key.into())).await.unwrap();
        }
    }

    /// Distinguishes temporary directories made by one process.
    static DIRS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "soqrates-store-{name}-{}-{}",
            std::process::id(),
            DIRS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ))
    }

    #[tokio::test]
    async fn in_memory_conforms() {
        conforms(&InMemoryStore::new()).await;
    }

    // @adr-0005-d5
    #[tokio::test]
    async fn store_opens_the_file_store_without_a_database_url() {
        let dir = temp_dir("open");
        let store = Store::open(None, dir.clone()).await.unwrap();
        assert!(matches!(store, Store::File(_)));
        conforms(&store).await;
        let _ = std::fs::remove_dir_all(dir);
    }

    // @adr-0009-d1
    #[tokio::test]
    async fn file_conforms() {
        let dir = temp_dir("conforms");
        conforms(&FileStore::new(&dir)).await;
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn in_memory_claims_are_exclusive() {
        exactly_one_claim(Arc::new(InMemoryStore::new())).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn file_claims_are_exclusive() {
        let dir = temp_dir("claims");
        exactly_one_claim(Arc::new(FileStore::new(&dir))).await;
        let _ = std::fs::remove_dir_all(dir);
    }

    // @adr-0002-d6
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn in_memory_writers_keep_every_review() {
        concurrent_writers_keep_every_review(Arc::new(InMemoryStore::new())).await;
    }

    // @adr-0002-d6
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn file_writers_keep_every_review() {
        let dir = temp_dir("writers");
        concurrent_writers_keep_every_review(Arc::new(FileStore::new(&dir))).await;
        let _ = std::fs::remove_dir_all(dir);
    }

    // @adr-0002-d6
    #[tokio::test]
    async fn a_profile_document_without_version_or_marks_loads_and_saves() {
        let dir = temp_dir("old-profile");
        let store = FileStore::new(&dir);
        let ada = LearnerId("ada".into());
        store
            .save_profile(&LearnerProfile::new(ada.clone()))
            .await
            .unwrap();
        let path = store.path("profiles", "ada");
        let mut doc: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let fields = doc.as_object_mut().unwrap();
        fields.remove("version");
        fields.remove("applied");
        std::fs::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();

        let old = store.load_profile(&ada).await.unwrap().unwrap();
        assert_eq!(old.version(), 0);
        assert!(store.save_profile_if_current(&old).await.unwrap());
        assert_eq!(
            store.load_profile(&ada).await.unwrap().unwrap().version(),
            1
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    // --- SQL database store (ADR-0005) -------------------------------------

    #[cfg(feature = "sqlite")]
    mod sqlite {
        use std::path::Path;

        use super::*;

        fn url(dir: &Path) -> String {
            format!("sqlite://{}?mode=rwc", dir.join("state.db").display())
        }

        // @adr-0005-d5
        #[tokio::test]
        async fn store_opens_the_database_given_a_url() {
            let store = Store::open(Some("sqlite::memory:"), temp_dir("unused"))
                .await
                .unwrap();
            assert!(matches!(store, Store::Sql(_)));
            conforms(&store).await;
        }

        // @adr-0005-d1 @adr-0005-d2 @adr-0005-d6 @adr-0009-d1
        #[tokio::test]
        async fn in_memory_sqlite_conforms() {
            conforms(&SqlStore::connect("sqlite::memory:").await.unwrap()).await;
        }

        // @adr-0005-d1 @adr-0005-d3 @adr-0005-d6
        #[tokio::test]
        async fn sqlite_file_conforms_and_survives_a_reconnect() {
            let dir = temp_dir("sqlite");
            std::fs::create_dir_all(&dir).unwrap();
            let store = SqlStore::connect(&url(&dir)).await.unwrap();
            conforms(&store).await;
            store.save_session(&session()).await.unwrap();
            drop(store);
            // Connecting again finds the schema and the data.
            let store = SqlStore::connect(&url(&dir)).await.unwrap();
            assert!(store.load_session(session().key()).await.unwrap().is_some());
            let _ = std::fs::remove_dir_all(dir);
        }

        // @adr-0002-d6
        #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
        async fn sqlite_writers_keep_every_review() {
            let dir = temp_dir("sqlite-writers");
            std::fs::create_dir_all(&dir).unwrap();
            concurrent_writers_keep_every_review(Arc::new(
                SqlStore::connect(&url(&dir)).await.unwrap(),
            ))
            .await;
            let _ = std::fs::remove_dir_all(dir);
        }

        // @adr-0005-d4
        #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
        async fn sqlite_claims_are_exclusive() {
            let dir = temp_dir("sqlite-claims");
            std::fs::create_dir_all(&dir).unwrap();
            exactly_one_claim(Arc::new(SqlStore::connect(&url(&dir)).await.unwrap())).await;
            let _ = std::fs::remove_dir_all(dir);
        }

        // @adr-0005-d2
        #[tokio::test]
        async fn rows_hold_the_documents_the_file_store_writes() {
            let dir = temp_dir("same-docs");
            std::fs::create_dir_all(&dir).unwrap();
            let files = FileStore::new(dir.join("files"));
            let sql = SqlStore::connect(&url(&dir)).await.unwrap();
            let profile = LearnerProfile::new(LearnerId("ada".into()));
            files.save_profile(&profile).await.unwrap();
            sql.save_profile(&profile).await.unwrap();
            let file_doc: serde_json::Value =
                serde_json::from_slice(&std::fs::read(files.path("profiles", "ada")).unwrap())
                    .unwrap();
            let row_doc: serde_json::Value = serde_json::from_str(
                &sql.raw_doc_for_test("soqrates_profiles", "learner", "ada")
                    .await,
            )
            .unwrap();
            assert_eq!(row_doc, file_doc);
            let _ = std::fs::remove_dir_all(dir);
        }

        // @adr-0005-d3
        #[tokio::test]
        async fn a_newer_schema_is_refused() {
            let dir = temp_dir("sqlite-newer");
            std::fs::create_dir_all(&dir).unwrap();
            let store = SqlStore::connect(&url(&dir)).await.unwrap();
            store.bump_schema_for_test(SCHEMA_VERSION + 1).await;
            drop(store);
            let err = SqlStore::connect(&url(&dir)).await.unwrap_err();
            assert!(err.to_string().contains("newer"), "{err}");
            let _ = std::fs::remove_dir_all(dir);
        }

        // @adr-0005-d5
        #[tokio::test]
        async fn connection_errors_never_show_the_url() {
            let err = SqlStore::connect("sqlite:///nonexistent-dir/x/state.db?secret=hunter2")
                .await
                .unwrap_err()
                .to_string();
            assert!(!err.contains("hunter2"), "{err}");
            assert!(err.contains("SQLite"), "{err}");
        }
    }

    /// Run against a real PostgreSQL only when `SOQRATES_TEST_POSTGRES_URL`
    /// is set, e.g. `postgres://postgres@localhost/soqrates_test`; the
    /// tests pass without it, so the suite stays hermetic (ADR-0001 D12).
    #[cfg(feature = "postgres")]
    mod postgres {
        use super::*;

        async fn store() -> Option<SqlStore> {
            let url = std::env::var("SOQRATES_TEST_POSTGRES_URL").ok()?;
            let store = SqlStore::connect(&url).await.unwrap();
            store.clear_for_test().await;
            Some(store)
        }

        // @adr-0005-d4 @adr-0005-d6
        // One test, so the shared database is never used by two at once.
        #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
        async fn postgres_conforms_and_claims_are_exclusive() {
            let Some(store) = store().await else {
                eprintln!("SOQRATES_TEST_POSTGRES_URL is not set; skipping");
                return;
            };
            conforms(&store).await;
            store.clear_for_test().await;
            let store = Arc::new(store);
            // @adr-0002-d6
            concurrent_writers_keep_every_review(store.clone()).await;
            store.clear_for_test().await;
            exactly_one_claim(store).await;
            // Concurrent connects (e.g. two CI runners) create the schema once.
            let url = std::env::var("SOQRATES_TEST_POSTGRES_URL").unwrap();
            let connects: Vec<_> = (0..8)
                .map(|_| {
                    let url = url.clone();
                    tokio::spawn(async move { SqlStore::connect(&url).await.map(|_| ()) })
                })
                .collect();
            for c in connects {
                c.await.unwrap().unwrap();
            }
        }
    }
}
