//! `ScmProvider` for GitHub (ADR-0001 D4). This is the only crate that may
//! depend on `octocrab`.
//!
//! Questions are posted as one PR comment carrying a hidden session marker;
//! the author answers with a `/soqrates answer` comment (ADR-0001 D8). The
//! check is reported as a commit status with context `soqrates`, which works
//! with the Actions `GITHUB_TOKEN` given `statuses: write`. Authors are
//! identified by host and numeric user id, never by login (ADR-0003 D1).

mod format;

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use octocrab::Octocrab;
use soqrates_core::ports::ScmProvider;
use soqrates_core::{
    Answer, AuthorId, CheckStatus, Diff, Error, PrRef, PullRequest, Result, Session,
};

use crate::format::{
    STATUS_CONTEXT, StatusState, author_id, commit_status, host_of, parse_answers,
    render_questions, select_answer_comment, session_marker, split_repo,
};

const DEFAULT_SERVER_URL: &str = "https://github.com";
const DEFAULT_API_URL: &str = "https://api.github.com";

pub struct GitHubProvider {
    token: String,
    api_url: String,
    /// The identity host: the host of the server URL (ADR-0003 D1).
    host: String,
    /// Built on first use: octocrab spawns a worker task, which needs a
    /// running Tokio runtime.
    client: OnceLock<Octocrab>,
    /// Head commits seen by `pull_request`, for `set_check`.
    head_shas: Mutex<HashMap<PrRef, String>>,
}

impl std::fmt::Debug for GitHubProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitHubProvider")
            .field("api_url", &self.api_url)
            .field("host", &self.host)
            .finish_non_exhaustive()
    }
}

fn port<E: std::fmt::Display>(what: &str) -> impl FnOnce(E) -> Error + '_ {
    move |e| Error::Port(format!("GitHub: {what}: {e}"))
}

impl GitHubProvider {
    /// A provider for the GitHub instance at `server_url` (e.g.
    /// `https://github.com`), whose REST API is at `api_url` (e.g.
    /// `https://api.github.com`, or `https://ghe.example.com/api/v3`).
    pub fn new(token: String, server_url: &str, api_url: &str) -> Result<Self> {
        if token.trim().is_empty() {
            return Err(Error::Port("GitHub: empty token".into()));
        }
        let host = host_of(server_url)
            .ok_or_else(|| Error::Port(format!("GitHub: no host in server URL {server_url:?}")))?;
        let api_url = api_url.trim_end_matches('/').to_string();
        if host_of(&api_url).is_none() || !api_url.contains("://") {
            return Err(Error::Port(format!("GitHub: invalid API URL {api_url:?}")));
        }
        Ok(Self {
            token,
            api_url,
            host,
            client: OnceLock::new(),
            head_shas: Mutex::new(HashMap::new()),
        })
    }

    /// Reads `GITHUB_TOKEN` (required), `GITHUB_SERVER_URL` (default
    /// `https://github.com`) and `GITHUB_API_URL` (default
    /// `https://api.github.com`), as set in GitHub Actions.
    pub fn from_env() -> Result<Self> {
        let var = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
        let token = var("GITHUB_TOKEN")
            .ok_or_else(|| Error::Port("GitHub: GITHUB_TOKEN is not set".into()))?;
        let server = var("GITHUB_SERVER_URL").unwrap_or_else(|| DEFAULT_SERVER_URL.into());
        let api = var("GITHUB_API_URL").unwrap_or_else(|| DEFAULT_API_URL.into());
        Self::new(token, &server, &api)
    }

    fn client(&self) -> Result<Octocrab> {
        if let Some(client) = self.client.get() {
            return Ok(client.clone());
        }
        let client = Octocrab::builder()
            .personal_token(self.token.clone())
            .base_uri(self.api_url.as_str())
            .map_err(port("API URL"))?
            .build()
            .map_err(port("client"))?;
        // A concurrent caller may have won the race; either client works.
        Ok(self.client.get_or_init(|| client).clone())
    }

    fn author(&self, user_id: u64) -> String {
        author_id(&self.host, user_id)
    }

    async fn head_sha(&self, pr: &PrRef) -> Result<String> {
        if let Some(sha) = self.head_shas.lock().ok().and_then(|m| m.get(pr).cloned()) {
            return Ok(sha);
        }
        let (owner, name) = repo(pr)?;
        let pull = self
            .client()?
            .pulls(owner, name)
            .get(pr.number)
            .await
            .map_err(port("get pull request"))?;
        Ok(pull.head.sha)
    }
}

