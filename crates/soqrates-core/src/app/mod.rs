//! Application layer: the engine, the CI and hook workflows, the ports and
//! the domain events. May use every context (ADR-0002 D2).
//!
//! The engine is [`Soqrates`], a facade over services, one module and one
//! `impl` each, which reach the stores only through the router in `stores`:
//!
//! - `identities`: who is behind an author, links and deletion (ADR-0003)
//! - `profiles`: a learner's profile and what is due on it
//! - `compose`: choosing a session's questions and opening it
//! - `grading`: grading a session through its journal, and where it is
//! - `stores`: the learner and organisation stores, the fallback when the
//!   learner store is unreachable, and recovery of interrupted gradings

pub mod ci;
mod compose;
pub mod engine;
pub mod events;
mod grading;
pub mod hook;
mod identities;
pub mod ports;
mod profiles;
mod state;
mod stores;

pub use engine::{Config, Soqrates};
pub use grading::{SessionOutcome, SessionState};
pub use stores::ProfileLocation;
