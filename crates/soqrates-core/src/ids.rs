//! Shared kernel: identities used by every context (ADR-0002 D2).

use serde::{Deserialize, Serialize};

/// Stable identity of an author, e.g. `github:12345` or `email:jan@example.com`
/// (ADR-0001 D5).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AuthorId(pub String);

/// Identifies a concept, e.g. `rust.ownership.borrowing`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ConceptId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct QuestionId(pub String);

/// Identity of a session. In CI it is derived from the PR and its head
/// commit (ADR-0001 D8); locally from the hook invocation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SessionKey(pub String);
