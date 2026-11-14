//! `ScmProvider` for GitHub (ADR-0001 D4). This is the only crate that may
//! depend on `octocrab`.
//!
//! Questions are posted as one PR comment carrying a hidden session marker.
//! The author answers multiple choice by ticking boxes in it, open questions
//! by typing into their answer fields in it, and submits by ticking "Grade
//! my answers"; or answers with a `/soqrates answer` comment (ADR-0001 D8,
//! ADR-0004 D3, D11). The command is read forgivingly (`/socrates`, any
//! case, or `@<app slug> answer`), and a mistyped one gets a hint (ADR-0004
//! D8). Boxes and answer fields count only when GraphQL reports the PR
//! author as the comment's last editor. The comment that submitted the
//! answers gets a 👀 reaction while they are graded, then a 👍 (ADR-0004
//! D9). Results are posted as a new comment, saying which answer comment,
//! boxes and fields were read, and the question comment is frozen (ADR-0004
//! D5). A newer push freezes
//! the earlier question comments that were never graded (D7). The
//! check is reported as a commit status with context `soqrates`, which works
//! with the Actions `GITHUB_TOKEN` given `statuses: write`. Authors are
//! identified by host and numeric user id, never by login (ADR-0003 D1).
//! The PR author's `/soqrates feedback` comments are about the results
//! comment before them, and each gets one short answer (ADR-0011 D2).

mod format;

use std::sync::OnceLock;

use octocrab::Octocrab;
use octocrab::models::issues::Comment;
use soqrates_core::answer_command::{hints_due, parse_answers};
use soqrates_core::ports::{EarlierSessions, Frozen, ScmProvider, Submission};
use soqrates_core::{
    AuthorId, CheckStatus, Diff, Error, FeedbackCommand, PrRef, PullRequest, Result, Session,
    SessionOutcome,
};

use crate::format::{
    AnswerComment, AnswerSource, EDITOR_QUERY, ListedComment, STATUS_CONTEXT, StatusState,
    author_id, comments_to_freeze, commit_status, diff_of_files, editor_database_id, graphql_base,
    hinted_comment, host_of, is_slug, merge_answers, parse_in_place, render_feedback_note,
    render_graded_questions, render_hint, render_questions, render_results,
    render_superseded_questions, select_answer_comment, select_feedback, session_marker,
    split_repo, superseded_keys,
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
    /// Like `client`, rooted where `/graphql` is served.
    graphql_client: OnceLock<Octocrab>,
    /// The slug of the app Soqrates posts as, if any: `@<slug> answer` is
    /// then an answer command too (ADR-0004 D8, ADR-0006 D4).
    app_slug: Option<String>,
}

/// How a session's answers were submitted (ADR-0004 D8, D9): what
/// `collect_answers` found, for `acknowledge_answers` and `post_results`.
#[derive(Debug, Clone)]
pub struct Receipt {
    /// The comment that submitted them: the answer comment, or the question
    /// comment when "Grade my answers" was ticked.
    comment_id: u64,
    source: AnswerSource,
    /// The 👀 reaction added by `acknowledge_answers`, for `post_results`
    /// to replace.
    eyes: Option<u64>,
}

