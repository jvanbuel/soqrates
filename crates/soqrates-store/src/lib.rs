//! `DocumentStore` adapters (ADR-0001 D5, ADR-0003): in memory, JSON files,
//! and, with the `sqlite` or `postgres` feature, a SQL database (ADR-0005).
//! Each implements only the small document port of `soqrates-core`; the
//! core provides `StateStore` on top of it.

#[cfg(any(feature = "sqlite", feature = "postgres"))]
mod sql;
#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub use sql::{SCHEMA_VERSION, SqlStore};

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use soqrates_core::ports::{Doc, DocumentStore, Kind, Removal};
use soqrates_core::{Error, Result};

/// Keeps everything in memory. Useful for tests and for the MCP server's
/// ephemeral mode. One lock around everything, so every step is atomic.
#[derive(Debug, Default)]
pub struct InMemoryStore {
    docs: Mutex<BTreeMap<(Kind, String), Doc>>,
}

impl InMemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn with<T>(&self, f: impl FnOnce(&mut BTreeMap<(Kind, String), Doc>) -> T) -> T {
        f(&mut self.docs.lock().expect("store mutex poisoned"))
    }
}

/// Whether `doc` of `kind` is in `group`.
fn in_group(kind: Kind, doc: &Doc, group: &str) -> bool {
    kind.group_of(doc).ok().flatten().as_deref() == Some(group)
}

impl DocumentStore for InMemoryStore {
    async fn load(&self, kind: Kind, key: &str) -> Result<Option<Doc>> {
        Ok(self.with(|d| d.get(&(kind, key.to_string())).cloned()))
    }
    async fn save(&self, kind: Kind, key: &str, doc: &Doc) -> Result<()> {
        // A grouped document must have its group, as in every backend.
        kind.group_of(doc)?;
        self.with(|d| d.insert((kind, key.to_string()), doc.clone()));
        Ok(())
    }
    async fn list(&self, kind: Kind, group: Option<&str>) -> Result<Vec<Doc>> {
        Ok(self.with(|d| {
            d.iter()
                .filter(|((k, _), doc)| *k == kind && group.is_none_or(|g| in_group(kind, doc, g)))
                .map(|(_, doc)| doc.clone())
                .collect()
        }))
    }
    async fn remove(&self, removals: &[Removal<'_>]) -> Result<()> {
        self.with(|d| {
            for removal in removals {
                match *removal {
                    Removal::Key(kind, key) => {
                        d.remove(&(kind, key.to_string()));
                    }
                    Removal::Group(kind, group) => {
                        d.retain(|(k, _), doc| *k != kind || !in_group(kind, doc, group));
                    }
                }
            }
        });
        Ok(())
    }
    async fn swap(&self, kind: Kind, key: &str, expected: Option<&Doc>, new: &Doc) -> Result<bool> {
        kind.group_of(new)?;
        Ok(self.with(|d| {
            let slot = (kind, key.to_string());
            if d.get(&slot) != expected {
                return false;
            }
            d.insert(slot, new.clone());
            true
        }))
    }
    async fn claim(&self, from: Kind, to: Kind, key: &str, doc: &Doc) -> Result<bool> {
        to.group_of(doc)?;
        Ok(self.with(|d| {
            let target = (to, key.to_string());
            if d.contains_key(&target) || d.remove(&(from, key.to_string())).is_none() {
                return false;
            }
            d.insert(target, doc.clone());
            true
        }))
    }
}

/// One JSON document per record, under a directory: `<kind>/<hex key>.json`
/// with the kinds `profiles`, `links`, `codes`, `sessions`, `gradings`,
/// `graded`, `rounds` and `seen`; bank questions are in a directory per
/// concept, `bank/<hex concept>/<hex id>.json`. File names are the
/// hex-encoded keys.
#[derive(Debug, Clone)]
pub struct FileStore {
    pub root: PathBuf,
}

/// The directory of `kind` under the root of a [`FileStore`].
fn dir_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Profile => "profiles",
        Kind::Link => "links",
        Kind::LinkCode => "codes",
        Kind::Session => "sessions",
        Kind::Grading => "gradings",
        Kind::Graded => "graded",
        Kind::Rounds => "rounds",
        Kind::Bank => "bank",
        Kind::Seen => "seen",
        Kind::Asked => "asked",
    }
}

