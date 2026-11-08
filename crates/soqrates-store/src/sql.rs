//! A `StateStore` in a SQL database: SQLite or PostgreSQL, chosen by the
//! connection URL (ADR-0005).
//!
//! Every record is a JSON document in a table of its kind, keyed by the same
//! key the file store hex-encodes into a file name (D2). Only the key, and
//! for identity links the learner, are columns of their own, so the domain
//! types stay the single source of truth for the content. The schema is
//! created on connect (D3).

use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use soqrates_core::ports::{LearnerStore, StateStore};
use soqrates_core::{
    Error, GradedSession, Identity, IdentityLink, LearnerId, LearnerProfile, LinkCode, PrRef,
    PrRounds, Result, Session, SessionKey,
};
use sqlx::any::{AnyPoolOptions, install_default_drivers};
use sqlx::{AnyConnection, AnyPool, Connection, Row};

/// The schema version this build writes. A database at a newer version was
/// written by a newer Soqrates and is refused rather than misread (D3).
pub const SCHEMA_VERSION: i64 = 1;

/// Statements creating schema version 1. Each is idempotent and valid in
/// both SQLite and PostgreSQL.
const SCHEMA: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS soqrates_schema (version BIGINT NOT NULL)",
    "CREATE TABLE IF NOT EXISTS soqrates_profiles (
        learner TEXT PRIMARY KEY,
        doc TEXT NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS soqrates_links (
        identity TEXT PRIMARY KEY,
        learner TEXT NOT NULL,
        doc TEXT NOT NULL
    )",
    "CREATE INDEX IF NOT EXISTS soqrates_links_by_learner ON soqrates_links (learner)",
    "CREATE TABLE IF NOT EXISTS soqrates_link_codes (
        code TEXT PRIMARY KEY,
        doc TEXT NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS soqrates_sessions (
        session_key TEXT PRIMARY KEY,
        doc TEXT NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS soqrates_graded (
        session_key TEXT PRIMARY KEY,
        doc TEXT NOT NULL
    )",
    // Added within version 1: a new table is invisible to older builds, so
    // they can't misread it (ADR-0002 D9).
    "CREATE TABLE IF NOT EXISTS soqrates_rounds (
        pr TEXT PRIMARY KEY,
        doc TEXT NOT NULL
    )",
];

/// Serialises schema creation between processes sharing one PostgreSQL
/// database, where concurrent `CREATE TABLE IF NOT EXISTS` can collide.
/// An arbitrary constant: "soqrates" in ASCII.
const PG_SCHEMA_LOCK: i64 = 0x736f_7172_6174_6573;

/// A learner store in SQLite (`sqlite://path/to/state.db?mode=rwc`,
/// `sqlite::memory:`) or PostgreSQL (`postgres://user@host/db`), depending
/// on the URL and on the backends compiled in (features `sqlite`,
/// `postgres`).
#[derive(Debug, Clone)]
pub struct SqlStore {
    pool: AnyPool,
}

fn db(e: sqlx::Error) -> Error {
    Error::Port(format!("database: {e}"))
}

fn to_json<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(|e| Error::Port(e.to_string()))
}

fn from_json<T: DeserializeOwned>(kind: &str, key: &str, doc: &str) -> Result<T> {
    serde_json::from_str(doc).map_err(|e| Error::Port(format!("corrupt {kind} {key:?}: {e}")))
}

impl SqlStore {
    /// Connect to the database at `url`, creating the schema if needed.
    pub async fn connect(url: &str) -> Result<Self> {
        install_default_drivers();
        let mut options = AnyPoolOptions::new().acquire_timeout(Duration::from_secs(30));
        if is_sqlite_memory(url) {
            // Each SQLite connection to `:memory:` is its own database: keep
            // exactly one, for good.
            options = options
                .max_connections(1)
                .min_connections(1)
                .idle_timeout(None)
                .max_lifetime(None);
        }
        // One direct connection first: a pool reports a refused or failed
        // connection only as a timeout, hiding the cause.
        let cannot = |e: sqlx::Error| {
            Error::Port(format!(
                "database: cannot connect to the {} database: {}",
                backend_of(url),
                redact(url, &e.to_string())
            ))
        };
        AnyConnection::connect(url)
            .await
            .map_err(cannot)?
            .close()
            .await
            .map_err(cannot)?;
        let pool = options.connect(url).await.map_err(cannot)?;
        let store = Self { pool };
        store.migrate().await?;
        Ok(store)
    }