impl std::fmt::Debug for GitHubProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitHubProvider")
            .field("api_url", &self.api_url)
            .field("host", &self.host)
            .field("app_slug", &self.app_slug)
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
            graphql_client: OnceLock::new(),
            app_slug: None,
        })
    }

    /// Also accept `@<slug> answer` as the answer command, for the app
    /// Soqrates posts as (ADR-0004 D8). Anything that is not an app slug is
    /// ignored, so a bare `@soqrates` is never accepted by accident.
    pub fn with_app_slug(mut self, slug: Option<String>) -> Self {
        self.app_slug = slug.map(|s| s.trim().to_string()).filter(|s| is_slug(s));
        self
    }

    /// Reads `GITHUB_TOKEN` (required), `GITHUB_SERVER_URL` (default
    /// `https://github.com`) and `GITHUB_API_URL` (default
    /// `https://api.github.com`), as set in GitHub Actions, and
    /// `SOQRATES_APP_SLUG` (optional), set by the action when it posts as an
    /// app.
    pub fn from_env() -> Result<Self> {
        let var = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
        let token = var("GITHUB_TOKEN")
            .ok_or_else(|| Error::Port("GitHub: GITHUB_TOKEN is not set".into()))?;
        let server = var("GITHUB_SERVER_URL").unwrap_or_else(|| DEFAULT_SERVER_URL.into());
        let api = var("GITHUB_API_URL").unwrap_or_else(|| DEFAULT_API_URL.into());
        Ok(Self::new(token, &server, &api)?.with_app_slug(var("SOQRATES_APP_SLUG")))
    }

    fn app_slug(&self) -> Option<&str> {
        self.app_slug.as_deref()
    }

    /// Add the reaction `content` to the comment `comment_id`, returning
    /// the reaction's id.
    async fn react(&self, pr: &PrRef, comment_id: u64, content: &str) -> Result<u64> {
        let (owner, name) = repo(pr)?;
        let route = format!("/repos/{owner}/{name}/issues/comments/{comment_id}/reactions");
        let reaction: serde_json::Value = self
            .client()?
            .post(route, Some(&serde_json::json!({ "content": content })))
            .await
            .map_err(port("add reaction"))?;
        reaction
            .get("id")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| Error::Port("GitHub: add reaction: no id in response".into()))
    }

    async fn unreact(&self, pr: &PrRef, comment_id: u64, reaction_id: u64) -> Result<()> {
        let (owner, name) = repo(pr)?;
        let route =
            format!("/repos/{owner}/{name}/issues/comments/{comment_id}/reactions/{reaction_id}");
        let response = self
            .client()?
            ._delete(route.as_str(), None::<&()>)
            .await
            .map_err(port("remove reaction"))?;
        if !response.status().is_success() {
            return Err(Error::Port(format!(
                "GitHub: remove reaction: {}",
                response.status()
            )));
        }
        Ok(())
    }

    fn client(&self) -> Result<Octocrab> {
        self.build_client(&self.client, &self.api_url)
    }

    /// The client for GraphQL. octocrab posts to `/graphql` under its base
    /// URI, which on GitHub Enterprise Server is `/api`, not `/api/v3`.
    fn graphql_client(&self) -> Result<Octocrab> {
        self.build_client(&self.graphql_client, &graphql_base(&self.api_url))
    }

    fn build_client(&self, cell: &OnceLock<Octocrab>, base: &str) -> Result<Octocrab> {
        if let Some(client) = cell.get() {
            return Ok(client.clone());
        }
        let client = Octocrab::builder()
            .personal_token(self.token.clone())
            .base_uri(base)
            .map_err(port("API URL"))?
            .build()
            .map_err(port("client"))?;
        // A concurrent caller may have won the race; either client works.
        Ok(cell.get_or_init(|| client).clone())
    }

    fn author(&self, user_id: u64) -> String {
        author_id(&self.host, user_id)
    }

    /// Every comment on the PR, oldest first.
    async fn comments(&self, pr: &PrRef) -> Result<Vec<Comment>> {
        let (owner, name) = repo(pr)?;
        let client = self.client()?;
        let first = client
            .issues(owner, name)
            .list_comments(pr.number)
            .per_page(100)
            .send()
            .await
            .map_err(port("list comments"))?;
        client.all_pages(first).await.map_err(port("list comments"))
    }

    /// The author id of the last editor of a comment, if it was edited by a
    /// user (ADR-0004 D3). REST does not say who edited a comment; GraphQL
    /// does.
    async fn last_editor(&self, node_id: &str) -> Result<Option<String>> {
        let payload = serde_json::json!({
            "query": EDITOR_QUERY,
            "variables": { "id": node_id },
        });
        let data: serde_json::Value = self
            .graphql_client()?
            .graphql(&payload)
            .await
            .map_err(port("query comment editor"))?;
        Ok(editor_database_id(&data).map(|id| self.author(id)))
    }

    /// Replace the body of the comment `id`.
    async fn edit_comment(&self, pr: &PrRef, id: u64, body: String, what: &str) -> Result<()> {
        let (owner, name) = repo(pr)?;
        let route = format!("/repos/{owner}/{name}/issues/comments/{id}");
        let _: Comment = self
            .client()?
            .patch(route, Some(&serde_json::json!({ "body": body })))
            .await
            .map_err(port(what))?;
        Ok(())
    }
}