fn repo(pr: &PrRef) -> Result<(&str, &str)> {
    split_repo(&pr.repo).ok_or_else(|| {
        Error::Port(format!(
            "GitHub: repo must be owner/name, got {:?}",
            pr.repo
        ))
    })
}

impl ScmProvider for GitHubProvider {
    async fn pull_request(&self, pr: &PrRef) -> Result<PullRequest> {
        let (owner, name) = repo(pr)?;
        let client = self.client()?;
        let pulls = client.pulls(owner, name);
        let pull = pulls
            .get(pr.number)
            .await
            .map_err(port("get pull request"))?;
        let user = pull
            .user
            .ok_or_else(|| Error::Port("GitHub: pull request has no author".into()))?;
        let diff = pulls
            .get_diff(pr.number)
            .await
            .map_err(port("get pull request diff"))?;
        let head_sha = pull.head.sha;
        if let Ok(mut shas) = self.head_shas.lock() {
            shas.insert(pr.clone(), head_sha.clone());
        }
        Ok(PullRequest {
            pr: pr.clone(),
            author: AuthorId(self.author(user.id.into_inner())),
            head_sha,
            diff: Diff(diff),
        })
    }

    async fn post_questions(&self, pr: &PrRef, session: &Session) -> Result<()> {
        let (owner, name) = repo(pr)?;
        self.client()?
            .issues(owner, name)
            .create_comment(pr.number, render_questions(session))
            .await
            .map_err(port("post questions comment"))?;
        Ok(())
    }

    async fn fetch_answers(&self, pr: &PrRef, session: &Session) -> Result<Option<Vec<Answer>>> {
        let (owner, name) = repo(pr)?;
        let client = self.client()?;
        let first = client
            .issues(owner, name)
            .list_comments(pr.number)
            .per_page(100)
            .send()
            .await
            .map_err(port("list comments"))?;
        let all = client
            .all_pages(first)
            .await
            .map_err(port("list comments"))?;
        let comments: Vec<_> = all
            .into_iter()
            .map(|c| {
                (
                    self.author(c.user.id.into_inner()),
                    c.created_at,
                    c.body.unwrap_or_default(),
                )
            })
            .collect();
        let marker = session_marker(session.key());
        Ok(
            select_answer_comment(&comments, &marker, &session.author().0)
                .and_then(|body| parse_answers(body, session.questions())),
        )
    }

    async fn set_check(&self, pr: &PrRef, status: CheckStatus, summary: &str) -> Result<()> {
        let (owner, name) = repo(pr)?;
        let sha = self.head_sha(pr).await?;
        let (state, description) = commit_status(status, summary);
        let state = match state {
            StatusState::Pending => octocrab::models::StatusState::Pending,
            StatusState::Success => octocrab::models::StatusState::Success,
            StatusState::Failure => octocrab::models::StatusState::Failure,
        };
        self.client()?
            .repos(owner, name)
            .create_status(sha, state)
            .context(STATUS_CONTEXT.to_string())
            .description(description)
            .send()
            .await
            .map_err(port("set commit status"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_validates_and_derives_the_identity_host() {
        let p = GitHubProvider::new("t".into(), "https://github.com", "https://api.github.com/")
            .unwrap();
        assert_eq!(p.host, "github.com");
        assert_eq!(p.api_url, "https://api.github.com");
        assert_eq!(p.author(12345), "github.com:12345");
        let p = GitHubProvider::new(
            "t".into(),
            "https://ghe.example.com",
            "https://ghe.example.com/api/v3",
        )
        .unwrap();
        assert_eq!(p.author(7), "ghe.example.com:7");
        assert!(matches!(
            GitHubProvider::new(" ".into(), "https://github.com", "https://api.github.com"),
            Err(Error::Port(_))
        ));
        assert!(matches!(
            GitHubProvider::new("t".into(), "", "https://api.github.com"),
            Err(Error::Port(_))
        ));
        assert!(matches!(
            GitHubProvider::new("t".into(), "https://github.com", "api"),
            Err(Error::Port(_))
        ));
    }

    #[test]
    fn debug_hides_the_token() {
        let p = GitHubProvider::new(
            "super-secret".into(),
            "https://github.com",
            "https://api.github.com",
        )
        .unwrap();
        assert!(!format!("{p:?}").contains("super-secret"));
    }

    #[test]
    fn bad_repo_is_a_port_error() {
        let pr = PrRef {
            repo: "nope".into(),
            number: 1,
        };
        assert!(matches!(repo(&pr), Err(Error::Port(_))));
    }
}
