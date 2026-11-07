//! Domain core of Soqrates.
//!
//! This crate is the hexagon of ADR-0001 D2. It holds the domain model and
//! workflows and defines the ports ([`ports`]) that adapters implement. It must
//! not do I/O and must not depend on any adapter crate.

pub mod ci;
pub mod engine;
pub mod hook;
pub mod model;
pub mod policy;
pub mod ports;
pub mod schedule;

pub use engine::{Config, Mode, Soqrates};
pub use model::*;

/// Errors surfaced by the core and by port implementations.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid input: {0}")]
    Invalid(String),
    #[error("port failure: {0}")]
    Port(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