impl GitHubProvider {
    /// Post a hint for each of the author's comments on `session` that
    /// starts like an answer command but isn't one and has no hint yet
    /// (ADR-0004 D8).
    async fn post_hints(&self, pr: &PrRef, session: &Session) -> Result<()> {
        let comments = self.comments(pr).await?;
        let Some(start) = comments.iter().position(|c| {
            c.body
                .as_deref()
                .is_some_and(|b| b.contains(&session_marker(session.key())))
        }) else {
            return Ok(());
        };
        // Hints are posted by the account that posts the questions, so only
        // its comments say what was hinted already.
        let poster = comments[start].user.id;
        let hinted: Vec<u64> = comments
            .iter()
            .filter(|c| c.user.id == poster)
            .filter_map(|c| hinted_comment(c.body.as_deref().unwrap_or_default()))
            .collect();
        let author = &session.author().0;
        let candidates: Vec<(u64, &str)> = comments[start + 1..]
            .iter()
            .filter(|c| self.author(c.user.id.into_inner()) == *author)
            .map(|c| (c.id.into_inner(), c.body.as_deref().unwrap_or_default()))
            .collect();
        let (owner, name) = repo(pr)?;
        for id in hints_due(&candidates, &hinted, self.app_slug()) {
            let Some(c) = comments.iter().find(|c| c.id.into_inner() == id) else {
                continue;
            };
            self.client()?
                .issues(owner, name)
                .create_comment(
                    pr.number,
                    render_hint(id, c.html_url.as_str(), self.app_slug()),
                )
                .await
                .map_err(port("post command hint"))?;
        }
        Ok(())
    }
}

/// The question comment of a session: the first comment carrying its marker.
fn question_comment<'a>(comments: &'a [Comment], marker: &str) -> Option<&'a Comment> {
    comments
        .iter()
        .find(|c| c.body.as_deref().is_some_and(|b| b.contains(marker)))
}

