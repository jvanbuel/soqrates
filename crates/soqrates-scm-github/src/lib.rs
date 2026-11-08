//! `ScmProvider` for GitHub (ADR-0001 D4). This is the only crate that may
//! depend on `octocrab`.
//!
//! Questions are posted as one PR comment carrying a hidden session marker.
//! The author answers multiple choice by ticking boxes in it and submits by
//! ticking "Grade my answers", or answers with a `/soqrates answer` comment
//! (ADR-0001 D8, ADR-0004 D3). The command is read forgivingly (`/socrates`,
//! any case, or `@<app slug> answer`), and a mistyped one gets a hint
//! (ADR-0004 D8). Boxes count only when GraphQL reports the PR
//! author as the comment's last editor. The comment that submitted the
//! answers gets a 👀 reaction while they are graded, then a 👍 (ADR-0004
//! D9). Results are posted as a new comment, saying which answer comment
//! and boxes were read, and the question comment is frozen (ADR-0004 D5). A newer push freezes
//! the earlier question comments that were never graded (D7). The
//! check is reported as a commit status with context `soqrates`, which works
//! with the Actions `GITHUB_TOKEN` given `statuses: write`. Authors are
//! identified by host and numeric user id, never by login (ADR-0003 D1).

mod format;

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use octocrab::Octocrab;
use octocrab::models::issues::Comment;
use soqrates_core::answer_command::{hints_due, parse_answers};
use soqrates_core::ports::ScmProvider;
use soqrates_core::{
    Answer, AuthorId, CheckStatus, Diff, Error, PrRef, PullRequest, Result, Session, SessionKey,
    SessionOutcome,
};

use crate::format::{
    AnswerComment, AnswerSource, ContentEntry, EDITOR_QUERY, STATUS_CONTEXT, StatusState,
    author_id, boxes_read, commit_status, diff_of_files, dir_entries, editor_database_id,
    encode_path, file_text, graphql_base, hinted_comment, host_of, is_slug, marker_key,
    merge_answers, parse_checkboxes, render_graded_questions, render_hint, render_questions,
    render_results, render_superseded_questions, select_answer_comment, session_marker, split_repo,
    takes_answers,
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
    /// Head commits seen by `pull_request`, for `set_check`.
    head_shas: Mutex<HashMap<PrRef, String>>,
    /// The slug of the app Soqrates posts as, if any: `@<slug> answer` is
    /// then an answer command too (ADR-0004 D8, ADR-0006 D4).
    app_slug: Option<String>,
    /// How the answers found by `fetch_answers` were submitted, for
    /// `acknowledge_answers` and `post_results`.
    submissions: Mutex<HashMap<SessionKey, Submission>>,
}

/// How a session's answers were submitted (ADR-0004 D8, D9).
#[derive(Debug, Clone)]
struct Submission {
    /// The comment that submitted them: the answer comment, or the question
    /// comment when "Grade my answers" was ticked.
    comment_id: u64,
    source: AnswerSource,
    /// The 👀 reaction added by `acknowledge_answers`.
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
            head_shas: Mutex::new(HashMap::new()),
            app_slug: None,
            submissions: Mutex::new(HashMap::new()),
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

    fn submission(&self, key: &SessionKey) -> Option<Submission> {
        self.submissions.lock().ok()?.get(key).cloned()
    }

