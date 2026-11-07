//! Application layer: the engine, the CI and hook workflows, the ports and
//! the domain events. May use every context (ADR-0002 D2).

pub mod ci;
pub mod engine;
pub mod events;
pub mod hook;
pub mod ports;

pub use engine::{Config, ProfileLocation, SessionOutcome, Soqrates};
