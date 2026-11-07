//! `StateStore` adapters (ADR-0001 D5, ADR-0003).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::Serialize;
use serde::de::DeserializeOwned;
use soqrates_core::ports::{LearnerStore, StateStore};
use soqrates_core::{
    Error, GradedSession, Identity, IdentityLink, LearnerId, LearnerProfile, LinkCode, Result,
    Session, SessionKey,
};

#[derive(Debug, Default)]
struct Data {
    profiles: BTreeMap<LearnerId, LearnerProfile>,
    links: BTreeMap<Identity, IdentityLink>,
    codes: BTreeMap<String, LinkCode>,
    sessions: BTreeMap<SessionKey, Session>,
    graded: BTreeMap<SessionKey, GradedSession>,
}

/// Keeps everything in memory. Useful for tests and for the MCP server's
/// ephemeral mode.
#[derive(Debug, Default)]
pub struct InMemoryStore {
    data: Mutex<Data>,
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
    async fn delete_learner(&self, learner: &LearnerId) -> Result<()> {
        self.with(|d| {
            d.profiles.remove(learner);
            d.links.retain(|_, link| &link.learner != learner);
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
    async fn take_session(&self, key: &SessionKey) -> Result<Option<Session>> {
        Ok(self.with(|d| d.sessions.remove(key)))
    }
    async fn save_graded(&self, graded: &GradedSession) -> Result<()> {
        self.with(|d| d.graded.insert(graded.key.clone(), graded.clone()));
        Ok(())
    }
    async fn load_graded(&self, key: &SessionKey) -> Result<Option<GradedSession>> {
        Ok(self.with(|d| d.graded.get(key).cloned()))
    }
    async fn delete_graded(&self, key: &SessionKey) -> Result<()> {
        self.with(|d| d.graded.remove(key));
        Ok(())
    }
}

impl LearnerStore for InMemoryStore {}

/// One JSON document per profile, link, link code and pending session,
/// under a directory. File names are the hex-encoded keys.
#[derive(Debug, Clone)]
pub struct FileStore {
    pub root: PathBuf,
}

impl FileStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn path(&self, kind: &str, key: &str) -> PathBuf {
        let name: String = key.bytes().map(|b| format!("{b:02x}")).collect();
        self.root.join(kind).join(format!("{name}.json"))
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

/// Distinguishes claims made by one process.
static CLAIMS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

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
    async fn delete_learner(&self, learner: &LearnerId) -> Result<()> {
        for link in self.links_of(learner).await? {
            self.delete_link(&link.identity).await?;
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
    async fn take_session(&self, key: &SessionKey) -> Result<Option<Session>> {
        // Claim by renaming: rename is atomic, so exactly one caller wins.
        let path = self.path("sessions", &key.0);
        let claim = path.with_extension(format!(
            "json.claimed-{}-{}",
            std::process::id(),
            CLAIMS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        match std::fs::rename(&path, &claim) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io(e)),
        }
        let bytes = std::fs::read(&claim).map_err(io)?;
        let _ = std::fs::remove_file(&claim);
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| Error::Port(format!("corrupt session {:?}: {e}", key.0)))
    }
    async fn save_graded(&self, graded: &GradedSession) -> Result<()> {
        self.write("graded", &graded.key.0, graded)
    }
    async fn load_graded(&self, key: &SessionKey) -> Result<Option<GradedSession>> {
        self.read("graded", &key.0)
    }
    async fn delete_graded(&self, key: &SessionKey) -> Result<()> {
        self.remove("graded", &key.0)
    }
}

impl LearnerStore for FileStore {}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use soqrates_core::{AuthorId, Session, SessionKey};

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

    /// Of many concurrent claims on one session, exactly one wins
    /// (ADR-0002 D6).
    async fn exactly_one_claim<S: StateStore + 'static>(store: Arc<S>) {
        let key = session().key().clone();
        store.save_session(&session()).await.unwrap();
        let handles: Vec<_> = (0..16)
            .map(|_| {
                let (store, key) = (store.clone(), key.clone());
                tokio::spawn(async move { store.take_session(&key).await.unwrap() })
            })
            .collect();
        let mut won = 0;
        for h in handles {
            if h.await.unwrap().is_some() {
                won += 1;
            }
        }
        assert_eq!(won, 1);
        assert!(store.load_session(&key).await.unwrap().is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn in_memory_claims_are_exclusive() {
        exactly_one_claim(Arc::new(InMemoryStore::new())).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn file_claims_are_exclusive() {
        let dir = std::env::temp_dir().join(format!(
            "soqrates-store-claims-{}-{}",
            std::process::id(),
            CLAIMS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        exactly_one_claim(Arc::new(FileStore::new(&dir))).await;
        let _ = std::fs::remove_dir_all(dir);
    }
}