    /// Create the schema of [`SCHEMA_VERSION`], or check that the database
    /// already has it (ADR-0005 D3).
    async fn migrate(&self) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        if tx.backend_name() == "PostgreSQL" {
            sqlx::query("SELECT pg_advisory_xact_lock($1)")
                .bind(PG_SCHEMA_LOCK)
                .execute(&mut *tx)
                .await
                .map_err(db)?;
        }
        for statement in SCHEMA {
            sqlx::query(*statement)
                .execute(&mut *tx)
                .await
                .map_err(db)?;
        }
        let found: Option<i64> = sqlx::query("SELECT MAX(version) FROM soqrates_schema")
            .fetch_one(&mut *tx)
            .await
            .map_err(db)?
            .try_get(0)
            .map_err(db)?;
        match found {
            None => {
                sqlx::query("INSERT INTO soqrates_schema (version) VALUES ($1)")
                    .bind(SCHEMA_VERSION)
                    .execute(&mut *tx)
                    .await
                    .map_err(db)?;
            }
            Some(v) if v > SCHEMA_VERSION => {
                return Err(Error::Port(format!(
                    "database: schema version {v} is newer than this Soqrates \
                     supports ({SCHEMA_VERSION}); upgrade Soqrates"
                )));
            }
            Some(_) => {}
        }
        tx.commit().await.map_err(db)
    }

    /// The document in `sql`'s single result column, if a row matched.
    async fn fetch_doc<T: DeserializeOwned>(
        &self,
        sql: &'static str,
        kind: &str,
        key: &str,
    ) -> Result<Option<T>> {
        let row = sqlx::query(sql)
            .bind(key.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(db)?;
        match row {
            Some(row) => {
                let doc: String = row.try_get(0).map_err(db)?;
                from_json(kind, key, &doc).map(Some)
            }
            None => Ok(None),
        }
    }

    /// Run `sql` with `binds`, in order.
    async fn exec(&self, sql: &'static str, binds: &[&str]) -> Result<()> {
        let mut query = sqlx::query(sql);
        for b in binds {
            query = query.bind(b.to_string());
        }
        query.execute(&self.pool).await.map_err(db)?;
        Ok(())
    }
}

#[cfg(test)]
impl SqlStore {
    /// Pretend a newer Soqrates migrated this database.
    #[cfg(feature = "sqlite")]
    pub(crate) async fn bump_schema_for_test(&self, version: i64) {
        sqlx::query("UPDATE soqrates_schema SET version = $1")
            .bind(version)
            .execute(&self.pool)
            .await
            .unwrap();
    }

    /// The stored document of `key` in `table`, as text.
    #[cfg(feature = "sqlite")]
    pub(crate) async fn raw_doc_for_test(&self, table: &str, column: &str, key: &str) -> String {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT doc FROM {table} WHERE {column} = $1"
        )))
        .bind(key.to_string())
        .fetch_one(&self.pool)
        .await
        .unwrap()
        .try_get(0)
        .unwrap()
    }

    /// Empty every table, for tests sharing one database.
    #[cfg(feature = "postgres")]
    pub(crate) async fn clear_for_test(&self) {
        for table in [
            "soqrates_profiles",
            "soqrates_links",
            "soqrates_link_codes",
            "soqrates_sessions",
            "soqrates_graded",
        ] {
            sqlx::query(sqlx::AssertSqlSafe(format!("DELETE FROM {table}")))
                .execute(&self.pool)
                .await
                .unwrap();
        }
    }
}

