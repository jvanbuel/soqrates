//! Links between identities and learners (ADR-0003 D1-D3, D6).

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::ids::{Identity, LearnerId};

/// How long a link code stays valid (ADR-0003 D2).
pub const LINK_CODE_TTL: Duration = Duration::minutes(15);

/// The fact that an identity belongs to a learner. Every identity has at
/// most one link (ADR-0003 D2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityLink {
    pub identity: Identity,
    pub learner: LearnerId,
    pub since: DateTime<Utc>,
}

/// Why a link was refused (ADR-0003 D2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum LinkRejection {
    #[error("the link code has expired")]
    Expired,
    #[error("the link code has already been used")]
    AlreadyUsed,
    #[error("the link code was presented by the identity that requested it")]
    SameIdentity,
    #[error("unknown link code")]
    UnknownCode,
    #[error("{0} already belongs to another learner")]
    BelongsToAnotherLearner(Identity),
    #[error("{host} is not administered by {organisation}")]
    NotAdministered { host: String, organisation: String },
}

/// A short-lived, single-use code proving that one person controls two
/// identities (ADR-0003 D2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkCode {
    code: String,
    requested_by: Identity,
    learner: LearnerId,
    expires_at: DateTime<Utc>,
    used: bool,
}

impl LinkCode {
    /// Issue `code` for `requested_by`, whose learner is `learner`, valid
    /// for [`LINK_CODE_TTL`] from `now`.
    pub fn issue(
        code: String,
        requested_by: Identity,
        learner: LearnerId,
        now: DateTime<Utc>,
    ) -> Self {
        Self {
            code,
            requested_by,
            learner,
            expires_at: now + LINK_CODE_TTL,
            used: false,
        }
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn requested_by(&self) -> &Identity {
        &self.requested_by
    }

    /// The learner that requested the code.
    pub fn learner(&self) -> &LearnerId {
        &self.learner
    }

    pub fn expires_at(&self) -> DateTime<Utc> {
        self.expires_at
    }

    /// Use the code, presented by `presenter` at `now`. Rejects an expired
    /// or used code, or one presented by the identity that requested it.
    /// A successful redeem uses the code up.
    pub fn redeem(
        &mut self,
        presenter: &Identity,
        now: DateTime<Utc>,
    ) -> Result<(), LinkRejection> {
        if self.used {
            return Err(LinkRejection::AlreadyUsed);
        }
        if now > self.expires_at {
            return Err(LinkRejection::Expired);
        }
        if presenter == &self.requested_by {
            return Err(LinkRejection::SameIdentity);
        }
        self.used = true;
        Ok(())
    }
}
