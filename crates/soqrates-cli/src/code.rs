//! The codebase the examiner reads, from git (ADR-0008 D1, D4).
//!
//! Every surface reads git objects, never the working tree:
//!
//! - `pre-commit`: the index, i.e. what is being committed;
//! - jj `pre-push`: the commit `@-`, in jj's git store;
//! - CI: a commit of the pull request, in the checkout the action made.
//!
//! So untracked and ignored files, such as a local `.env`, are never read.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use chrono::DateTime;
use soqrates_core::ports::{BlameLine, CodeCommit, CodeMatch, CodeSource, Codebase};
use soqrates_core::{Error, Result};

/// Which version of the files to read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Revision {
    /// The index: what `git commit` would commit.
    Index,
    /// A commit, by id.
    Commit(String),
}

/// A git repository at one revision, as a [`Codebase`].
#[derive(Debug, Clone)]
pub struct GitCodebase {
    /// Where git runs.
    dir: PathBuf,
    /// The git directory, when it isn't found from `dir` (jj's store).
    git_dir: Option<PathBuf>,
    revision: Revision,
    /// Whether the revision is there to read.
    available: bool,
}

/// The output of a git command, or `None` when it failed: the path or
/// revision isn't there.
fn git(
    dir: &Path,
    git_dir: Option<&Path>,
    args: &[&str],
    stdin: Option<&[u8]>,
) -> Result<Option<Vec<u8>>> {
    let mut command = git_command(dir, git_dir, args);
    command
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command
        .spawn()
        .map_err(|e| Error::Port(format!("running git: {e}")))?;
    if let Some(input) = stdin {
        let mut pipe = child.stdin.take().expect("stdin is piped");
        pipe.write_all(input)
            .map_err(|e| Error::Port(format!("writing to git: {e}")))?;
    }
    let out = child
        .wait_with_output()
        .map_err(|e| Error::Port(format!("running git: {e}")))?;
    Ok(out.status.success().then_some(out.stdout))
}

/// `git args`, run in `dir`, against `git_dir` if given.
fn git_command(dir: &Path, git_dir: Option<&Path>, args: &[&str]) -> Command {
    let mut command = Command::new("git");
    command.current_dir(dir);
    if let Some(git_dir) = git_dir {
        command.arg("--git-dir").arg(git_dir);
    }
    // Repository content must not steer git: no pager, no external diff or
    // text conversion, no prompts.
    command
        .args([
            "--no-pager",
            "-c",
            "core.quotepath=off",
            "-c",
            "diff.external=",
        ])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0");
    command
}

/// The first `limit` lines `git args` prints, read as they come: git is
/// stopped once there are enough, so a broad search never fills memory.
/// `Ok(None)` when git exits with status 1 (`git grep`: nothing found); an
/// error with git's message for any other failure, e.g. an invalid
/// regular expression.
fn git_lines(
    dir: &Path,
    git_dir: Option<&Path>,
    args: &[&str],
    limit: usize,
) -> Result<Option<Vec<String>>> {
    use std::io::{BufRead, BufReader, Read};
    let mut child = git_command(dir, git_dir, args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error::Port(format!("running git: {e}")))?;
    let stdout = child.stdout.take().expect("stdout is piped");
    let mut lines = Vec::new();
    for line in BufReader::new(stdout).split(b'\n') {
        let line = line.map_err(|e| Error::Port(format!("reading from git: {e}")))?;
        lines.push(String::from_utf8_lossy(&line).into_owned());
        if lines.len() >= limit {
            // Enough: what git would print next is never read.
            let _ = child.kill();
            let _ = child.wait();
            return Ok(Some(lines));
        }
    }
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    let status = child
        .wait()
        .map_err(|e| Error::Port(format!("running git: {e}")))?;
    match status.code() {
        Some(0) => Ok(Some(lines)),
        Some(1) => Ok(None),
        _ => {
            let why = stderr
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("git failed")
                .trim()
                .to_owned();
            Err(Error::Port(format!(
                "git {}: {why}",
                args.first().unwrap_or(&"")
            )))
        }
    }
}