/// `message` without the URL's user info (user and password), should a
/// driver ever quote it.
fn redact(url: &str, message: &str) -> String {
    let userinfo = url
        .split_once("://")
        .and_then(|(_, rest)| rest.split(['/', '?', '#']).next())
        .and_then(|authority| authority.rsplit_once('@'))
        .map(|(userinfo, _)| userinfo)
        .filter(|u| !u.is_empty());
    match userinfo {
        Some(u) => message.replace(u, "***"),
        None => message.to_string(),
    }
}

/// Whether `url` names an in-memory SQLite database.
fn is_sqlite_memory(url: &str) -> bool {
    url.starts_with("sqlite:") && (url.contains(":memory:") || url.contains("mode=memory"))
}

/// The backend named by the URL scheme, for messages without the URL.
fn backend_of(url: &str) -> &str {
    match url.split_once(':').map(|(scheme, _)| scheme) {
        Some("sqlite") => "SQLite",
        Some("postgres" | "postgresql") => "PostgreSQL",
        _ => "unknown",
    }
}

impl StateStore for SqlStore {
    async fn load_profile(&self, learner: &LearnerId) -> Result<Option<LearnerProfile>> {
        self.fetch_doc(
            "SELECT doc FROM soqrates_profiles WHERE learner = $1",
            "profile",
            &learner.0,
        )
        .await
    }
    async fn save_profile(&self, profile: &LearnerProfile) -> Result<()> {
        self.exec(
            "INSERT INTO soqrates_profiles (learner, doc) VALUES ($1, $2)
             ON CONFLICT (learner) DO UPDATE SET doc = excluded.doc",
            &[&profile.learner().0, &to_json(profile)?],
        )
        .await
    }
    async fn delete_learner(&self, learner: &LearnerId) -> Result<()> {
        // Both or neither (ADR-0003 D6).
        let mut tx = self.pool.begin().await.map_err(db)?;
        for sql in [
            "DELETE FROM soqrates_links WHERE learner = $1",
            "DELETE FROM soqrates_profiles WHERE learner = $1",
        ] {
            sqlx::query(sql)
                .bind(learner.0.clone())
                .execute(&mut *tx)
                .await
                .map_err(db)?;
        }
        tx.commit().await.map_err(db)
    }

    async fn learner_of(&self, identity: &Identity) -> Result<Option<LearnerId>> {
        let link: Option<IdentityLink> = self
            .fetch_doc(
                "SELECT doc FROM soqrates_links WHERE identity = $1",
                "link",
                &identity.to_string(),
            )
            .await?;
        Ok(link.map(|l| l.learner))
    }
    async fn save_link(&self, link: &IdentityLink) -> Result<()> {
        self.exec(
            "INSERT INTO soqrates_links (identity, learner, doc) VALUES ($1, $2, $3)
             ON CONFLICT (identity) DO UPDATE SET learner = excluded.learner, doc = excluded.doc",
            &[&link.identity.to_string(), &link.learner.0, &to_json(link)?],
        )
        .await
    }
    async fn delete_link(&self, identity: &Identity) -> Result<()> {
        self.exec(
            "DELETE FROM soqrates_links WHERE identity = $1",
            &[&identity.to_string()],
        )
        .await
    }
    async fn links_of(&self, learner: &LearnerId) -> Result<Vec<IdentityLink>> {
        let rows = sqlx::query(
            "SELECT identity, doc FROM soqrates_links WHERE learner = $1 ORDER BY identity",
        )
        .bind(learner.0.clone())
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        rows.iter()
            .map(|row| {
                let identity: String = row.try_get(0).map_err(db)?;
                let doc: String = row.try_get(1).map_err(db)?;
                from_json("link", &identity, &doc)
            })
            .collect()
    }

