//! Domain core of Soqrates.
//!
//! ADR-0001 D2 makes this crate the hexagon. ADR-0002 structures it into
//! contexts, with this dependency direction (D2):
//!
//! - [`ids`]: shared kernel, uses nothing
//! - [`assessment`]: concepts, questions, answers, grades
//! - [`learner`]: learner profiles, levels, FSRS scheduling
//! - [`delivery`]: sessions, checks, hooks
//! - [`app`]: engine, CI and hook workflows, ports, events; uses everything
//!
//! The domain contexts are synchronous and pure. All I/O goes through the
//! ports in [`app::ports`].

pub mod app;
pub mod assessment;
pub mod delivery;
pub mod ids;
pub mod learner;

// Short paths for the most used items.
pub use app::events::DomainEvent;
pub use app::{Config, ProfileLocation, SessionOutcome, Soqrates, ci, engine, hook, ports};
pub use assessment::*;
pub use delivery::*;
pub use ids::*;
pub use learner::{
    GradedAnswer, IdentityLink, LINK_CODE_TTL, LearnerEvent, LearnerProfile, Level, LinkCode,
    LinkRejection, ReviewCard, identity, policy, schedule,
};

/// Errors surfaced by the core and by port implementations.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid input: {0}")]
    Invalid(String),
    #[error("port failure: {0}")]
    Port(String),
    /// A link between identities was refused (ADR-0003 D2).
    #[error("link rejected: {0}")]
    Link(#[from] learner::LinkRejection),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
