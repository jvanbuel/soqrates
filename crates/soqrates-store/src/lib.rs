//! `StateStore` adapters (ADR-0001 D5).

use std::path::PathBuf;

use soqrates_core::ports::StateStore;
use soqrates_core::{AuthorId, LearnerProfile, Result, Session, SessionKey};

/// Keeps everything in memory. Useful for tests and for the MCP server's
/// ephemeral mode.
#[derive(Debug, Default)]
pub struct InMemoryStore {
    _private: (),
}

impl InMemoryStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl StateStore for InMemoryStore {
    async fn load_profile(&self, author: &AuthorId) -> Result<Option<LearnerProfile>> {
        let _ = author;
        todo!("ADR-0001 D5")
    }
    async fn save_profile(&self, profile: &LearnerProfile) -> Result<()> {
        let _ = profile;
        todo!("ADR-0001 D5")
    }
    async fn load_session(&self, key: &SessionKey) -> Result<Option<Session>> {
        let _ = key;
        todo!("ADR-0001 D5")
    }
    async fn save_session(&self, session: &Session) -> Result<()> {
        let _ = session;
        todo!("ADR-0001 D5")
    }
    async fn delete_session(&self, key: &SessionKey) -> Result<()> {
        let _ = key;
        todo!("ADR-0001 D5")
    }
}

/// One JSON document per author (and per pending session) under a directory.
#[derive(Debug, Clone)]
pub struct FileStore {
    pub root: PathBuf,
}

impl FileStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl StateStore for FileStore {
    async fn load_profile(&self, author: &AuthorId) -> Result<Option<LearnerProfile>> {
        let _ = author;
        todo!("ADR-0001 D5")
    }
    async fn save_profile(&self, profile: &LearnerProfile) -> Result<()> {
        let _ = profile;
        todo!("ADR-0001 D5")
    }
    async fn load_session(&self, key: &SessionKey) -> Result<Option<Session>> {
        let _ = key;
        todo!("ADR-0001 D5")
    }
    async fn save_session(&self, session: &Session) -> Result<()> {
        let _ = session;
        todo!("ADR-0001 D5")
    }
    async fn delete_session(&self, key: &SessionKey) -> Result<()> {
        let _ = key;
        todo!("ADR-0001 D5")
    }
}