impl FileStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The file of the document of `kind` with `key`, which for a bank
    /// question is in the directory of its `group` (ADR-0009 D1).
    fn path(&self, kind: Kind, key: &str, group: Option<&str>) -> PathBuf {
        let dir = self.root.join(dir_name(kind));
        let dir = match (kind, group) {
            (Kind::Bank, Some(concept)) => dir.join(hex(concept)),
            _ => dir,
        };
        dir.join(format!("{}.json", hex(key)))
    }

    /// The file of the stored document of `kind` with `key`, if any. A bank
    /// question's group is not known by its key, so its directories are
    /// searched.
    fn find(&self, kind: Kind, key: &str) -> Result<Option<PathBuf>> {
        if kind != Kind::Bank {
            let path = self.path(kind, key, None);
            return Ok(path.exists().then_some(path));
        }
        let file = format!("{}.json", hex(key));
        for dir in self.subdirs(kind)? {
            if dir.join(&file).exists() {
                return Ok(Some(dir.join(file)));
            }
        }
        Ok(None)
    }

    fn subdirs(&self, kind: Kind) -> Result<Vec<PathBuf>> {
        let Some(entries) = read_dir(&self.root.join(dir_name(kind)))? else {
            return Ok(Vec::new());
        };
        let mut dirs = Vec::new();
        for entry in entries {
            let path = entry.map_err(io)?.path();
            if path.is_dir() {
                dirs.push(path);
            }
        }
        Ok(dirs)
    }

    fn read_doc(path: &Path) -> Result<Option<Doc>> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|e| Error::Port(format!("corrupt {}: {e}", path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io(e)),
        }
    }

    fn write_doc(path: &Path, doc: &Doc) -> Result<()> {
        std::fs::create_dir_all(path.parent().expect("has parent")).map_err(io)?;
        let json = serde_json::to_vec_pretty(doc).map_err(|e| Error::Port(e.to_string()))?;
        // Write then rename, so a crash never leaves a half-written file.
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json).map_err(io)?;
        std::fs::rename(&tmp, path).map_err(io)
    }

    fn remove_file(path: &Path) -> Result<()> {
        match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(io(e)),
            _ => Ok(()),
        }
    }

    /// The files and documents of `kind`, or of `group`, in file name order.
    fn entries(&self, kind: Kind, group: Option<&str>) -> Result<Vec<(PathBuf, Doc)>> {
        let dirs = match (kind, group) {
            (Kind::Bank, Some(concept)) => vec![self.root.join("bank").join(hex(concept))],
            (Kind::Bank, None) => self.subdirs(kind)?,
            _ => vec![self.root.join(dir_name(kind))],
        };
        let mut out = Vec::new();
        for dir in dirs {
            let Some(entries) = read_dir(&dir)? else {
                continue;
            };
            for entry in entries {
                let path = entry.map_err(io)?.path();
                if path.extension().is_some_and(|x| x == "json")
                    && let Some(doc) = Self::read_doc(&path)?
                    && (kind == Kind::Bank || group.is_none_or(|g| in_group(kind, &doc, g)))
                {
                    out.push((path, doc));
                }
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }

    /// Hold the advisory lock of `kind`'s directory until the returned file
    /// is dropped. Between processes it makes checking and writing one
    /// step; the OS releases it if the process stops.
    fn lock(&self, kind: Kind) -> Result<std::fs::File> {
        let dir = self.root.join(dir_name(kind));
        std::fs::create_dir_all(&dir).map_err(io)?;
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join(".lock"))
            .map_err(io)?;
        lock.lock().map_err(io)?;
        Ok(lock)
    }

    /// Write `doc` as the document of `kind` with `key`, moving a bank
    /// question whose concept changed.
    fn put(&self, kind: Kind, key: &str, doc: &Doc) -> Result<()> {
        let group = kind.group_of(doc)?;
        let path = self.path(kind, key, group.as_deref());
        let old = if kind == Kind::Bank {
            self.find(kind, key)?
        } else {
            None
        };
        Self::write_doc(&path, doc)?;
        match old {
            Some(old) if old != path => Self::remove_file(&old),
            _ => Ok(()),
        }
    }
}

fn read_dir(dir: &Path) -> Result<Option<std::fs::ReadDir>> {
    match std::fs::read_dir(dir) {
        Ok(entries) => Ok(Some(entries)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io(e)),
    }
}

fn hex(key: &str) -> String {
    key.bytes().map(|b| format!("{b:02x}")).collect()
}