/// Where a feedback comment came from: its forge and id, so the same
/// comment counts once (ADR-0011 D2).
fn feedback_source(host: &str, comment_id: u64) -> String {
    format!("{host}:comment:{comment_id}")
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
    type Receipt = Receipt;

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
        Ok(PullRequest {
            pr: pr.clone(),
            author: AuthorId(self.author(user.id.into_inner())),
            head_sha: pull.head.sha,
            diff: Diff(diff),
        })
    }

    async fn diff_between(&self, pr: &PrRef, from: &str, to: &str) -> Result<Option<Diff>> {
        let (owner, name) = repo(pr)?;
        let comparison = match self
            .client()?
            .commits(owner, name)
            .compare(from, to)
            .send()
            .await
        {
            Ok(c) => c,
            // A force-push can leave `from` unreachable, or gone.
            Err(octocrab::Error::GitHub { source, .. })
                if matches!(source.status_code.as_u16(), 404 | 422) =>
            {
                return Ok(None);
            }
            Err(e) => return Err(port("compare commits")(e)),
        };
        use octocrab::models::commits::GithubCommitStatus;
        if !matches!(
            comparison.status,
            GithubCommitStatus::Ahead | GithubCommitStatus::Identical
        ) {
            // Behind or diverged: `from` is not an ancestor of `to`.
            return Ok(None);
        }
        let files: Vec<(String, Option<String>, Option<String>)> = comparison
            .files
            .unwrap_or_default()
            .into_iter()
            .map(|f| (f.filename, f.previous_filename, f.patch))
            .collect();
        Ok(Some(Diff(diff_of_files(&files))))
    }

    async fn publish_questions(&self, pr: &PrRef, session: &Session) -> Result<()> {
        let (owner, name) = repo(pr)?;
        self.client()?
            .issues(owner, name)
            .create_comment(pr.number, render_questions(session))
            .await
            .map_err(port("post questions comment"))?;
        Ok(())
    }

    async fn freeze_superseded(
        &self,
        pr: &PrRef,
        current: &Session,
        sessions: &impl EarlierSessions,
    ) -> Result<Frozen> {
        let comments = self.comments(pr).await?;
        let Some(ours) = question_comment(&comments, &session_marker(current.key())) else {
            return Ok(Frozen::default());
        };
        // Only comments posted by the account that posts Soqrates' questions.
        let poster = ours.user.id.into_inner();
        let listed: Vec<(u64, &str)> = comments
            .iter()
            .map(|c| {
                (
                    c.user.id.into_inner(),
                    c.body.as_deref().unwrap_or_default(),
                )
            })
            .collect();
        let mut done = Frozen::default();
        for key in superseded_keys(&listed, poster, current.key()) {
            if !sessions.may_freeze(&key) {
                continue;
            }
            let frozen = async {
                let session = sessions.pending(&key).await?;
                for i in comments_to_freeze(&listed, poster, &key) {
                    // Text left in the answer fields is kept, shown inert,
                    // so the author can reuse it when the question is asked
                    // again (ADR-0004 D11).
                    let drafts = session
                        .as_ref()
                        .map(|s| parse_in_place(listed[i].1, s.questions()).fields)
                        .unwrap_or_default();
                    let body = render_superseded_questions(
                        &key,
                        session.as_ref(),
                        Some(ours.html_url.as_str()),
                        !current.questions().is_empty(),
                        &drafts,
                    );
                    self.edit_comment(
                        pr,
                        comments[i].id.into_inner(),
                        body,
                        "mark question comment superseded",
                    )
                    .await?;
                }
                Ok(())
            }
            .await;
            match frozen {
                Ok(()) => done.frozen.push(key),
                Err(e) => {
                    done.failure = Some(e);
                    break;
                }
            }
        }
        Ok(done)
    }

    async fn collect_answers(
        &self,
        pr: &PrRef,
        session: &Session,
    ) -> Result<Option<Submission<Receipt>>> {
        // Best effort: a hint that can't be posted must not stop grading.
        if let Err(e) = self.post_hints(pr, session).await {
            eprintln!("warning: could not post a command hint: {e}");
        }
        let comments = self.comments(pr).await?;
        let marker = session_marker(session.key());
        let Some(question) = question_comment(&comments, &marker) else {
            return Ok(None);
        };
        let author = &session.author().0;
        let listed: Vec<_> = comments
            .iter()
            .map(|c| {
                (
                    self.author(c.user.id.into_inner()),
                    c.created_at,
                    c.body.clone().unwrap_or_default(),
                )
            })
            .collect();
        let answer_comment =
            select_answer_comment(&listed, &marker, author, self.app_slug()).map(|i| &comments[i]);
        let from_comment = answer_comment.and_then(|c| {
            parse_answers(
                c.body.as_deref().unwrap_or_default(),
                session.questions(),
                self.app_slug(),
            )
        });

        // Boxes and answer fields count only if the PR author made the last
        // edit (ADR-0004 D3, D11). Ask GraphQL only when they could matter:
        // something is ticked or typed, and the author has submitted.
        let boxes = parse_in_place(
            question.body.as_deref().unwrap_or_default(),
            session.questions(),
        );
        let boxes = if boxes.any() && (boxes.done || from_comment.is_some()) {
            let editor = self.last_editor(&question.node_id).await?;
            (editor.as_deref() == Some(author.as_str())).then_some(boxes)
        } else {
            None
        };
        let source = AnswerSource::read(
            session.questions(),
            answer_comment.map(|c| AnswerComment {
                url: c.html_url.to_string(),
                created_at: c.created_at,
                updated_at: c.updated_at,
            }),
            from_comment.as_deref(),
            boxes.as_ref(),
        );
        let answers = merge_answers(session.questions(), boxes.as_ref(), from_comment);
        // The comment that submitted: the answer comment, else the question
        // comment, whose "Grade my answers" box was ticked.
        let comment_id = answer_comment.unwrap_or(question).id.into_inner();
        Ok(answers.map(|answers| Submission {
            answers,
            receipt: Receipt {
                comment_id,
                source,
                eyes: None,
            },
        }))
    }

    async fn acknowledge_answers(
        &self,
        pr: &PrRef,
        _session: &Session,
        mut receipt: Receipt,
    ) -> Result<Receipt> {
        // Best effort, like every reaction: it only shows progress.
        match self.react(pr, receipt.comment_id, "eyes").await {
            Ok(id) => receipt.eyes = Some(id),
            Err(e) => eprintln!("warning: could not react to the answers: {e}"),
        }
        Ok(receipt)
    }

    async fn post_results(
        &self,
        pr: &PrRef,
        session: &Session,
        outcome: &SessionOutcome,
        receipt: &Receipt,
    ) -> Result<()> {
        let (owner, name) = repo(pr)?;
        let client = self.client()?;
        // Results first: a new comment notifies the author (ADR-0004 D5).
        client
            .issues(owner, name)
            .create_comment(
                pr.number,
                render_results(
                    session,
                    &outcome.results,
                    outcome.passed,
                    Some(&receipt.source),
                ),
            )
            .await
            .map_err(port("post results comment"))?;
        // Then freeze the question comment, so its boxes can't be ticked.
        let comments = self.comments(pr).await?;
        if let Some(question) = question_comment(&comments, &session_marker(session.key())) {
            self.edit_comment(
                pr,
                question.id.into_inner(),
                render_graded_questions(session, &outcome.results),
                "mark question comment graded",
            )
            .await?;
        }
        // Then 👀 becomes 👍: the answers were graded (ADR-0004 D9). GitHub
        // has no ✅ reaction.
        if let Some(eyes) = receipt.eyes
            && let Err(e) = self.unreact(pr, receipt.comment_id, eyes).await
        {
            eprintln!("warning: could not remove the 👀 reaction: {e}");
        }
        if let Err(e) = self.react(pr, receipt.comment_id, "+1").await {
            eprintln!("warning: could not react to the graded answers: {e}");
        }
        Ok(())
    }

    async fn feedback_commands(
        &self,
        pr: &PrRef,
        author: &AuthorId,
    ) -> Result<Vec<FeedbackCommand>> {
        // Keep GitHub's app provenance, which octocrab's Comment omits.
        #[derive(serde::Deserialize)]
        struct FeedbackComment {
            #[serde(flatten)]
            comment: Comment,
            performed_via_github_app: Option<serde_json::Value>,
        }
        let client = self.client()?;
        let (owner, name) = repo(pr)?;
        let page: octocrab::Page<FeedbackComment> = client
            .get(
                format!("/repos/{owner}/{name}/issues/{}/comments", pr.number),
                Some(&[("per_page", 100)]),
            )
            .await
            .map_err(port("list feedback comments"))?;
        let comments = client
            .all_pages(page)
            .await
            .map_err(port("list feedback comments"))?;
        // User tokens authenticate their account. Installation tokens use
        // GitHub's app metadata (the configured app, or GitHub Actions).
        let user: std::result::Result<serde_json::Value, _> =
            client.get("/user", None::<&()>).await;
        let poster = user.ok().and_then(|u| u["id"].as_u64());
        let app = self.app_slug().unwrap_or("github-actions");
        let listed: Vec<ListedComment> = comments
            .iter()
            .map(|entry| {
                let c = &entry.comment;
                ListedComment {
                    trusted: poster == Some(c.user.id.into_inner())
                        || entry
                            .performed_via_github_app
                            .as_ref()
                            .and_then(|app| app["slug"].as_str())
                            == Some(app),
                    id: c.id.into_inner(),
                    user: c.user.id.into_inner(),
                    author: self.author(c.user.id.into_inner()),
                    body: c.body.clone().unwrap_or_default(),
                }
            })
            .collect();
        Ok(select_feedback(&listed, &author.0, self.app_slug())
            .into_iter()
            .map(|(id, session, request)| FeedbackCommand {
                source: feedback_source(&self.host, id),
                session,
                request,
            })
            .collect())
    }

    async fn answer_feedback(
        &self,
        pr: &PrRef,
        command: &FeedbackCommand,
        note: &str,
    ) -> Result<()> {
        let Some(id) = command
            .source
            .rsplit_once(':')
            .and_then(|(_, id)| id.parse::<u64>().ok())
        else {
            return Ok(());
        };
        let (owner, name) = repo(pr)?;
        // The provider keeps nothing between calls, so the comment is looked
        // up again for its URL; without it the note links nothing.
        let url = match self
            .client()?
            .issues(owner, name)
            .get_comment(id.into())
            .await
        {
            Ok(c) => c.html_url.to_string(),
            Err(_) => String::new(),
        };
        // Best effort, like a hint: the feedback is recorded already, and
        // the next run answers it if this fails.
        if let Err(e) = self
            .client()?
            .issues(owner, name)
            .create_comment(pr.number, render_feedback_note(id, &url, note))
            .await
        {
            eprintln!("warning: could not answer a feedback comment: {e}");
        }
        Ok(())
    }

    async fn set_check(
        &self,
        pull: &PullRequest,
        status: CheckStatus,
        summary: &str,
    ) -> Result<()> {
        let (owner, name) = repo(&pull.pr)?;
        let sha = pull.head_sha.clone();
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

    // @adr-0004-d8
    #[test]
    fn only_an_app_slug_is_accepted_for_mentions() {
        let p = || {
            GitHubProvider::new("t".into(), "https://github.com", "https://api.github.com").unwrap()
        };
        assert_eq!(
            p().with_app_slug(Some(" soqrates-review ".into()))
                .app_slug(),
            Some("soqrates-review")
        );
        assert_eq!(p().with_app_slug(Some("".into())).app_slug(), None);
        assert_eq!(p().with_app_slug(Some("@x y".into())).app_slug(), None);
        assert_eq!(p().with_app_slug(None).app_slug(), None);
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
