//! A `DocumentStore` in a SQL database: SQLite or PostgreSQL, chosen by the
//! connection URL (ADR-0005).
//!
//! Every record is a JSON document in a table of its kind, keyed by the same
//! key the file store hex-encodes into a file name (D2). Only the key, and
//! for identity links the learner, are columns of their own, so the domain
//! types stay the single source of truth for the content. The schema is
//! created on connect (D3).

use std::time::Duration;

use soqrates_core::ports::{Doc, DocumentStore, Kind, Removal};
use soqrates_core::{Error, Result};
use sqlx::any::{AnyPoolOptions, install_default_drivers};
use sqlx::{AnyConnection, AnyPool, AssertSqlSafe, Connection, Row};

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
    // Also added within version 1 (ADR-0002 D6).
    "CREATE TABLE IF NOT EXISTS soqrates_gradings (
        session_key TEXT PRIMARY KEY,
        doc TEXT NOT NULL
    )",
    // The question bank, also within version 1 (ADR-0009 D1).
    "CREATE TABLE IF NOT EXISTS soqrates_bank (
        id TEXT PRIMARY KEY,
        concept TEXT NOT NULL,
        doc TEXT NOT NULL
    )",
    "CREATE INDEX IF NOT EXISTS soqrates_bank_by_concept ON soqrates_bank (concept)",
    "CREATE TABLE IF NOT EXISTS soqrates_seen (
        learner TEXT PRIMARY KEY,
        doc TEXT NOT NULL
    )",
    // Asked questions kept for feedback, also within version 1 (ADR-0011 D1).
    "CREATE TABLE IF NOT EXISTS soqrates_asked (
        session_key TEXT PRIMARY KEY,
        learner TEXT NOT NULL,
        doc TEXT NOT NULL
    )",
    "CREATE INDEX IF NOT EXISTS soqrates_asked_by_learner ON soqrates_asked (learner)",
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

fn to_json(doc: &Doc) -> Result<String> {
    serde_json::to_string(doc).map_err(|e| Error::Port(e.to_string()))
}

fn from_json(kind: Kind, key: &str, text: &str) -> Result<Doc> {
    serde_json::from_str(text)
        .map_err(|e| Error::Port(format!("corrupt {} {key:?}: {e}", kind.label())))
}

/// Where the documents of a kind are: the table, the column of the key and,
/// for a grouped kind, the column of the group (ADR-0005 D2). These names
/// are the stored schema: changing one orphans data.
struct Table {
    name: &'static str,
    key: &'static str,
    group: Option<&'static str>,
}

fn table(kind: Kind) -> Table {
    let (name, key, group) = match kind {
        Kind::Profile => ("soqrates_profiles", "learner", None),
        Kind::Link => ("soqrates_links", "identity", Some("learner")),
        Kind::LinkCode => ("soqrates_link_codes", "code", None),
        Kind::Session => ("soqrates_sessions", "session_key", None),
        Kind::Grading => ("soqrates_gradings", "session_key", None),
        Kind::Graded => ("soqrates_graded", "session_key", None),
        Kind::Rounds => ("soqrates_rounds", "pr", None),
        Kind::Bank => ("soqrates_bank", "id", Some("concept")),
        Kind::Seen => ("soqrates_seen", "learner", None),
        Kind::Asked => ("soqrates_asked", "session_key", Some("learner")),
    };
    Table { name, key, group }
}

impl Table {
    /// `INSERT` of a row; replaces the row of the key if `replace`, else
    /// does nothing then. Parameters: key, group if any, document.
    fn insert(&self, replace: bool) -> String {
        let (key, name) = (self.key, self.name);
        let (columns, values, update) = match self.group {
            Some(group) => (
                format!("{key}, {group}, doc"),
                "$1, $2, $3",
                format!("{group} = excluded.{group}, doc = excluded.doc"),
            ),
            None => (
                format!("{key}, doc"),
                "$1, $2",
                "doc = excluded.doc".to_string(),
            ),
        };
        let conflict = if replace {
            format!("DO UPDATE SET {update}")
        } else {
            "DO NOTHING".to_string()
        };
        format!("INSERT INTO {name} ({columns}) VALUES ({values}) ON CONFLICT ({key}) {conflict}")
    }
}

fn sql(text: String) -> AssertSqlSafe<String> {
    AssertSqlSafe(text)
}

/// The parameters of [`Table::insert`] for `doc` of `kind` with `key`.
fn insert_params(kind: Kind, key: &str, doc: &Doc) -> Result<Vec<String>> {
    let mut params = vec![key.to_string()];
    params.extend(kind.group_of(doc)?);
    params.push(to_json(doc)?);
    Ok(params)
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

    /// The key and group of every row of `table`, in key order.
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub(crate) async fn raw_rows_for_test(
        &self,
        table: &str,
        key_column: &str,
        group_column: Option<&str>,
    ) -> Vec<(String, Option<String>)> {
        let group = group_column.unwrap_or("NULL");
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {key_column}, {group}, doc FROM {table} ORDER BY {key_column}"
        )))
        .fetch_all(&self.pool)
        .await
        .unwrap()
        .iter()
        .map(|row| {
            (
                row.try_get(0).unwrap(),
                group_column.map(|_| row.try_get(1).unwrap()),
            )
        })
        .collect()
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
            "soqrates_gradings",
            "soqrates_rounds",
            "soqrates_bank",
            "soqrates_seen",
            "soqrates_asked",
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

