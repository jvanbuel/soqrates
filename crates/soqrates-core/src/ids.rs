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

/// Stable identity of a question in a question bank (ADR-0009 D6). Opaque,
/// derived from the question when it was first banked, and kept when the
/// question is revised.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct BankId(pub String);

/// Identity of a session. In CI it is derived from the PR and its head
/// commit (ADR-0001 D8); locally from the hook invocation.
///
/// The key is kept as its string form, which is what stores, file names and
/// comment markers hold, and what MCP clients may choose freely. Its forms
/// are built by the typed constructors ([`SessionKey::pr`],
/// [`SessionKey::hook`], [`SessionKey::mcp`]) and read back through
/// [`SessionKey::parts`] and [`SessionKey::pr_head`], so nothing outside
/// this module formats or parses the string. A string that is no known form
/// is [`KeyParts::Other`] and is stored and serialised unchanged.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionKey(pub String);

/// A [`SessionKey`] taken apart. Borrowed from the key; displaying the
/// parts again is not needed, since the key keeps its string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyParts<'a> {
    /// `{repo}#{number}@{head}`: a PR at a head commit.
    Pr {
        repo: &'a str,
        number: u64,
        head: &'a str,
    },
    /// `hook:{kind}:{author}:{time}`; `rest` is `{author}:{time}`.
    Hook { kind: &'a str, rest: &'a str },
    /// `mcp:{author}:{time}:{n}`; `rest` is what follows `mcp:`.
    Mcp { rest: &'a str },
    /// Any other key, e.g. one an MCP client chose.
    Other(&'a str),
}

impl SessionKey {
    /// The key of a pull request at a head commit. A new push gives a new key.
    pub fn pr(repo: &str, number: u64, head: &str) -> Self {
        SessionKey(format!("{repo}#{number}@{head}"))
    }

    /// The key of a git hook invocation.
    pub fn hook(kind: impl std::fmt::Debug, author: &AuthorId, time: &str) -> Self {
        SessionKey(format!("hook:{kind:?}:{}:{time}", author.0))
    }

    /// The key of a session an MCP client started without choosing a key.
    pub fn mcp(author: &AuthorId, time: &str, n: u64) -> Self {
        SessionKey(format!("mcp:{}:{time}:{n}", author.0))
    }

    /// The string form, as stored.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// This key taken apart.
    pub fn parts(&self) -> KeyParts<'_> {
        let s = self.0.as_str();
        if let Some(rest) = s.strip_prefix("hook:")
            && let Some((kind, rest)) = rest.split_once(':')
        {
            return KeyParts::Hook { kind, rest };
        }
        if let Some(rest) = s.strip_prefix("mcp:") {
            return KeyParts::Mcp { rest };
        }
        if let Some((front, head)) = s.rsplit_once('@')
            && let Some((repo, number)) = front.rsplit_once('#')
            && let Ok(number) = number.parse::<u64>()
            && !repo.is_empty()
            && number.to_string() == front[repo.len() + 1..]
        {
            return KeyParts::Pr { repo, number, head };
        }
        KeyParts::Other(s)
    }

    /// The head commit of this key if it is the key of PR `repo#number`
    /// (whatever its head), `None` for any other key. An empty head counts
    /// as none.
    pub fn pr_head(&self, repo: &str, number: u64) -> Option<&str> {
        let prefix = format!("{repo}#{number}@");
        self.0.strip_prefix(&prefix).filter(|head| !head.is_empty())
    }

    /// Whether this is a key of PR `repo#number`, whatever its head.
    pub fn is_of_pr(&self, repo: &str, number: u64) -> bool {
        self.0.starts_with(&format!("{repo}#{number}@"))
    }
}

