# ADR-0005: A SQL database store

- **Status:** Accepted
- **Date:** 2026-11-07
- **Refines:** ADR-0001 D5 (state behind `StateStore`), ADR-0003 D4 (the
  learner store)

## Context

Soqrates keeps its state in `FileStore`: one JSON file per profile, link,
link code, pending session and graded session. In CI, the directory lives on
the `soqrates-state` branch, and the workflow's concurrency group allows one
run at a time. That is enough while every writer is serialised.

ADR-0001 D5 expected "a hosted store" behind the same port, and ADR-0003 D4
describes a learner store reachable from several forges and owned by the
learner or an organisation. Several CI runners, local hooks and MCP clients
can share such a store at the same time. Files on a branch can't offer
that, and neither can a directory on one machine. A database can: it is
reachable over the network, and it makes single-statement claims atomic
between processes.

Two backends cover the cases: PostgreSQL for a shared, hosted store, and
SQLite for a local store in a single file, with the same code path.

## Decisions

### D1: One SQL adapter, `SqlStore`, for SQLite and PostgreSQL
**Enforcement:** automated

`soqrates-store` provides `SqlStore`, which implements `StateStore` and
`LearnerStore` on [sqlx](https://github.com/launchbadge/sqlx)'s `Any`
driver. The connection URL chooses the backend: `sqlite://…` or
`postgres://…`. The SQL is written once, in the subset both backends share:
`$N` parameters, `ON CONFLICT … DO UPDATE`, `DELETE … RETURNING`. The
backends are cargo features, `sqlite` and `postgres`, so a build carries
only the drivers it needs. Only `soqrates-store` may depend on `sqlx`.

### D2: Each record is a JSON document in a table of its kind
**Enforcement:** automated

The five kinds of record (profiles, links, link codes, pending sessions and
graded sessions) each get a table, keyed like the file store's files. Every
row holds the record as the same JSON document the file store writes. Only
the key, and for links the learner, are columns of their own. The domain
types stay the single definition of the content, and records move between
stores unchanged. Keys are bound as parameters, never spliced into SQL.

### D3: The schema is created on connect and versioned
**Enforcement:** automated

`SqlStore::connect` creates any missing table with idempotent statements and
records the schema version in `soqrates_schema`. A database whose version is
newer than the build's is refused rather than misread. On PostgreSQL, schema
creation holds a transaction-scoped advisory lock, so processes connecting
at the same time don't collide.

### D4: A session is claimed in one transaction
**Enforcement:** automated

`begin_grading` deletes the session row with `DELETE … RETURNING` and
inserts the grading journal in one transaction, so of several concurrent
callers, in any number of processes, exactly one claims the session, and
the claim and the journal are stored together (ADR-0002 D6). Deleting a learner removes the profile and its links in one
transaction (ADR-0003 D6).

### D5: The database is chosen by URL, from a flag or the environment only
**Enforcement:** automated

The `soqrates` binary uses `SqlStore` when `--database-url` or
`SOQRATES_DATABASE_URL` is set, and the file store otherwise. The URL is not
a configuration file setting, because a URL may carry a password and
`.soqrates.toml` is committed. The URL's user info never appears in an error
message.

### D6: Tests use SQLite always, and PostgreSQL when one is provided
**Enforcement:** automated

Every store adapter passes one conformance suite. The SQLite store runs it,
along with the acceptance scenarios on restarts, everywhere, in memory and
in a file. The suite runs against PostgreSQL when
`SOQRATES_TEST_POSTGRES_URL` points at a database it may empty. Without it,
those tests pass without doing anything, so the default test run stays
hermetic (ADR-0001 D12). `soqrates-spec` builds the `sqlite` backend only.

## Consequences

- The dogfooding workflow still keeps state on the `soqrates-state` branch.
  Moving it to a hosted PostgreSQL is a separate change: it needs a secret
  holding the URL, and a database that can be reached from Actions.
- Grading still writes the graded record and the profiles with separate
  statements (ADR-0002 D6). A database could make that one transaction,
  which would close most of the gaps in #6. That needs a port that groups
  writes, so it is not part of this decision.
- There is no tool yet to copy state from a file store into a database. The
  documents are the same, so such a tool only has to read files and insert
  rows.
- `soqrates` grows by the SQLite library and the PostgreSQL driver. The TLS
  stack is `rustls` on `aws-lc-rs`, which the GitHub client already uses.
