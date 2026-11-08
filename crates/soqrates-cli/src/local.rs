//! Facts about the local repository, for hooks (ADR-0001 D9, ADR-0003 D7).

use std::process::Command;

use soqrates_core::ports::Codebase;

/// Which version control tool drives the hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vcs {
    Git,
    Jj,
}

fn run(program: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("running {program}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{program} {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The change being committed (git: staged changes) or pushed (jj: the
/// commits between trunk and the working-copy parent).
pub fn diff(vcs: Vcs) -> Result<String, String> {
    match vcs {
        Vcs::Git => run("git", &["diff", "--cached", "--no-color"]),
        Vcs::Jj => run("jj", &["diff", "--git", "--from", "trunk()", "--to", "@-"]),
    }
}

pub fn commit_email(vcs: Vcs) -> Result<String, String> {
    let email = match vcs {
        Vcs::Git => run("git", &["config", "user.email"])?,
        Vcs::Jj => run("jj", &["config", "get", "user.email"])?,
    };
    Ok(email.trim().to_string())
}

/// The host of the `origin` remote, if any.
pub fn remote_host(vcs: Vcs) -> Option<String> {
    let url = match vcs {
        Vcs::Git => run("git", &["remote", "get-url", "origin"]).ok()?,
        Vcs::Jj => run("jj", &["git", "remote", "list"])
            .ok()?
            .lines()
            .find_map(|l| l.strip_prefix("origin ").map(str::to_string))?,
    };
    host_of_remote(url.trim())
}

/// `https://github.com/o/r.git`, `ssh://git@host:22/o/r`, `git@github.com:o/r.git`.
pub fn host_of_remote(url: &str) -> Option<String> {
    let rest = match url.split_once("://") {
        Some((_, rest)) => rest,
        None => url,
    };
    let rest = rest.rsplit_once('@').map_or(rest, |(_, r)| r);
    let host = rest.split(['/', ':']).next()?;
    (!host.is_empty()).then(|| host.to_lowercase())
}

/// Where git runs the pre-commit hook from. Asks git, so that linked
/// worktrees, submodules and `core.hooksPath` are respected.
pub fn git_hook_path(name: &str) -> Result<std::path::PathBuf, String> {
    let path = run(
        "git",
        &["rev-parse", "--git-path", &format!("hooks/{name}")],
    )?;
    let path = std::path::PathBuf::from(path.trim());
    Ok(if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(path)
    })
}

/// The root of the repository the hook runs in.
pub fn repo_root(vcs: Vcs) -> Option<std::path::PathBuf> {
    let root = match vcs {
        Vcs::Git => run("git", &["rev-parse", "--show-toplevel"]).ok()?,
        Vcs::Jj => run("jj", &["root"]).ok()?,
    };
    Some(root.trim().into())
}

/// Run `program` in `dir` and return its raw stdout, or `None` if it
/// fails: the path isn't there, or isn't a file.
fn output_in(
    dir: &std::path::Path,
    program: &str,
    args: &[&str],
) -> Result<Option<Vec<u8>>, String> {
    let out = Command::new(program)
        .current_dir(dir)
        .args(args)
        .output()
        .map_err(|e| format!("running {program}: {e}"))?;
    Ok(out.status.success().then_some(out.stdout))
}

/// What the hook asks about, as a [`Codebase`] (ADR-0008 D1, D4): git's
/// index (what is being committed), or jj's `@-` (what is being pushed).
/// Reads only what version control holds, never the working tree.
pub struct LocalCodebase {
    vcs: Vcs,
    root: std::path::PathBuf,
}

impl LocalCodebase {
    pub fn new(vcs: Vcs, root: std::path::PathBuf) -> Self {
        Self { vcs, root }
    }

    /// Every file path at the revision, relative to the root.
    fn all_files(&self) -> Result<Vec<String>, String> {
        let out = match self.vcs {
            Vcs::Git => output_in(&self.root, "git", &["ls-files", "-z", "--cached"])?,
            Vcs::Jj => output_in(
                &self.root,
                "jj",
                &["--ignore-working-copy", "file", "list", "-r", "@-"],
            )?,
        }
        .ok_or_else(|| "listing the files of the change failed".to_string())?;
        let text = String::from_utf8_lossy(&out);
        let sep = match self.vcs {
            Vcs::Git => '\0',
            Vcs::Jj => '\n',
        };
        Ok(text
            .split(sep)
            .filter(|p| !p.is_empty())
            // jj prints paths with the platform's separator.
            .map(|p| p.replace(std::path::MAIN_SEPARATOR, "/"))
            .collect())
    }
}

/// A jj fileset naming exactly `path`, from the repository root.
fn jj_file(path: &str) -> String {
    format!("root-file:\"{}\"", path.replace('"', "\\\""))
}

/// The text of a blob, or `None` if it is binary.
fn as_text(bytes: Vec<u8>) -> Option<String> {
    if bytes.contains(&0) {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// The entries directly in `dir` (`""` is the root), given every file
/// path: sorted, directories with a trailing `/`. `None` when `dir` holds
/// no file, so is not a directory.
pub fn children(files: &[String], dir: &str) -> Option<Vec<String>> {
    let prefix = if dir.is_empty() {
        String::new()
    } else {
        format!("{dir}/")
    };
    let mut entries: Vec<String> = files
        .iter()
        .filter_map(|f| f.strip_prefix(&prefix))
        .filter(|rest| !rest.is_empty())
        .map(|rest| match rest.split_once('/') {
            Some((sub, _)) => format!("{sub}/"),
            None => rest.to_owned(),
        })
        .collect();
    entries.sort();
    entries.dedup();
    (!entries.is_empty()).then_some(entries)
}

impl Codebase for LocalCodebase {
    async fn read_file(&self, path: &str) -> soqrates_core::Result<Option<String>> {
        let Some(path) = soqrates_core::code_path(path).filter(|p| !p.is_empty()) else {
            return Ok(None);
        };
        let out = match self.vcs {
            // `:path` is the staged blob, from the repository root.
            Vcs::Git => output_in(&self.root, "git", &["show", &format!(":{path}")]),
            Vcs::Jj => output_in(
                &self.root,
                "jj",
                &[
                    "--ignore-working-copy",
                    "file",
                    "show",
                    "-r",
                    "@-",
                    "--",
                    &jj_file(&path),
                ],
            ),
        }
        .map_err(soqrates_core::Error::Port)?;
        Ok(out.and_then(as_text))
    }

    async fn list_dir(&self, path: &str) -> soqrates_core::Result<Option<Vec<String>>> {
        let Some(path) = soqrates_core::code_path(path) else {
            return Ok(None);
        };
        let files = self.all_files().map_err(soqrates_core::Error::Port)?;
        Ok(children(&files, &path))
    }
}

#[cfg(test)]
mod tests {
    use super::{children, host_of_remote, jj_file};

    // @adr-0008-d4
    #[test]
    fn listings_show_one_level() {
        let files: Vec<String> = ["Cargo.toml", "src/lib.rs", "src/app/ci.rs", "srcs.txt"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            children(&files, "").unwrap(),
            vec!["Cargo.toml", "src/", "srcs.txt"]
        );
        assert_eq!(children(&files, "src").unwrap(), vec!["app/", "lib.rs"]);
        assert_eq!(children(&files, "src/lib.rs"), None);
        assert_eq!(children(&files, "missing"), None);
        assert_eq!(jj_file("a \"b\".rs"), "root-file:\"a \\\"b\\\".rs\"");
    }

    #[test]
    fn remote_hosts() {
        for (url, host) in [
            ("https://github.com/jvanbuel/soqrates.git", "github.com"),
            ("git@github.com:jvanbuel/soqrates.git", "github.com"),
            (
                "ssh://git@gitlab.client.example:2222/a/b",
                "gitlab.client.example",
            ),
            ("https://user:token@GitLab.Example/a/b", "gitlab.example"),
        ] {
            assert_eq!(host_of_remote(url).as_deref(), Some(host), "{url}");
        }
    }
}