    fn set_submission(&self, key: &SessionKey, submission: Option<Submission>) {
        if let Ok(mut map) = self.submissions.lock() {
            match submission {
                Some(s) => map.insert(key.clone(), s),
                None => map.remove(key),
            };
        }
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

    /// The contents API reply for `path` at commit `revision`: the file, or
    /// a directory's entries. `None` when there is nothing at `path`
    /// (ADR-0008 D4). Works for commits of forks too: GitHub keeps a PR's
    /// head commits in the base repository.
    async fn contents(
        &self,
        pr: &PrRef,
        revision: &str,
        path: &str,
    ) -> Result<Option<Vec<ContentEntry>>> {
        let (owner, name) = repo(pr)?;
        let reply = self
            .client()?
            .repos(owner, name)
            .get_content()
            .path(encode_path(path))
            .r#ref(revision)
            .send()
            .await;
        let items = match reply {
            Ok(items) => items.items,
            Err(octocrab::Error::GitHub { source, .. }) if source.status_code.as_u16() == 404 => {
                return Ok(None);
            }
            Err(e) => return Err(port("get contents")(e)),
        };
        Ok(Some(
            items
                .into_iter()
                .map(|c| ContentEntry {
                    // Inlined as base64 only up to 1 MB; larger files
                    // have encoding `none` and no content.
                    text: (c.encoding.as_deref() == Some("base64"))
                        .then(|| c.decoded_content())
                        .flatten(),
                    name: c.name,
                    path: c.path,
                    kind: c.r#type,
                })
                .collect(),
        ))
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

    async fn read_file(&self, pr: &PrRef, revision: &str, path: &str) -> Result<Option<String>> {
        Ok(self
            .contents(pr, revision, path)
            .await?
            .and_then(|entries| file_text(entries, path)))
    }

    async fn list_dir(
        &self,
        pr: &PrRef,
        revision: &str,
        path: &str,
    ) -> Result<Option<Vec<String>>> {
        Ok(self
            .contents(pr, revision, path)
            .await?
            .and_then(|entries| dir_entries(entries, path)))
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

        // Boxes count only if the PR author made the last edit (ADR-0004
        // D3). Ask GraphQL only when the boxes could matter: something is
        // ticked, and the author has submitted.
        let boxes = parse_checkboxes(
            question.body.as_deref().unwrap_or_default(),
            session.questions(),
        );
        let boxes = if boxes.any() && (boxes.done || from_comment.is_some()) {
            let editor = self.last_editor(&question.node_id).await?;
            (editor.as_deref() == Some(author.as_str())).then_some(boxes)
        } else {
            None
        };
        let source = AnswerSource {
            comment: answer_comment.map(|c| AnswerComment {
                url: c.html_url.to_string(),
                created_at: c.created_at,
                updated_at: c.updated_at,
            }),
            boxes: boxes_read(session.questions(), boxes.as_ref(), from_comment.as_deref()),
        };
        let answers = merge_answers(session.questions(), boxes.as_ref(), from_comment);
        // The comment that submitted: the answer comment, else the question
        // comment, whose "Grade my answers" box was ticked.
        let submission = answers.as_ref().map(|_| Submission {
            comment_id: answer_comment.unwrap_or(question).id.into_inner(),
            source,
            eyes: None,
        });
        self.set_submission(session.key(), submission);
        Ok(answers)
    }

    async fn hint_commands(&self, pr: &PrRef, session: &Session) -> Result<()> {
        // Best effort: a hint that can't be posted must not stop grading.
        if let Err(e) = self.post_hints(pr, session).await {
            eprintln!("warning: could not post a command hint: {e}");
        }
        Ok(())
    }

    async fn acknowledge_answers(&self, pr: &PrRef, session: &Session) -> Result<()> {
        let Some(mut submission) = self.submission(session.key()) else {
            return Ok(());
        };
        // Best effort, like every reaction: it only shows progress.
        match self.react(pr, submission.comment_id, "eyes").await {
            Ok(id) => submission.eyes = Some(id),
            Err(e) => eprintln!("warning: could not react to the answers: {e}"),
        }
        self.set_submission(session.key(), Some(submission));
        Ok(())
    }

    async fn post_results(
        &self,
        pr: &PrRef,
        session: &Session,
        outcome: &SessionOutcome,
    ) -> Result<()> {
        let (owner, name) = repo(pr)?;
        let client = self.client()?;
        let submission = self.submission(session.key());
        // Results first: a new comment notifies the author (ADR-0004 D5).
        client
            .issues(owner, name)
            .create_comment(
                pr.number,
                render_results(
                    session,
                    &outcome.results,
                    outcome.passed,
                    submission.as_ref().map(|s| &s.source),
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
        if let Some(s) = submission {
            if let Some(eyes) = s.eyes
                && let Err(e) = self.unreact(pr, s.comment_id, eyes).await
            {
                eprintln!("warning: could not remove the 👀 reaction: {e}");
            }
            if let Err(e) = self.react(pr, s.comment_id, "+1").await {
                eprintln!("warning: could not react to the graded answers: {e}");
            }
            self.set_submission(session.key(), None);
        }
        Ok(())
    }

    async fn superseded_questions(&self, pr: &PrRef, current: &Session) -> Result<Vec<SessionKey>> {
        let comments = self.comments(pr).await?;
        let Some(ours) = question_comment(&comments, &session_marker(current.key())) else {
            return Ok(Vec::new());
        };
        // Only comments posted by the account that posts Soqrates' questions,
        // so nobody can have another comment rewritten by copying a marker.
        let poster = ours.user.id;
        let mut keys: Vec<SessionKey> = Vec::new();
        for c in comments.iter().filter(|c| c.user.id == poster) {
            let body = c.body.as_deref().unwrap_or_default();
            if let Some(key) = marker_key(body)
                && key != *current.key()
                && takes_answers(body)
                && !keys.contains(&key)
            {
                keys.push(key);
            }
        }
        Ok(keys)
    }

    async fn supersede_questions(
        &self,
        pr: &PrRef,
        superseded: &SessionKey,
        session: Option<&Session>,
        current: &Session,
    ) -> Result<()> {
        let comments = self.comments(pr).await?;
        let Some(ours) = question_comment(&comments, &session_marker(current.key())) else {
            return Ok(());
        };
        let old = comments.iter().filter(|c| {
            c.user.id == ours.user.id
                && c.body
                    .as_deref()
                    .is_some_and(|b| marker_key(b).as_ref() == Some(superseded) && takes_answers(b))
        });
        let body = render_superseded_questions(
            superseded,
            session,
            Some(ours.html_url.as_str()),
            !current.questions().is_empty(),
        );
        for c in old {
            self.edit_comment(
                pr,
                c.id.into_inner(),
                body.clone(),
                "mark question comment superseded",
            )
            .await?;
        }
        Ok(())
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
