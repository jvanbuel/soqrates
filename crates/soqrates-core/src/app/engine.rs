//! The Soqrates engine: the facade over the services that orchestrate the
//! aggregates (load, call, save, emit events). Holds no domain rules
//! (ADR-0002 D3).
//!
//! [`Soqrates`] holds the stores, the examiner, the clock and the
//! configuration, and nothing else. What it does is split by use case into
//! `impl` blocks in the sibling modules `identities`, `profiles`, `compose`
//! and `grading`, which reach the stores only through the router in
//! `stores`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::app::ports::{Clock, Examiner, StateStore};
use crate::app::stores::Stores;
use crate::delivery::Mode;

pub use crate::app::grading::{SessionOutcome, SessionState};
pub use crate::app::stores::ProfileLocation;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    /// Upper bound on questions per session, due reviews included. A cap,
    /// not a target (ADR-0002 D10).
    pub max_questions: usize,
    /// Fraction of correct answers needed to pass, in `0.0..=1.0`.
    pub pass_threshold: f64,
    pub mode: Mode,
    /// Whether experts get open questions (ADR-0002 D5). Off: everyone
    /// gets multiple choice.
    #[serde(default)]
    pub open_questions: bool,
    /// Follow-up rounds per session at most: new questions about the
    /// concepts answered wrongly (ADR-0010 D2). 0 turns them off.
    #[serde(default = "default_max_follow_ups")]
    pub max_follow_ups: u32,
    /// Forge host -> the organisation that administers it (ADR-0003 D2, D5).
    pub forges: BTreeMap<String, String>,
    /// Whether rounds reuse questions from the question bank (ADR-0009).
    #[serde(default = "default_true")]
    pub question_bank: bool,
    /// A learner is shown a bank question at most once within this many
    /// rounds (ADR-0009 D5).
    #[serde(default = "default_reask_after_rounds")]
    pub reask_after_rounds: u64,
}

fn default_true() -> bool {
    true
}

fn default_reask_after_rounds() -> u64 {
    10
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_questions: 5,
            pass_threshold: 0.6,
            mode: Mode::Advisory,
            open_questions: false,
            max_follow_ups: default_max_follow_ups(),
            forges: BTreeMap::new(),
            question_bank: true,
            reask_after_rounds: default_reask_after_rounds(),
        }
    }
}

fn default_max_follow_ups() -> u32 {
    2
}

pub struct Soqrates<S, E, C> {
    /// The learner store (ADR-0003 D4): profiles, links, link codes, sessions.
    pub store: S,
    /// Per-organisation stores for reviews of project concepts (ADR-0003 D5),
    /// and for profiles local to a forge when the learner store is
    /// unreachable (D4). Keyed by organisation.
    pub org_stores: BTreeMap<String, S>,
    /// A question bank shared across organisations, if the deployment has
    /// one; otherwise each organisation's store holds its own (ADR-0009 D2).
    pub shared_bank: Option<S>,
    pub examiner: E,
    pub clock: C,
    pub config: Config,
}

impl<S: StateStore, E: Examiner, C: Clock> Soqrates<S, E, C> {
    pub fn new(store: S, examiner: E, clock: C, config: Config) -> Self {
        Self {
            store,
            org_stores: BTreeMap::new(),
            shared_bank: None,
            examiner,
            clock,
            config,
        }
    }

    /// Add the store of `organisation` (ADR-0003 D5).
    pub fn with_org_store(mut self, organisation: impl Into<String>, store: S) -> Self {
        self.org_stores.insert(organisation.into(), store);
        self
    }

    /// Keep the question bank in `store`, shared across organisations,
    /// instead of in each organisation's store (ADR-0009 D2).
    pub fn with_shared_bank(mut self, store: S) -> Self {
        self.shared_bank = Some(store);
        self
    }

    /// The router over this engine's stores.
    pub(super) fn stores(&self) -> Stores<'_, S, C> {
        Stores::new(
            &self.store,
            &self.org_stores,
            self.shared_bank.as_ref(),
            &self.config.forges,
            &self.clock,
        )
    }
}
