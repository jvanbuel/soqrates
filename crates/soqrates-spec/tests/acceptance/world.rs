//! The cucumber world: one Soqrates engine wired to hermetic fakes.

use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use cucumber::World;
use soqrates_core::ports::{NoCodebase, StateStore};
use soqrates_core::{
    AuthorId, CheckStatus, Config, Diff, HookDecision, HookKind, LearnerEvent, LearnerProfile,
    Level, PrRef, Question, Session, SessionKey, SessionOutcome, SessionResult, Soqrates,
};

use crate::fakes::{AnyStore, FakeClock, FakeExaminer, FakeScm, ScriptedPrompter, diff_touching};

pub type App = Soqrates<AnyStore, FakeExaminer, FakeClock>;

/// One run of a local hook, as seen from the terminal.
#[derive(Debug, Clone)]
pub struct HookRun {
    pub kind: HookKind,
    pub decision: HookDecision,
    pub asked: Vec<Question>,
    pub reported: Vec<SessionOutcome>,
}

/// State of the scenarios that exercise the aggregates directly (ADR-0002),
/// without the engine.
#[derive(Debug, Default)]
pub struct DomainState {
    /// The learner profile under test.
    pub profile: Option<LearnerProfile>,
    /// Events returned by the most recent `LearnerProfile::record`.
    pub last_events: Vec<LearnerEvent>,
    /// Events returned by every `LearnerProfile::record`, in order.
    pub all_events: Vec<LearnerEvent>,
    /// Counter for generated concept IDs.
    pub generated: usize,
    /// Questions to open a session with.
    pub questions: Vec<Question>,
    /// The result of `Session::open`.
    pub opened: Option<soqrates_core::Result<Session>>,
    /// The result of `Session::validate_answers`.
    pub validated: Option<soqrates_core::Result<()>>,
    /// The result of `Session::conclude`.
    pub concluded: Option<SessionResult>,
    /// The error of the most recent failed `Soqrates::submit`.
    pub submit_error: Option<soqrates_core::Error>,
}

/// State of `learner_identity.feature` (ADR-0003). Usernames and verified
/// e-mails of forge accounts live only here: they are not part of the domain
/// on purpose (ADR-0003 D1).
#[derive(Debug, Default)]
pub struct IdentityState {
    /// Learner names used in the scenario -> the learner they resolved to.
    pub learners: std::collections::HashMap<String, soqrates_core::LearnerId>,
    /// Learner name -> the identity the scenario first saw it through.
    pub identity_of: std::collections::HashMap<String, AuthorId>,
    /// Every learner the scenario has resolved so far.
    pub seen: Vec<soqrates_core::LearnerId>,
    /// Identity -> current username (world only).
    pub usernames: std::collections::HashMap<String, String>,
    /// Identity -> e-mail the forge has verified (world only).
    pub verified_emails: std::collections::HashMap<String, String>,
    /// The identity whose learner was found to be new.
    pub that_identity: Option<AuthorId>,
    /// The identities of the two learners to link, in the order given.
    pub pair: Vec<AuthorId>,
    /// Reviews given in the scenario: (concept, format, grade, at, via).
    pub reviews: Vec<(
        soqrates_core::ConceptId,
        soqrates_core::QuestionFormat,
        soqrates_core::Grade,
        chrono::DateTime<chrono::Utc>,
        soqrates_core::Identity,
    )>,
    pub link_code: Option<soqrates_core::LinkCode>,
    /// Identities of the code requester's learner just before the latest
    /// presentation.
    pub before_presenting: Option<Vec<soqrates_core::Identity>>,
    pub link_result: Option<soqrates_core::Result<soqrates_core::LearnerId>>,
    pub declaration: Option<soqrates_core::Result<soqrates_core::LearnerId>>,
    /// Snapshot of a learner's profile and links in the learner store.
    pub snapshot: Option<(Option<LearnerProfile>, Vec<soqrates_core::IdentityLink>)>,
    /// Local repositories: (remote host, commit e-mail, configured identity).
    pub repos: Vec<(String, String, Option<String>)>,
    /// The identity and learner each local hook run resolved to.
    pub hook_learners: Vec<(soqrates_core::Identity, soqrates_core::LearnerId)>,
    /// Identities listed by the learner.
    pub listed: Option<Vec<soqrates_core::Identity>>,
    /// Every session key used in the scenario.
    pub session_keys: Vec<SessionKey>,
    /// Every question asked in the scenario.
    pub questions: Vec<Question>,
    /// The check output being shown.
    pub shown: Option<String>,
    pub counter: usize,
}

#[derive(World)]
#[world(init = Self::new)]
pub struct SpecWorld {
    pub app: App,
    pub clock: FakeClock,
    pub examiner: FakeExaminer,
    pub scm: FakeScm,
    pub prompter: ScriptedPrompter,
    /// Directory of the file store, if the scenario uses one.
    pub state_dir: Option<PathBuf>,
    /// Directory of the SQLite database, if the scenario uses one.
    pub database_dir: Option<PathBuf>,
    /// The session most recently started through the engine.
    pub session: Option<Session>,
    pub outcome: Option<SessionOutcome>,
    pub looked_up: Option<LearnerProfile>,
    /// The pull request the CI steps act on.
    pub pr: Option<PrRef>,
    pub status: Option<CheckStatus>,
    pub hook_runs: Vec<HookRun>,
    pub tools: Vec<String>,
    pub recompute: Option<(Level, Vec<soqrates_core::GradedAnswer>)>,
    pub recomputed: Option<Level>,
    pub domain: DomainState,
    pub identity: IdentityState,
    /// The result of the most recent MCP tool call (ADR-0001 D10).
    pub mcp_result: Option<soqrates_core::Result<serde_json::Value>>,
    /// The question about the concept of each round, in order (ADR-0009).
    pub bank_rounds: Vec<Question>,
    sessions_started: usize,
}