impl std::fmt::Display for SessionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for SessionKey {
    fn from(s: String) -> Self {
        SessionKey(s)
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

    fn author(s: &str) -> AuthorId {
        AuthorId(s.into())
    }

    #[derive(Debug)]
    enum Kind {
        PrePush,
    }

    #[test]
    fn keys_keep_their_string_form_and_serialise_as_a_plain_string() {
        let t = "2026-01-01T00:00:00+00:00";
        let keys = [
            (SessionKey::pr("o/r", 7, "abc"), "o/r#7@abc"),
            (SessionKey::pr("o/r", 7, ""), "o/r#7@"),
            (SessionKey::pr("a#b@c:d/r", 1, "h"), "a#b@c:d/r#1@h"),
            (
                SessionKey::hook(Kind::PrePush, &author("github.com:1"), t),
                "hook:PrePush:github.com:1:2026-01-01T00:00:00+00:00",
            ),
            (
                SessionKey::mcp(&author("email:a@b"), t, 3),
                "mcp:email:a@b:2026-01-01T00:00:00+00:00:3",
            ),
            (SessionKey("anything goes".into()), "anything goes"),
        ];
        for (key, text) in keys {
            assert_eq!(key.to_string(), text);
            let json = serde_json::to_string(&key).unwrap();
            assert_eq!(json, serde_json::to_string(text).unwrap());
            let back: SessionKey = serde_json::from_str(&json).unwrap();
            assert_eq!(back, key);
        }
    }

    #[test]
    fn keys_are_told_apart_by_form() {
        assert_eq!(
            SessionKey::pr("o/r", 7, "abc").parts(),
            KeyParts::Pr {
                repo: "o/r",
                number: 7,
                head: "abc"
            }
        );
        assert_eq!(
            SessionKey::pr("a#b@c:d/r", 1, "").parts(),
            KeyParts::Pr {
                repo: "a#b@c:d/r",
                number: 1,
                head: ""
            }
        );
        assert!(matches!(
            SessionKey("hook:Commit:x:y".into()).parts(),
            KeyParts::Hook { kind: "Commit", .. }
        ));
        assert!(matches!(
            SessionKey("mcp:x:y:1".into()).parts(),
            KeyParts::Mcp { .. }
        ));
        for other in ["", "k", "local:a:1", "o/r#x@abc", "#1@a", "o/r#01@a"] {
            assert_eq!(SessionKey(other.into()).parts(), KeyParts::Other(other));
        }
    }

    #[test]
    fn pr_head_reads_only_the_keys_of_that_pr() {
        let key = SessionKey::pr("o/r", 7, "abc");
        assert_eq!(key.pr_head("o/r", 7), Some("abc"));
        assert_eq!(key.pr_head("o/r", 70), None);
        assert_eq!(key.pr_head("o/r2", 7), None);
        assert!(key.is_of_pr("o/r", 7));
        assert!(!key.is_of_pr("o/r", 8));
        let empty = SessionKey::pr("o/r", 7, "");
        assert_eq!(empty.pr_head("o/r", 7), None);
        assert!(empty.is_of_pr("o/r", 7));
        assert_eq!(SessionKey("hook:x".into()).pr_head("o/r", 7), None);
    }

    #[test]
    fn parsing_and_displaying_any_string_is_the_identity() {
        let samples = [
            "",
            "@",
            "#",
            "#@",
            "1@",
            "o/r#1@",
            "a@b#2@c@d",
            "hook:",
            "hook:a",
            "mcp:",
            "::",
            "o/r#1@abc",
            "ü#9@é",
        ];
        for s in samples {
            let key: SessionKey = serde_json::from_value(serde_json::json!(s)).unwrap();
            assert_eq!(key.to_string(), s);
            assert_eq!(serde_json::to_value(&key).unwrap(), serde_json::json!(s));
            // The parts never lose a character.
            match key.parts() {
                KeyParts::Pr { repo, number, head } => {
                    assert_eq!(format!("{repo}#{number}@{head}"), s)
                }
                KeyParts::Hook { kind, rest } => assert_eq!(format!("hook:{kind}:{rest}"), s),
                KeyParts::Mcp { rest } => assert_eq!(format!("mcp:{rest}"), s),
                KeyParts::Other(o) => assert_eq!(o, s),
            }
        }
    }

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

    #[test]
    fn malformed_identities_are_rejected() {
        for text in ["", "github.com", "github.com:", ":1001", "email:"] {
            assert!(text.parse::<Identity>().is_err(), "{text:?}");
        }
    }
}
