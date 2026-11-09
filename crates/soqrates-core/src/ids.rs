//! Shared kernel: identities used by every context (ADR-0002 D2).

use serde::{Deserialize, Serialize};

/// Stable identity of an author, e.g. `github.com:12345` or `email:jan@example.com`
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

impl SessionKey {
    /// The key of follow-up round `round` (2 or more) of this key's session
    /// (ADR-0010 D2): the first round's key with `#<round>` appended, e.g.
    /// `jvanbuel/soqrates#7@abc123#2`.
    pub fn follow_up(&self, round: u32) -> SessionKey {
        let (first, _) = self.round();
        SessionKey(format!("{}#{round}", first.0))
    }

    /// The key of this session's first round, and which round this key is:
    /// 1 for a first round, 2 or more for a follow-up (ADR-0010 D2).
    pub fn round(&self) -> (SessionKey, u32) {
        let follow_up = self.0.rsplit_once('#').and_then(|(first, n)| {
            let digits = !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit());
            let round: u32 = n.parse().ok().filter(|_| digits)?;
            (round >= 2 && !first.is_empty()).then(|| (SessionKey(first.to_string()), round))
        });
        follow_up.unwrap_or_else(|| (self.clone(), 1))
    }
}

/// The person behind one or more identities. Opaque and generated; owns
/// exactly one learner profile (ADR-0003 D1).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct LearnerId(pub String);

/// Whether the forge vouches for an identity (ADR-0003 D2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Verification {
    Verified,
    Unverified,
}

/// One way a learner shows up (ADR-0003 D1). Usernames are never identities:
/// they can be renamed and reused.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum Identity {
    /// An account on one forge: the forge's host and its immutable account
    /// ID, written `github.com:1001`.
    Forge { host: String, account: String },
    /// An e-mail address, written `email:<address>`.
    Email(String),
}

impl Identity {
    /// Parse `host:account` or `email:<address>`.
    pub fn parse(text: &str) -> crate::Result<Self> {
        match text.split_once(':') {
            Some(("email", address)) if !address.is_empty() => Ok(Identity::Email(address.into())),
            Some((host, account)) if !host.is_empty() && !account.is_empty() => {
                Ok(Identity::Forge {
                    host: host.into(),
                    account: account.into(),
                })
            }
            _ => Err(crate::Error::Invalid(format!("not an identity: {text:?}"))),
        }
    }

    /// The forge host, for forge identities.
    pub fn host(&self) -> Option<&str> {
        match self {
            Identity::Forge { host, .. } => Some(host),
            Identity::Email(_) => None,
        }
    }

    /// Forge accounts are verified; an e-mail on its own is not (ADR-0003 D2).
    pub fn verification(&self) -> Verification {
        match self {
            Identity::Forge { .. } => Verification::Verified,
            Identity::Email(_) => Verification::Unverified,
        }
    }
}

/// `"github.com:1001".parse::<Identity>()`, the idiomatic counterpart of
/// [`Identity::parse`].
impl std::str::FromStr for Identity {
    type Err = crate::Error;

    fn from_str(text: &str) -> crate::Result<Self> {
        Identity::parse(text)
    }
}

impl std::fmt::Display for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Identity::Forge { host, account } => write!(f, "{host}:{account}"),
            Identity::Email(address) => write!(f, "email:{address}"),
        }
    }
}

impl TryFrom<String> for Identity {
    type Error = crate::Error;
    fn try_from(text: String) -> crate::Result<Self> {
        Identity::parse(&text)
    }
}

impl From<Identity> for String {
    fn from(identity: Identity) -> String {
        identity.to_string()
    }
}

impl AuthorId {
    /// The identity this author string denotes (ADR-0003).
    pub fn identity(&self) -> crate::Result<Identity> {
        Identity::parse(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identities_round_trip_through_from_str_and_display() {
        for text in [
            "github.com:1001",
            "gitlab.client.example:678",
            "email:a+b@example.com",
        ] {
            let identity: Identity = text.parse().unwrap();
            assert_eq!(identity.to_string(), text);
        }
    }

    // @adr-0010-d2
    #[test]
    fn follow_up_keys_append_the_round_to_the_first_round() {
        let first = SessionKey("jvanbuel/soqrates#7@abc123".into());
        assert_eq!(first.round(), (first.clone(), 1));
        let second = first.follow_up(2);
        assert_eq!(second.0, "jvanbuel/soqrates#7@abc123#2");
        assert_eq!(second.round(), (first.clone(), 2));
        assert_eq!(second.follow_up(3).0, "jvanbuel/soqrates#7@abc123#3");
        for key in [
            "hook:PreCommit:github.com:1001:2026-01-05T09:00:00+00:00",
            "x#1",
            "#2",
        ] {
            let key = SessionKey(key.into());
            assert_eq!(key.round(), (key.clone(), 1), "{key:?}");
        }
    }

    #[test]
    fn malformed_identities_are_rejected() {
        for text in ["", "github.com", "github.com:", ":1001", "email:"] {
            assert!(text.parse::<Identity>().is_err(), "{text:?}");
        }
    }
}
