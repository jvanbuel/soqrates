//! `ScmProvider` for GitHub (ADR-0001 D4). This is the only crate that may
//! depend on `octocrab`.

use soqrates_core::ports::ScmProvider;
use soqrates_core::{Answer, CheckStatus, PrRef, PullRequest, Result, Session};

pub struct GitHubProvider {
    _private: (),
}

impl GitHubProvider {
    /// Uses `GITHUB_TOKEN` from the environment, as in GitHub Actions.
    pub fn from_env() -> Result<Self> {
        todo!("ADR-0001 D4")
    }
}

impl ScmProvider for GitHubProvider {
    async fn pull_request(&self, pr: &PrRef) -> Result<PullRequest> {
        let _ = pr;
        todo!("ADR-0001 D4")
    }
    async fn post_questions(&self, pr: &PrRef, session: &Session) -> Result<()> {
        let _ = (pr, session);
        todo!("ADR-0001 D4")
    }
    async fn fetch_answers(&self, pr: &PrRef, session: &Session) -> Result<Option<Vec<Answer>>> {
        let _ = (pr, session);
        todo!("ADR-0001 D4")
    }
    async fn set_check(&self, pr: &PrRef, status: CheckStatus) -> Result<()> {
        let _ = (pr, status);
        todo!("ADR-0001 D4")
    }
}