impl DocumentStore for SqlStore {
    async fn load(&self, kind: Kind, key: &str) -> Result<Option<Doc>> {
        let t = table(kind);
        let row = sqlx::query(sql(format!(
            "SELECT doc FROM {} WHERE {} = $1",
            t.name, t.key
        )))
        .bind(key.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        match row {
            Some(row) => from_json(kind, key, &row.try_get::<String, _>(0).map_err(db)?).map(Some),
            None => Ok(None),
        }
    }

    async fn save(&self, kind: Kind, key: &str, doc: &Doc) -> Result<()> {
        let mut query = sqlx::query(sql(table(kind).insert(true)));
        for param in insert_params(kind, key, doc)? {
            query = query.bind(param);
        }
        query.execute(&self.pool).await.map_err(db)?;
        Ok(())
    }

    async fn list(&self, kind: Kind, group: Option<&str>) -> Result<Vec<Doc>> {
        let t = table(kind);
        let query = match (group, t.group) {
            (None, _) => sqlx::query(sql(format!(
                "SELECT {}, doc FROM {} ORDER BY {}",
                t.key, t.name, t.key
            ))),
            (Some(group), Some(column)) => sqlx::query(sql(format!(
                "SELECT {}, doc FROM {} WHERE {column} = $1 ORDER BY {}",
                t.key, t.name, t.key
            )))
            .bind(group.to_string()),
            // A kind that is not grouped has no group to match.
            (Some(_), None) => return Ok(Vec::new()),
        };
        let rows = query.fetch_all(&self.pool).await.map_err(db)?;
        rows.iter()
            .map(|row| {
                let key: String = row.try_get(0).map_err(db)?;
                let doc: String = row.try_get(1).map_err(db)?;
                from_json(kind, &key, &doc)
            })
            .collect()
    }

    async fn remove(&self, removals: &[Removal<'_>]) -> Result<()> {
        // All or nothing (ADR-0003 D6, ADR-0009 D5).
        let mut tx = self.pool.begin().await.map_err(db)?;
        for removal in removals {
            let (kind, column, value) = match *removal {
                Removal::Key(kind, key) => (kind, Some(table(kind).key), key),
                Removal::Group(kind, group) => (kind, table(kind).group, group),
            };
            // A kind that is not grouped has no group to remove.
            let Some(column) = column else { continue };
            sqlx::query(sql(format!(
                "DELETE FROM {} WHERE {column} = $1",
                table(kind).name
            )))
            .bind(value.to_string())
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        }
        tx.commit().await.map_err(db)
    }

    async fn swap(&self, kind: Kind, key: &str, expected: Option<&Doc>, new: &Doc) -> Result<bool> {
        // Compare and swap, as one statement: insert only if there is no
        // row, or replace the document only if it is still the stored text
        // that was compared, whatever its formatting.
        let t = table(kind);
        let stored: Option<String> = sqlx::query(sql(format!(
            "SELECT doc FROM {} WHERE {} = $1",
            t.name, t.key
        )))
        .bind(key.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?
        .map(|row| row.try_get(0).map_err(db))
        .transpose()?;
        let params = insert_params(kind, key, new)?;
        let changed = match (stored, expected) {
            (None, None) => {
                let mut insert = sqlx::query(sql(t.insert(false)));
                for param in params {
                    insert = insert.bind(param);
                }
                insert.execute(&self.pool).await
            }
            (Some(text), Some(expected)) => {
                if &from_json(kind, key, &text)? != expected {
                    return Ok(false);
                }
                let set = match t.group {
                    Some(group) => format!("doc = $1, {group} = $2"),
                    None => "doc = $1".to_string(),
                };
                let n = params.len();
                let sql_text = format!(
                    "UPDATE {} SET {set} WHERE {} = ${} AND doc = ${}",
                    t.name,
                    t.key,
                    n,
                    n + 1
                );
                // Parameters: document, group if any, key, stored text.
                let mut update = sqlx::query(sql(sql_text)).bind(params[n - 1].clone());
                if t.group.is_some() {
                    update = update.bind(params[1].clone());
                }
                update
                    .bind(key.to_string())
                    .bind(text)
                    .execute(&self.pool)
                    .await
            }
            _ => return Ok(false),
        }
        .map_err(db)?
        .rows_affected();
        Ok(changed == 1)
    }

    async fn claim(&self, from: Kind, to: Kind, key: &str, doc: &Doc) -> Result<bool> {
        // One transaction: the `from` row is deleted, which of several
        // concurrent callers only one can do, and the `to` row inserted
        // (ADR-0002 D6, ADR-0005 D4).
        let source = table(from);
        let mut tx = self.pool.begin().await.map_err(db)?;
        let claimed = sqlx::query(sql(format!(
            "DELETE FROM {} WHERE {} = $1 RETURNING doc",
            source.name, source.key
        )))
        .bind(key.to_string())
        .fetch_optional(&mut *tx)
        .await
        .map_err(db)?
        .is_some();
        if !claimed {
            tx.rollback().await.map_err(db)?;
            return Ok(false);
        }
        let mut insert = sqlx::query(sql(table(to).insert(false)));
        for param in insert_params(to, key, doc)? {
            insert = insert.bind(param);
        }
        let inserted = insert.execute(&mut *tx).await.map_err(db)?.rows_affected();
        if inserted != 1 {
            tx.rollback().await.map_err(db)?;
            return Ok(false);
        }
        tx.commit().await.map_err(db)?;
        Ok(true)
    }
}

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