impl fmt::Debug for SpecWorld {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpecWorld")
            .field("store", &self.app.store)
            .field("config", &self.app.config)
            .field("clock", &self.clock)
            .field("scm", &self.scm)
            .field("state_dir", &self.state_dir)
            .field("session", &self.session)
            .field("outcome", &self.outcome)
            .field("pr", &self.pr)
            .field("status", &self.status)
            .field("hook_runs", &self.hook_runs)
            .field("domain", &self.domain)
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

impl SpecWorld {
    fn new() -> Self {
        let clock = FakeClock::default();
        let examiner = FakeExaminer::default();
        let app = Soqrates::new(
            AnyStore::Memory(soqrates_store::InMemoryStore::new()),
            examiner.clone(),
            clock.clone(),
            Config::default(),
        );
        Self {
            app,
            clock,
            examiner,
            scm: FakeScm::default(),
            prompter: ScriptedPrompter::default(),
            state_dir: None,
            database_dir: None,
            session: None,
            outcome: None,
            looked_up: None,
            pr: None,
            status: None,
            hook_runs: Vec::new(),
            tools: Vec::new(),
            recompute: None,
            recomputed: None,
            domain: DomainState::default(),
            identity: IdentityState::default(),
            mcp_result: None,
            bank_rounds: Vec::new(),
            sessions_started: 0,
        }
    }

    /// Switch to a file store in a fresh, unique temporary directory.
    pub fn use_file_store(&mut self) {
        let dir = fresh_dir();
        self.app.store = AnyStore::file(&dir);
        self.state_dir = Some(dir);
    }

    /// Switch to a SQLite database in a fresh, unique temporary directory
    /// (ADR-0005).
    pub async fn use_sqlite_store(&mut self) {
        let dir = fresh_dir();
        self.app.store = AnyStore::sqlite(&dir).await;
        self.database_dir = Some(dir);
    }

    /// Simulate a new process: a fresh engine on the same state directory
    /// or database.
    pub async fn restart(&mut self) {
        let store = match (&self.state_dir, &self.database_dir) {
            (Some(dir), _) => AnyStore::file(dir),
            (None, Some(dir)) => AnyStore::sqlite(dir).await,
            (None, None) => {
                panic!("restart needs a file store or a database; use a store step first")
            }
        };
        let config = self.app.config.clone();
        self.app = Soqrates::new(store, self.examiner.clone(), self.clock.clone(), config);
    }

    pub fn next_local_key(&mut self, author: &AuthorId) -> SessionKey {
        self.sessions_started += 1;
        SessionKey(format!("local:{}:{}", author.0, self.sessions_started))
    }

    pub async fn start(&mut self, author: &AuthorId, diff: &Diff) -> Session {
        let key = self.next_local_key(author);
        let session = self
            .app
            .start_session(key, author, diff, &NoCodebase)
            .await
            .expect("start_session");
        self.session = Some(session.clone());
        session
    }

    pub async fn start_touching(&mut self, author: &AuthorId, concepts: &[String]) -> Session {
        self.start(author, &diff_touching(concepts)).await
    }

    pub async fn submit_all(&mut self, session: &Session, correct: bool) -> SessionOutcome {
        let answers: Vec<_> = session
            .questions()
            .iter()
            .map(|q| FakeExaminer::answer(q, correct))
            .collect();
        let outcome = self
            .app
            .submit(session.key(), &answers, &NoCodebase)
            .await
            .expect("submit");
        self.outcome = Some(outcome.clone());
        outcome
    }

    pub async fn profile(&self, author: &AuthorId) -> LearnerProfile {
        self.app.profile(author).await.expect("profile")
    }

    pub async fn seed_level(&mut self, author: &AuthorId, level: Level) {
        // ADR-0002 D4: a declared starting level is a domain operation.
        // ADR-0003 D1: profiles belong to the learner behind the identity.
        let learner = self.app.learner_of(author).await.expect("learner_of");
        let profile = LearnerProfile::starting_at(learner, level);
        self.app
            .store
            .save_profile(&profile)
            .await
            .expect("save_profile");
    }

    /// Call the MCP tool `name` in-process against this world's engine
    /// (ADR-0001 D10). The engine is lent to a `SoqratesMcp` for the call
    /// and put back afterwards, so every other step keeps owning it.
    pub async fn call_mcp(
        &mut self,
        name: &str,
        args: serde_json::Value,
    ) -> soqrates_core::Result<serde_json::Value> {
        let placeholder = Soqrates::new(
            AnyStore::Memory(soqrates_store::InMemoryStore::new()),
            self.examiner.clone(),
            self.clock.clone(),
            Config::default(),
        );
        let app = std::sync::Arc::new(std::mem::replace(&mut self.app, placeholder));
        let mcp = soqrates_mcp::SoqratesMcp::new(app.clone());
        let result = mcp.call_tool(name, args).await;
        drop(mcp);
        self.app = std::sync::Arc::try_unwrap(app)
            .ok()
            .expect("the MCP server released the engine");
        result
    }

    pub fn current_pr(&self) -> &PrRef {
        self.pr.as_ref().expect("no pull request in this scenario")
    }
}

impl Drop for SpecWorld {
    fn drop(&mut self) {
        if let Some(dir) = &self.state_dir {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// A fresh, unique temporary directory.
fn fresh_dir() -> PathBuf {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let dir = std::env::temp_dir().join(format!(
        "soqrates-spec-{}-{}-{nanos}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).expect("create state directory");
    dir
}