    async fn save_link_code(&self, code: &LinkCode) -> Result<()> {
        self.exec(
            "INSERT INTO soqrates_link_codes (code, doc) VALUES ($1, $2)
             ON CONFLICT (code) DO UPDATE SET doc = excluded.doc",
            &[code.code(), &to_json(code)?],
        )
        .await
    }
    async fn load_link_code(&self, code: &str) -> Result<Option<LinkCode>> {
        self.fetch_doc(
            "SELECT doc FROM soqrates_link_codes WHERE code = $1",
            "link code",
            code,
        )
        .await
    }

    async fn load_session(&self, key: &SessionKey) -> Result<Option<Session>> {
        self.fetch_doc(
            "SELECT doc FROM soqrates_sessions WHERE session_key = $1",
            "session",
            &key.0,
        )
        .await
    }
    async fn save_session(&self, session: &Session) -> Result<()> {
        self.exec(
            "INSERT INTO soqrates_sessions (session_key, doc) VALUES ($1, $2)
             ON CONFLICT (session_key) DO UPDATE SET doc = excluded.doc",
            &[&session.key().0, &to_json(session)?],
        )
        .await
    }
    async fn delete_session(&self, key: &SessionKey) -> Result<()> {
        self.exec(
            "DELETE FROM soqrates_sessions WHERE session_key = $1",
            &[&key.0],
        )
        .await
    }
    async fn take_session(&self, key: &SessionKey) -> Result<Option<Session>> {
        // One statement: the row is deleted and returned atomically, so of
        // several concurrent callers exactly one gets it (ADR-0002 D6).
        self.fetch_doc(
            "DELETE FROM soqrates_sessions WHERE session_key = $1 RETURNING doc",
            "session",
            &key.0,
        )
        .await
    }

    async fn save_graded(&self, graded: &GradedSession) -> Result<()> {
        self.exec(
            "INSERT INTO soqrates_graded (session_key, doc) VALUES ($1, $2)
             ON CONFLICT (session_key) DO UPDATE SET doc = excluded.doc",
            &[&graded.key.0, &to_json(graded)?],
        )
        .await
    }
    async fn load_graded(&self, key: &SessionKey) -> Result<Option<GradedSession>> {
        self.fetch_doc(
            "SELECT doc FROM soqrates_graded WHERE session_key = $1",
            "graded session",
            &key.0,
        )
        .await
    }
    async fn delete_graded(&self, key: &SessionKey) -> Result<()> {
        self.exec(
            "DELETE FROM soqrates_graded WHERE session_key = $1",
            &[&key.0],
        )
        .await
    }

    async fn load_rounds(&self, pr: &PrRef) -> Result<Option<PrRounds>> {
        self.fetch_doc(
            "SELECT doc FROM soqrates_rounds WHERE pr = $1",
            "rounds",
            &pr.id(),
        )
        .await
    }
    async fn save_rounds(&self, rounds: &PrRounds) -> Result<()> {
        self.exec(
            "INSERT INTO soqrates_rounds (pr, doc) VALUES ($1, $2)
             ON CONFLICT (pr) DO UPDATE SET doc = excluded.doc",
            &[&rounds.pr().id(), &to_json(rounds)?],
        )
        .await
    }
}

impl LearnerStore for SqlStore {}

#[cfg(test)]
mod tests {
    use super::*;

    // @adr-0005-d5
    #[test]
    fn redact_removes_user_info() {
        let url = "postgres://ada:hunter2@db.example.com:5432/soqrates?sslmode=require";
        assert_eq!(
            redact(url, "failed for ada:hunter2 at db.example.com"),
            "failed for *** at db.example.com"
        );
        assert_eq!(redact("sqlite://state.db", "no such file"), "no such file");
        assert_eq!(backend_of(url), "PostgreSQL");
        assert_eq!(backend_of("sqlite::memory:"), "SQLite");
        assert!(is_sqlite_memory("sqlite::memory:"));
        assert!(!is_sqlite_memory("sqlite://state.db?mode=rwc"));
    }
}