fn text(bytes: Vec<u8>) -> Option<String> {
    if bytes.contains(&0) {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// A glob as a git pathspec: `**` crosses directories, `*` doesn't.
fn glob_pathspec(glob: &str) -> String {
    format!(":(glob){glob}")
}

impl GitCodebase {
    /// The index of the repository at `dir` (`pre-commit`).
    pub fn index(dir: PathBuf) -> Self {
        GitCodebase {
            dir,
            git_dir: None,
            revision: Revision::Index,
            available: true,
        }
    }

    /// The commit `id` of the repository at `dir`, or of the git directory
    /// `git_dir`; not available if git doesn't have it.
    pub fn commit(dir: PathBuf, git_dir: Option<PathBuf>, id: &str) -> Self {
        let available = !id.starts_with('-')
            && matches!(
                git(
                    &dir,
                    git_dir.as_deref(),
                    &["cat-file", "-e", &format!("{id}^{{commit}}")],
                    None
                ),
                Ok(Some(_))
            );
        GitCodebase {
            dir,
            git_dir,
            revision: Revision::Commit(id.to_owned()),
            available,
        }
    }

    /// The commit jj pushes, `@-`, in the jj repository at `dir`; not
    /// available when jj can't say, e.g. a version without `jj git root`.
    pub fn jj_parent(dir: PathBuf) -> Self {
        Self::try_jj_parent(dir.clone()).unwrap_or(GitCodebase {
            dir,
            git_dir: None,
            revision: Revision::Index,
            available: false,
        })
    }

    fn try_jj_parent(dir: PathBuf) -> Option<Self> {
        let jj = |args: &[&str]| -> Option<String> {
            let out = Command::new("jj")
                .current_dir(&dir)
                .args(args)
                .stderr(Stdio::null())
                .output()
                .ok()?;
            out.status
                .success()
                .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
        };
        let id = jj(&[
            "--ignore-working-copy",
            "log",
            "-r",
            "@-",
            "--no-graph",
            "-T",
            "commit_id",
        ])?;
        let git_dir = jj(&["--ignore-working-copy", "git", "root"])?;
        Some(Self::commit(dir.clone(), Some(PathBuf::from(git_dir)), &id))
    }

    fn run(&self, args: &[&str], stdin: Option<&[u8]>) -> Result<Option<Vec<u8>>> {
        git(&self.dir, self.git_dir.as_deref(), args, stdin)
    }

    /// The object name of `path` at the revision: `:path` or `id:path`.
    fn object(&self, path: &str) -> String {
        match &self.revision {
            Revision::Index => format!(":{path}"),
            Revision::Commit(id) => format!("{id}:{path}"),
        }
    }

    /// The commit history starts from: `HEAD` for the index.
    fn tip(&self) -> &str {
        match &self.revision {
            Revision::Index => "HEAD",
            Revision::Commit(id) => id,
        }
    }

    fn blob(&self, path: &str) -> Result<Option<Vec<u8>>> {
        if path.is_empty() {
            return Ok(None);
        }
        let object = self.object(path);
        let kind = self.run(&["cat-file", "-t", &object], None)?;
        if kind
            .as_deref()
            .map(|k| String::from_utf8_lossy(k).trim().to_owned())
            != Some("blob".into())
        {
            return Ok(None);
        }
        self.run(&["cat-file", "blob", &object], None)
    }
}

impl Codebase for GitCodebase {
    fn is_available(&self) -> bool {
        self.available
    }

    async fn read_file(&self, path: &str) -> Result<Option<String>> {
        Ok(self.blob(path)?.and_then(text))
    }

    async fn list_files(&self) -> Result<Vec<String>> {
        let out = match &self.revision {
            Revision::Index => self.run(&["ls-files", "-z", "--cached"], None)?,
            Revision::Commit(id) => self.run(&["ls-tree", "-r", "-z", "--name-only", id], None)?,
        }
        .ok_or_else(|| Error::Port("listing the files of the change failed".into()))?;
        let mut files: Vec<String> = String::from_utf8_lossy(&out)
            .split('\0')
            .filter(|p| !p.is_empty())
            .map(str::to_owned)
            .collect();
        files.sort();
        // A conflicted index lists a path once per stage.
        files.dedup();
        Ok(files)
    }

    async fn grep(
        &self,
        pattern: &str,
        glob: Option<&str>,
        ignore_case: bool,
        limit: usize,
    ) -> Result<Vec<CodeMatch>> {
        let mut args = vec!["grep", "-z", "-n", "-I", "--no-color", "-E"];
        if ignore_case {
            args.push("-i");
        }
        args.extend(["-e", pattern]);
        let tip;
        match &self.revision {
            Revision::Index => args.push("--cached"),
            Revision::Commit(id) => {
                tip = id.clone();
                args.push(&tip);
            }
        }
        args.push("--");
        let spec = glob.map(glob_pathspec);
        if let Some(spec) = &spec {
            args.push(spec);
        }
        // Status 1 is "nothing found"; any other failure, such as an invalid
        // expression, is reported, so the model doesn't take it for "no
        // such code".
        let Some(lines) = git_lines(&self.dir, self.git_dir.as_deref(), &args, limit)? else {
            return Ok(Vec::new());
        };
        let prefix = match &self.revision {
            Revision::Index => String::new(),
            Revision::Commit(id) => format!("{id}:"),
        };
        Ok(lines
            .iter()
            .filter_map(|line| {
                let mut parts = line.splitn(3, '\0');
                let path = parts.next()?;
                let number = parts.next()?.parse().ok()?;
                let text = parts.next().unwrap_or_default();
                Some(CodeMatch {
                    path: path.strip_prefix(&prefix).unwrap_or(path).to_owned(),
                    line: number,
                    text: text.to_owned(),
                })
            })
            .take(limit)
            .collect())
    }

    async fn history(&self, path: &str, limit: usize) -> Result<Vec<CodeCommit>> {
        let limit = limit.to_string();
        let mut args = vec![
            "log",
            "-n",
            &limit,
            "--date=short",
            "--format=%h%x1f%an%x1f%ad%x1f%s",
            self.tip(),
            "--",
        ];
        if !path.is_empty() {
            args.push(path);
        }
        // No commits yet, or not in a shallow history: none to show.
        let Some(out) = self.run(&args, None)? else {
            return Ok(Vec::new());
        };
        Ok(String::from_utf8_lossy(&out)
            .lines()
            .filter_map(|line| {
                let mut parts = line.split('\x1f');
                Some(CodeCommit {
                    id: parts.next()?.to_owned(),
                    author: parts.next()?.to_owned(),
                    date: parts.next()?.to_owned(),
                    summary: parts.next().unwrap_or_default().to_owned(),
                })
            })
            .collect())
    }

    async fn blame(&self, path: &str, start: u32, end: u32) -> Result<Option<Vec<BlameLine>>> {
        let Some(blob) = self.blob(path)? else {
            return Ok(None);
        };
        let lines =
            u32::try_from(String::from_utf8_lossy(&blob).lines().count()).unwrap_or(u32::MAX);
        let start = start.max(1);
        let end = end.min(lines);
        if lines == 0 || start > end {
            return Ok(Some(Vec::new()));
        }
        let range = format!("{start},{end}");
        let out = match &self.revision {
            // The staged content, on top of HEAD.
            Revision::Index => self.run(
                &[
                    "blame",
                    "--porcelain",
                    "-L",
                    &range,
                    "--contents",
                    "-",
                    "--",
                    path,
                ],
                Some(&blob),
            )?,
            Revision::Commit(id) => self.run(
                &["blame", "--porcelain", "-L", &range, id, "--", path],
                None,
            )?,
        };
        Ok(out.map(|out| parse_porcelain(&String::from_utf8_lossy(&out))))
    }
}

/// Parse `git blame --porcelain`: a header per line, the commit's details
/// the first time it appears, then the line itself after a tab.
pub fn parse_porcelain(out: &str) -> Vec<BlameLine> {
    use std::collections::HashMap;
    let mut authors: HashMap<String, (String, String)> = HashMap::new();
    let mut lines = Vec::new();
    let mut current: Option<(String, u32)> = None;
    let mut author = String::new();
    let mut date = String::new();
    for line in out.lines() {
        if let Some(code) = line.strip_prefix('\t') {
            if let Some((commit, number)) = current.take() {
                let (who, when) = authors
                    .entry(commit.clone())
                    .or_insert_with(|| (author.clone(), date.clone()))
                    .clone();
                lines.push(BlameLine {
                    line: number,
                    commit: commit.chars().take(12).collect(),
                    author: who,
                    date: when,
                    text: code.to_owned(),
                });
            }
            author.clear();
            date.clear();
        } else if let Some(name) = line.strip_prefix("author ") {
            author = name.to_owned();
        } else if let Some(time) = line.strip_prefix("author-time ") {
            date = time
                .trim()
                .parse::<i64>()
                .ok()
                .and_then(|t| DateTime::from_timestamp(t, 0))
                .map(|t| t.format("%Y-%m-%d").to_string())
                .unwrap_or_default();
        } else {
            let mut parts = line.split(' ');
            if let (Some(sha), Some(_), Some(number)) = (parts.next(), parts.next(), parts.next())
                && sha.len() >= 40
                && sha.chars().all(|c| c.is_ascii_hexdigit())
                && let Ok(number) = number.parse()
            {
                current = Some((sha.to_owned(), number));
            }
        }
    }
    lines
}

/// The checkout the action made of the pull request (ADR-0008 D4): any
/// commit in it, by id.
#[derive(Debug, Clone)]
pub struct GitCheckout {
    pub dir: PathBuf,
}

impl CodeSource for GitCheckout {
    type Code = GitCodebase;
    fn at(&self, revision: &str) -> GitCodebase {
        GitCodebase::commit(self.dir.clone(), None, revision)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch repository with one commit, and a staged change on top.
    fn repo() -> (PathBuf, String) {
        let dir = std::env::temp_dir().join(format!(
            "soqrates-code-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let sh = |args: &[&str]| {
            let ok = Command::new("git")
                .current_dir(&dir)
                .args(args)
                .env("GIT_AUTHOR_NAME", "Ada")
                .env("GIT_AUTHOR_EMAIL", "ada@example.com")
                .env("GIT_COMMITTER_NAME", "Ada")
                .env("GIT_COMMITTER_EMAIL", "ada@example.com")
                .env("GIT_AUTHOR_DATE", "2026-01-02T03:04:05Z")
                .env("GIT_COMMITTER_DATE", "2026-01-02T03:04:05Z")
                .output()
                .unwrap();
            assert!(
                ok.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&ok.stderr)
            );
            String::from_utf8_lossy(&ok.stdout).trim().to_owned()
        };
        sh(&["init", "-q"]);
        std::fs::write(dir.join("src/lib.rs"), "fn one() {}\nfn two() {}\n").unwrap();
        std::fs::write(dir.join("README.md"), "# demo\n").unwrap();
        sh(&["add", "."]);
        sh(&["commit", "-q", "-m", "Add one and two"]);
        let head = sh(&["rev-parse", "HEAD"]);
        // Staged, not committed.
        std::fs::write(
            dir.join("src/lib.rs"),
            "fn one() {}\nfn two() {}\nfn three() {}\n",
        )
        .unwrap();
        sh(&["add", "src/lib.rs"]);
        // In the working tree only: never read.
        std::fs::write(dir.join(".env"), "SECRET=1\n").unwrap();
        std::fs::write(dir.join("src/lib.rs"), "fn unstaged() {}\n").unwrap();
        (dir, head)
    }

    // @adr-0008-d1 @adr-0008-d4
    #[tokio::test]
    async fn the_index_is_read_never_the_working_tree() {
        let (dir, _) = repo();
        let code = GitCodebase::index(dir.clone());
        assert_eq!(
            code.read_file("src/lib.rs").await.unwrap().as_deref(),
            Some("fn one() {}\nfn two() {}\nfn three() {}\n")
        );
        assert_eq!(code.read_file(".env").await.unwrap(), None);
        assert_eq!(code.read_file("src").await.unwrap(), None);
        assert_eq!(
            code.list_files().await.unwrap(),
            vec!["README.md", "src/lib.rs"]
        );
        let hits = code.grep("fn t", Some("**/*.rs"), false, 10).await.unwrap();
        let lines: Vec<u32> = hits.iter().map(|h| h.line).collect();
        assert_eq!(lines, vec![2, 3]);
        assert!(
            code.grep("unstaged", None, false, 10)
                .await
                .unwrap()
                .is_empty()
        );
        let blame = code.blame("src/lib.rs", 2, 99).await.unwrap().unwrap();
        assert_eq!(blame.len(), 2);
        assert_eq!(blame[0].author, "Ada");
        assert_eq!(blame[0].date, "2026-01-02");
        assert!(blame[1].commit.chars().all(|c| c == '0'), "{blame:?}");
        let history = code.history("src/lib.rs", 5).await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].summary, "Add one and two");
        std::fs::remove_dir_all(dir).ok();
    }

    // @adr-0008-d4
    #[tokio::test]
    async fn a_commit_is_read_as_committed() {
        let (dir, head) = repo();
        let code = GitCheckout { dir: dir.clone() }.at(&head);
        assert!(code.is_available());
        assert_eq!(
            code.read_file("src/lib.rs").await.unwrap().as_deref(),
            Some("fn one() {}\nfn two() {}\n")
        );
        let hits = code.grep("TWO", None, true, 10).await.unwrap();
        assert_eq!(
            hits,
            vec![CodeMatch {
                path: "src/lib.rs".into(),
                line: 2,
                text: "fn two() {}".into(),
            }]
        );
        assert_eq!(code.grep("fn", None, false, 1).await.unwrap().len(), 1);
        assert!(
            code.grep("no such code", None, false, 10)
                .await
                .unwrap()
                .is_empty()
        );
        // An invalid expression is an error the model is told about, not
        // "no matches".
        let invalid = code.grep("fn(", None, false, 10).await.unwrap_err();
        assert!(invalid.to_string().contains("git grep"), "{invalid}");
        let blame = code.blame("src/lib.rs", 1, 1).await.unwrap().unwrap();
        assert_eq!(blame[0].text, "fn one() {}");
        assert_eq!(blame[0].commit, head[..12]);
        assert_eq!(code.blame("missing.rs", 1, 1).await.unwrap(), None);
        // A commit that isn't in the checkout can't be read.
        assert!(
            !GitCheckout { dir: dir.clone() }
                .at("0123456789abcdef0123456789abcdef01234567")
                .is_available()
        );
        assert!(
            !GitCheckout { dir: dir.clone() }
                .at("--output=x")
                .is_available()
        );
        std::fs::remove_dir_all(dir).ok();
    }
}