fn io(e: std::io::Error) -> Error {
    Error::Port(e.to_string())
}

impl DocumentStore for FileStore {
    async fn load(&self, kind: Kind, key: &str) -> Result<Option<Doc>> {
        match self.find(kind, key)? {
            Some(path) => Self::read_doc(&path),
            None => Ok(None),
        }
    }
    async fn save(&self, kind: Kind, key: &str, doc: &Doc) -> Result<()> {
        self.put(kind, key, doc)
    }
    async fn list(&self, kind: Kind, group: Option<&str>) -> Result<Vec<Doc>> {
        Ok(self
            .entries(kind, group)?
            .into_iter()
            .map(|(_, doc)| doc)
            .collect())
    }
    async fn remove(&self, removals: &[Removal<'_>]) -> Result<()> {
        for removal in removals {
            match *removal {
                Removal::Key(kind, key) => {
                    if let Some(path) = self.find(kind, key)? {
                        Self::remove_file(&path)?;
                    }
                }
                Removal::Group(kind, group) => {
                    for (path, _) in self.entries(kind, Some(group))? {
                        Self::remove_file(&path)?;
                    }
                }
            }
        }
        Ok(())
    }
    async fn swap(&self, kind: Kind, key: &str, expected: Option<&Doc>, new: &Doc) -> Result<bool> {
        let _lock = self.lock(kind)?;
        let stored = match self.find(kind, key)? {
            Some(path) => Self::read_doc(&path)?,
            None => None,
        };
        if stored.as_ref() != expected {
            return Ok(false);
        }
        self.put(kind, key, new)?;
        Ok(true)
    }
    async fn claim(&self, from: Kind, to: Kind, key: &str, doc: &Doc) -> Result<bool> {
        let _lock = self.lock(to)?;
        let Some(pending) = self.find(from, key)? else {
            return Ok(false);
        };
        if self.find(to, key)?.is_some() {
            return Ok(false);
        }
        // `to` first: a stop before `from` is removed leaves both, and
        // finishing the journal removes the session (ADR-0002 D6).
        self.put(to, key, doc)?;
        Self::remove_file(&pending)?;
        Ok(true)
    }
}

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

impl DocumentStore for Store {
    async fn load(&self, kind: Kind, key: &str) -> Result<Option<Doc>> {
        delegate!(self, load(kind, key))
    }
    async fn save(&self, kind: Kind, key: &str, doc: &Doc) -> Result<()> {
        delegate!(self, save(kind, key, doc))
    }
    async fn list(&self, kind: Kind, group: Option<&str>) -> Result<Vec<Doc>> {
        delegate!(self, list(kind, group))
    }
    async fn remove(&self, removals: &[Removal<'_>]) -> Result<()> {
        delegate!(self, remove(removals))
    }
    async fn swap(&self, kind: Kind, key: &str, expected: Option<&Doc>, new: &Doc) -> Result<bool> {
        delegate!(self, swap(kind, key, expected, new))
    }
    async fn claim(&self, from: Kind, to: Kind, key: &str, doc: &Doc) -> Result<bool> {
        delegate!(self, claim(from, to, key, doc))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde::Serialize;
    use soqrates_core::ports::StateStore;
    use soqrates_core::{
        AskedSession, AuthorId, BankedQuestion, ConceptId, GradedSession, GradingJournal, Identity,
        IdentityLink, LearnerId, LearnerProfile, LinkCode, PrRef, PrRounds, SeenQuestions, Session,
        SessionKey, StoreId,
    };

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
        rounds.record(soqrates_core::Round::own(
            "abc".into(),
            s.key().clone(),
            vec![soqrates_core::ConceptId("rust.traits".into())],
            now,
        ));
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

        // A stale selection or feedback write cannot replace a newer dispute.
        let old = second.clone();
        let mut disputed = old.clone();
        let id = soqrates_core::FeedbackId("first-dispute".into());
        disputed.dispute(&id);
        assert!(
            store
                .save_bank_question_if_current(&disputed, Some(&old))
                .await
                .unwrap()
        );
        let mut stale = old.clone();
        stale.take();
        assert!(
            !store
                .save_bank_question_if_current(&stale, Some(&old))
                .await
                .unwrap()
        );
        let mut latest = disputed.clone();
        latest.dispute(&soqrates_core::FeedbackId("second-dispute".into()));
        assert!(
            store
                .save_bank_question_if_current(&latest, Some(&disputed))
                .await
                .unwrap()
        );
        latest.settle(&id);
        assert!(latest.is_disputed(), "the second dispute survives");
        latest.settle(&soqrates_core::FeedbackId("second-dispute".into()));
        latest.dispute(&id);
        assert!(
            !latest.is_disputed(),
            "a delayed journal cannot reopen resolved feedback"
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
        let mut changed = mine.clone();
        changed
            .give(
                &cy,
                &soqrates_core::FeedbackRequest {
                    question: soqrates_core::QuestionRef::Number(1),
                    kind: soqrates_core::FeedbackKind::Disagree,
                    text: None,
                },
                Some("comment-1"),
                now,
            )
            .unwrap();
        assert!(
            store
                .save_asked_if_current(&changed, Some(&mine))
                .await
                .unwrap()
        );
        assert!(
            !store.save_asked_if_current(&mine, None).await.unwrap(),
            "grading replay preserves feedback"
        );
        assert!(
            !store
                .save_asked_if_current(&mine, Some(&mine))
                .await
                .unwrap(),
            "stale sessions cannot overwrite feedback"
        );
        assert_eq!(store.load_asked(mine.key()).await.unwrap(), Some(changed));
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

    struct StoreExaminer;
    impl soqrates_core::ports::Examiner for StoreExaminer {
        async fn extract_concepts(
            &self,
            _: &soqrates_core::Diff,
            _: &impl soqrates_core::ports::Codebase,
        ) -> Result<Vec<soqrates_core::Concept>> {
            unreachable!()
        }
        async fn write_question(
            &self,
            _: &soqrates_core::Concept,
            _: &soqrates_core::QuestionBrief,
            _: &impl soqrates_core::ports::Codebase,
        ) -> Result<soqrates_core::Question> {
            unreachable!()
        }
        async fn grade(
            &self,
            _: &soqrates_core::Question,
            _: &soqrates_core::Answer,
            _: &impl soqrates_core::ports::Codebase,
        ) -> Result<soqrates_core::Grading> {
            unreachable!()
        }
    }
    struct TestClock;
    impl soqrates_core::ports::Clock for TestClock {
        fn now(&self) -> chrono::DateTime<chrono::Utc> {
            chrono::Utc::now()
        }
    }

    #[tokio::test]
    async fn feedback_recovers_after_bank_write_failure_without_resubmission() {
        use soqrates_core::{FeedbackKind, FeedbackRequest, QuestionRef, Soqrates};
        let root = temp_dir("feedback-recovery");
        let store = FileStore::new(&root);
        let now = chrono::Utc::now();
        let bank = banked("rust.traits", "Question?", now);
        store.save_bank_question(&bank).await.unwrap();
        let asked = AskedSession::new(
            SessionKey("session".into()),
            LearnerId("learner".into()),
            now,
            &[bank.question()],
            &[],
        );
        store.save_asked(&asked).await.unwrap();
        let app = Soqrates::new(store, StoreExaminer, TestClock, Default::default());
        // Prevent the bank CAS lock from opening after the session is saved.
        let lock = root.join("bank").join(".lock");
        std::fs::create_dir(&lock).unwrap();
        let request = FeedbackRequest {
            question: QuestionRef::Number(1),
            kind: FeedbackKind::Disagree,
            text: None,
        };
        assert!(
            app.give_feedback(None, asked.key(), &request, Some("source"))
                .await
                .is_err()
        );
        let saved = app.store.load_asked(asked.key()).await.unwrap().unwrap();
        assert_eq!(saved.questions()[0].feedback.len(), 1);
        let id = saved.questions()[0].feedback[0].id.clone();
        std::fs::remove_dir(&lock).unwrap();
        // Any later operation recovers; the original request is never retried.
        app.export_questions(true).await.unwrap();
        assert!(app.store.bank_questions(bank.concept()).await.unwrap()[0].is_disputed());
        std::fs::remove_file(&lock).unwrap();
        std::fs::create_dir(&lock).unwrap();
        assert!(app.resolve_feedback(&id, true).await.is_err());
        // A concurrent opposite verdict must retain the irreversible retirement intent.
        let accepted = app.store.load_asked(asked.key()).await.unwrap().unwrap();
        let mut rejected = accepted.clone();
        rejected.resolve(&id, false).unwrap();
        assert!(
            app.store
                .save_asked_if_current(&rejected, Some(&accepted))
                .await
                .unwrap()
        );
        std::fs::remove_dir(&lock).unwrap();
        app.export_questions(true).await.unwrap();
        let recovered = &app.store.bank_questions(bank.concept()).await.unwrap()[0];
        assert!(recovered.is_retired());
        assert!(!recovered.is_disputed());
        std::fs::remove_dir_all(root).unwrap();
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
        let path = store.path(Kind::Profile, "ada", None);
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

    /// The stored schema, pinned: the table, key column and group column
    /// of each kind, with the key and group of what `populate` saved. A
    /// change here orphans the rows of every existing database.
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    async fn schema_is_pinned(store: &SqlStore) {
        populate(store).await;
        let id = banked("rust.traits", "First?", chrono::Utc::now())
            .id()
            .0
            .clone();
        for (table, key_column, group_column, key, group) in [
            ("soqrates_profiles", "learner", None, "ada", None),
            (
                "soqrates_links",
                "identity",
                Some("learner"),
                "github.com:1",
                Some("ada"),
            ),
            ("soqrates_link_codes", "code", None, "K7Q-29XZ", None),
            ("soqrates_sessions", "session_key", None, "o/r#1@abc", None),
            ("soqrates_gradings", "session_key", None, "o/r#2@def", None),
            ("soqrates_graded", "session_key", None, "o/r#1@abc", None),
            ("soqrates_rounds", "pr", None, "o/r#1", None),
            (
                "soqrates_bank",
                "id",
                Some("concept"),
                id.as_str(),
                Some("rust.traits"),
            ),
            ("soqrates_seen", "learner", None, "cy", None),
        ] {
            let rows = store
                .raw_rows_for_test(table, key_column, group_column)
                .await;
            assert_eq!(
                rows,
                vec![(key.to_string(), group.map(str::to_string))],
                "{table}"
            );
        }
    }

    // --- The document port ---------------------------------------------------

    fn link_doc(identity: &str, learner: &str) -> Doc {
        serde_json::json!({ "identity": identity, "learner": learner, "since": "2026-01-01T00:00:00Z" })
    }

    fn bank_doc(concept: &str, n: u32) -> Doc {
        serde_json::json!({ "question": { "concept": concept }, "n": n })
    }

    fn sorted(mut docs: Vec<Doc>) -> Vec<Doc> {
        docs.sort_by_key(|d| d.to_string());
        docs
    }

    /// What every `DocumentStore` promises, method by method.
    async fn document_port_conforms<D: DocumentStore>(store: &D) {
        use serde_json::json;

        // load and save: documents round-trip, saving replaces, kinds and
        // keys are independent, and keys are used verbatim.
        assert_eq!(store.load(Kind::Profile, "a").await.unwrap(), None);
        store
            .save(Kind::Profile, "a", &json!({"v": 1}))
            .await
            .unwrap();
        store
            .save(Kind::Profile, "a", &json!({"v": 2}))
            .await
            .unwrap();
        store
            .save(Kind::Seen, "a", &json!({"seen": true}))
            .await
            .unwrap();
        for key in ["A", "a ", "a'b", "a/b", "../a"] {
            store
                .save(Kind::Profile, key, &json!({"key": key}))
                .await
                .unwrap();
        }
        assert_eq!(
            store.load(Kind::Profile, "a").await.unwrap(),
            Some(json!({"v": 2}))
        );
        assert_eq!(
            store.load(Kind::Seen, "a").await.unwrap(),
            Some(json!({"seen": true}))
        );
        for key in ["A", "a ", "a'b", "a/b", "../a"] {
            assert_eq!(
                store.load(Kind::Profile, key).await.unwrap(),
                Some(json!({"key": key}))
            );
        }
        assert_eq!(store.load(Kind::Session, "a").await.unwrap(), None);

        // list: a kind's documents, and for a grouped kind only a group's.
        assert_eq!(store.list(Kind::Profile, None).await.unwrap().len(), 6);
        assert!(store.list(Kind::Rounds, None).await.unwrap().is_empty());
        assert!(
            store
                .list(Kind::Profile, Some("a"))
                .await
                .unwrap()
                .is_empty(),
            "a kind that is not grouped has no group"
        );
        let (ada1, ada2, bob1) = (
            link_doc("github.com:1", "ada"),
            link_doc("email:ada@example.com", "ada"),
            link_doc("github.com:2", "bob"),
        );
        for (key, doc) in [("l1", &ada1), ("l2", &ada2), ("l3", &bob1)] {
            store.save(Kind::Link, key, doc).await.unwrap();
        }
        assert_eq!(
            sorted(store.list(Kind::Link, None).await.unwrap()),
            sorted(vec![ada1.clone(), ada2.clone(), bob1.clone()])
        );
        assert_eq!(
            sorted(store.list(Kind::Link, Some("ada")).await.unwrap()),
            sorted(vec![ada1.clone(), ada2.clone()])
        );
        assert!(store.list(Kind::Link, Some("cy")).await.unwrap().is_empty());
        // The group is saved with the document and replaced with it.
        store
            .save(Kind::Link, "l2", &link_doc("email:ada@example.com", "bob"))
            .await
            .unwrap();
        assert_eq!(
            store.list(Kind::Link, Some("ada")).await.unwrap(),
            vec![ada1.clone()]
        );
        assert_eq!(store.list(Kind::Link, Some("bob")).await.unwrap().len(), 2);
        // A document of a grouped kind needs its group.
        assert!(
            store
                .save(Kind::Link, "bad", &json!({"identity": "x"}))
                .await
                .is_err()
        );
        assert_eq!(store.load(Kind::Link, "bad").await.unwrap(), None);

        // Bank questions are found by key, wherever their group is.
        let (t1, t2, o1) = (
            bank_doc("traits", 1),
            bank_doc("traits", 2),
            bank_doc("own", 3),
        );
        for (key, doc) in [("q1", &t1), ("q2", &t2), ("q3", &o1)] {
            store.save(Kind::Bank, key, doc).await.unwrap();
        }
        assert_eq!(
            store.load(Kind::Bank, "q3").await.unwrap(),
            Some(o1.clone())
        );
        assert_eq!(
            sorted(store.list(Kind::Bank, Some("traits")).await.unwrap()),
            sorted(vec![t1.clone(), t2.clone()])
        );
        assert_eq!(store.list(Kind::Bank, None).await.unwrap().len(), 3);
        store
            .save(Kind::Bank, "q1", &bank_doc("own", 4))
            .await
            .unwrap();
        assert_eq!(
            store.list(Kind::Bank, Some("traits")).await.unwrap(),
            vec![t2.clone()]
        );
        assert_eq!(store.list(Kind::Bank, Some("own")).await.unwrap().len(), 2);
        assert_eq!(store.list(Kind::Bank, None).await.unwrap().len(), 3);

        // remove: by key or by group, what is not there is skipped, and all
        // of them are removed.
        store
            .remove(&[
                Removal::Key(Kind::Bank, "q3"),
                Removal::Key(Kind::Bank, "nope"),
                Removal::Key(Kind::Rounds, "nope"),
                Removal::Group(Kind::Rounds, "nope"),
                Removal::Group(Kind::Profile, "a"),
            ])
            .await
            .unwrap();
        assert_eq!(store.load(Kind::Bank, "q3").await.unwrap(), None);
        assert!(store.load(Kind::Bank, "q1").await.unwrap().is_some());
        assert!(store.load(Kind::Profile, "a").await.unwrap().is_some());
        store
            .remove(&[
                Removal::Group(Kind::Link, "bob"),
                Removal::Key(Kind::Seen, "a"),
                Removal::Key(Kind::Profile, "a"),
            ])
            .await
            .unwrap();
        assert_eq!(
            store.list(Kind::Link, None).await.unwrap(),
            vec![ada1.clone()]
        );
        assert_eq!(store.load(Kind::Seen, "a").await.unwrap(), None);
        assert_eq!(store.load(Kind::Profile, "a").await.unwrap(), None);
        assert!(store.load(Kind::Profile, "A").await.unwrap().is_some());
        store.remove(&[]).await.unwrap();

        // swap: only from the expected document, or from none.
        let (one, two, three) = (json!({"n": 1}), json!({"n": 2}), json!({"n": 3}));
        assert!(
            !store
                .swap(Kind::Rounds, "r", Some(&one), &two)
                .await
                .unwrap()
        );
        assert_eq!(store.load(Kind::Rounds, "r").await.unwrap(), None);
        assert!(store.swap(Kind::Rounds, "r", None, &one).await.unwrap());
        assert!(!store.swap(Kind::Rounds, "r", None, &two).await.unwrap());
        assert!(
            !store
                .swap(Kind::Rounds, "r", Some(&two), &three)
                .await
                .unwrap()
        );
        assert_eq!(
            store.load(Kind::Rounds, "r").await.unwrap(),
            Some(one.clone())
        );
        assert!(
            store
                .swap(Kind::Rounds, "r", Some(&one), &two)
                .await
                .unwrap()
        );
        assert!(
            !store
                .swap(Kind::Rounds, "r", Some(&one), &three)
                .await
                .unwrap()
        );
        assert_eq!(
            store.load(Kind::Rounds, "r").await.unwrap(),
            Some(two.clone())
        );
        // A swap moves the group too.
        assert!(
            store
                .swap(Kind::Link, "l1", Some(&ada1), &bob1)
                .await
                .unwrap()
        );
        assert!(
            store
                .list(Kind::Link, Some("ada"))
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store.list(Kind::Link, Some("bob")).await.unwrap(),
            vec![bob1.clone()]
        );

        // claim: moves a document to another kind if there is one to move
        // and nothing in the way.
        let claimed = json!({"claimed": true});
        assert!(
            !store
                .claim(Kind::Session, Kind::Grading, "s", &claimed)
                .await
                .unwrap()
        );
        assert_eq!(store.load(Kind::Grading, "s").await.unwrap(), None);
        store.save(Kind::Session, "s", &one).await.unwrap();
        assert!(
            store
                .claim(Kind::Session, Kind::Grading, "s", &claimed)
                .await
                .unwrap()
        );
        assert_eq!(store.load(Kind::Session, "s").await.unwrap(), None);
        assert_eq!(
            store.load(Kind::Grading, "s").await.unwrap(),
            Some(claimed.clone())
        );
        assert!(
            !store
                .claim(Kind::Session, Kind::Grading, "s", &two)
                .await
                .unwrap()
        );
        store.save(Kind::Session, "s", &one).await.unwrap();
        assert!(
            !store
                .claim(Kind::Session, Kind::Grading, "s", &two)
                .await
                .unwrap()
        );
        assert_eq!(
            store.load(Kind::Session, "s").await.unwrap(),
            Some(one.clone())
        );
        assert_eq!(store.load(Kind::Grading, "s").await.unwrap(), Some(claimed));
    }

    /// Of many concurrent swaps from one document, exactly one succeeds;
    /// of many concurrent claims, too.
    async fn document_port_is_atomic<D: DocumentStore + 'static>(store: Arc<D>) {
        use serde_json::json;
        let start = json!({"n": 0});
        assert!(store.swap(Kind::Profile, "p", None, &start).await.unwrap());
        let swaps: Vec<_> = (0..16)
            .map(|i| {
                let (store, start) = (store.clone(), start.clone());
                tokio::spawn(async move {
                    store
                        .swap(Kind::Profile, "p", Some(&start), &json!({"n": i + 1}))
                        .await
                        .unwrap()
                })
            })
            .collect();
        let mut won = 0;
        for h in swaps {
            won += usize::from(h.await.unwrap());
        }
        assert_eq!(won, 1, "one swap from one document");
        // Of many concurrent first writers, one inserts.
        let inserts: Vec<_> = (0..16)
            .map(|i| {
                let store = store.clone();
                tokio::spawn(async move {
                    store
                        .swap(Kind::Profile, "fresh", None, &json!({"n": i}))
                        .await
                        .unwrap()
                })
            })
            .collect();
        let mut won = 0;
        for h in inserts {
            won += usize::from(h.await.unwrap());
        }
        assert_eq!(won, 1, "one insert of one document");

        store.save(Kind::Session, "s", &start).await.unwrap();
        let claims: Vec<_> = (0..16)
            .map(|i| {
                let store = store.clone();
                tokio::spawn(async move {
                    store
                        .claim(Kind::Session, Kind::Grading, "s", &json!({"by": i}))
                        .await
                        .unwrap()
                })
            })
            .collect();
        let mut won = 0;
        for h in claims {
            won += usize::from(h.await.unwrap());
        }
        assert_eq!(won, 1, "one claim of one document");
        assert_eq!(store.load(Kind::Session, "s").await.unwrap(), None);
        assert_eq!(store.list(Kind::Grading, None).await.unwrap().len(), 1);
    }

    /// One record of every kind, through `StateStore`, for the layout tests.
    async fn populate<S: StateStore>(store: &S) {
        let now = chrono::Utc::now();
        let ada = LearnerId("ada".into());
        store
            .save_profile(&LearnerProfile::new(ada.clone()))
            .await
            .unwrap();
        store
            .save_link(&IdentityLink {
                identity: forge("1"),
                learner: ada.clone(),
                since: now,
            })
            .await
            .unwrap();
        let code = LinkCode::issue("K7Q-29XZ".into(), forge("2"), ada.clone(), now);
        store.save_link_code(&code).await.unwrap();
        let s = session();
        store.save_session(&s).await.unwrap();
        let other = Session::open(
            SessionKey("o/r#2@def".into()),
            s.author().clone(),
            Vec::new(),
            3,
            now,
        )
        .unwrap();
        store.save_session(&other).await.unwrap();
        assert!(store.begin_grading(&journal(&other, "ada")).await.unwrap());
        store
            .save_graded(&GradedSession {
                key: s.key().clone(),
                author: s.author().clone(),
                correct: 1,
                total: 1,
                passed: true,
                at: now,
            })
            .await
            .unwrap();
        let pr = PrRef {
            repo: "o/r".into(),
            number: 1,
        };
        store.save_rounds(&PrRounds::new(pr)).await.unwrap();
        store
            .save_bank_question(&banked("rust.traits", "First?", now))
            .await
            .unwrap();
        store
            .save_seen(&SeenQuestions::new(LearnerId("cy".into())))
            .await
            .unwrap();
    }

    /// The stored layout of the file store, pinned: a change here orphans
    /// the files of every existing installation.
    #[tokio::test]
    async fn file_layout_is_pinned() {
        let dir = temp_dir("layout");
        let store = FileStore::new(&dir);
        populate(&store).await;
        // Take the lock files of swaps and claims out of the picture.
        let mut files = Vec::new();
        let mut pending = vec![dir.clone()];
        while let Some(d) = pending.pop() {
            for entry in std::fs::read_dir(&d).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    pending.push(path);
                } else {
                    files.push(
                        path.strip_prefix(&dir)
                            .unwrap()
                            .to_string_lossy()
                            .into_owned(),
                    );
                }
            }
        }
        files.sort();
        let id = hex(&banked("rust.traits", "First?", chrono::Utc::now()).id().0);
        let mut expected = vec![
            "profiles/616461.json".to_string(),
            "links/6769746875622e636f6d3a31.json".into(),
            "codes/4b37512d3239585a.json".into(),
            "sessions/6f2f72233140616263.json".into(),
            "gradings/6f2f72233240646566.json".into(),
            "graded/6f2f72233140616263.json".into(),
            "rounds/6f2f722331.json".into(),
            format!("bank/727573742e747261697473/{id}.json"),
            "seen/6379.json".into(),
            // Sessions are claimed under this lock, which is kept.
            "gradings/.lock".into(),
        ];
        expected.sort();
        assert_eq!(files, expected);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn in_memory_document_port_conforms() {
        document_port_conforms(&InMemoryStore::new()).await;
    }

    #[tokio::test]
    async fn file_document_port_conforms() {
        let dir = temp_dir("doc-conforms");
        document_port_conforms(&FileStore::new(&dir)).await;
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn in_memory_document_port_is_atomic() {
        document_port_is_atomic(Arc::new(InMemoryStore::new())).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn file_document_port_is_atomic() {
        let dir = temp_dir("doc-atomic");
        document_port_is_atomic(Arc::new(FileStore::new(&dir))).await;
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

        #[tokio::test]
        async fn sqlite_document_port_conforms() {
            document_port_conforms(&SqlStore::connect("sqlite::memory:").await.unwrap()).await;
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
        async fn sqlite_document_port_is_atomic() {
            let dir = temp_dir("sqlite-doc-atomic");
            std::fs::create_dir_all(&dir).unwrap();
            document_port_is_atomic(Arc::new(SqlStore::connect(&url(&dir)).await.unwrap())).await;
            let _ = std::fs::remove_dir_all(dir);
        }

        // @adr-0005-d2
        #[tokio::test]
        async fn sqlite_schema_is_pinned() {
            schema_is_pinned(&SqlStore::connect("sqlite::memory:").await.unwrap()).await;
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
            let file_doc: serde_json::Value = serde_json::from_slice(
                &std::fs::read(files.path(Kind::Profile, "ada", None)).unwrap(),
            )
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
            document_port_conforms(&store).await;
            store.clear_for_test().await;
            schema_is_pinned(&store).await;
            store.clear_for_test().await;
            let store = Arc::new(store);
            document_port_is_atomic(store.clone()).await;
            store.clear_for_test().await;
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
