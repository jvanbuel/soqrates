//! Facts about the local repository, for hooks (ADR-0001 D9, ADR-0003 D7).

use std::process::Command;

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

/// The root of the repository the hook runs in.
pub fn repo_root(vcs: Vcs) -> Option<std::path::PathBuf> {
    let root = match vcs {
        Vcs::Git => run("git", &["rev-parse", "--show-toplevel"]).ok()?,
        Vcs::Jj => run("jj", &["root"]).ok()?,
    };
    Some(root.trim().into())
}

#[cfg(test)]
mod tests {
    use super::host_of_remote;

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
