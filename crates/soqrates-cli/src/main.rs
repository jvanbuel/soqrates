//! The `soqrates` binary (ADR-0001 D1): composes the core with its adapters.
//!
//! ```text
//! soqrates hook pre-commit        # git pre-commit hook
//! soqrates hook pre-push          # jj, before pushing
//! soqrates ci                     # PR check in GitHub Actions
//! soqrates mcp                    # MCP server over stdio
//! soqrates identity ...           # manage linked identities (ADR-0003)
//! soqrates install-hook           # install the git pre-commit hook
//! ```

mod config;
mod event;
mod local;
mod prompter;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand, ValueEnum};
use soqrates_core::ports::Clock;
use soqrates_core::{AuthorId, Diff, HookDecision, HookKind, Identity, PrRef, Soqrates, ci, hook};
use soqrates_examiner::{DEFAULT_MODEL, ModelSpec, RigExaminer};
use soqrates_scm_github::GitHubProvider;
use soqrates_store::FileStore;

use crate::config::ModeName;
use crate::local::Vcs;

#[derive(Parser)]
#[command(version, about = "Soqrates: learn the concepts in your own changes")]
struct Cli {
    /// Directory of the learner store (default: the platform data dir).
    #[arg(long, global = true, env = "SOQRATES_STATE_DIR")]
    state_dir: Option<PathBuf>,
    /// Model, e.g. `anthropic:claude-sonnet-5-5`.
    #[arg(long, global = true, env = "SOQRATES_MODEL")]
    model: Option<String>,
    /// Whether a failed session blocks.
    #[arg(long, global = true, value_enum, env = "SOQRATES_MODE")]
    mode: Option<ModeName>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run as a local hook (ADR-0001 D9).
    Hook {
        #[arg(value_enum)]
        kind: HookArg,
    },
    /// Run the PR check (ADR-0001 D8). Reads GitHub Actions' environment.
    Ci {
        /// `owner/name`; defaults to GITHUB_REPOSITORY.
        #[arg(long, env = "GITHUB_REPOSITORY")]
        repo: String,
        /// PR number; defaults to the one in GITHUB_EVENT_PATH.
        #[arg(long)]
        pr: Option<u64>,
    },
    /// Serve the MCP tools over stdio (ADR-0001 D10).
    Mcp,
    /// Manage your identities (ADR-0003).
    Identity {
        #[command(subcommand)]
        command: IdentityCommand,
    },
    /// Install `soqrates hook pre-commit` as this repository's git hook.
    InstallHook {
        /// Replace an existing pre-commit hook.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum HookArg {
    /// git pre-commit
    PreCommit,
    /// jj, before `jj git push`
    PrePush,
}

#[derive(Subcommand)]
enum IdentityCommand {
    /// Use this identity for repositories on its forge, e.g. `github.com:1001`.
    Use { identity: String },
    /// List the identities of your learner.
    List,
    /// Ask for a link code, to present from your other identity.
    RequestLink,
    /// Present a link code from another identity, linking the two.
    Link { code: String },
    /// Unlink the current identity from your learner.
    Unlink,
    /// Delete your learner profile and all links.
    Delete {
        #[arg(long)]
        yes: bool,
    },
}

struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

type App = Soqrates<FileStore, RigExaminer, SystemClock>;

/// The effective configuration: user file, project file, then flags.
fn settings(cli: &Cli, project_root: Option<&std::path::Path>) -> Result<config::File, String> {
    let mut file = config::File::load(&config::user_config_path())?;
    if let Some(root) = project_root {
        file = file.overlay(config::File::load(&root.join(".soqrates.toml"))?);
    }
    Ok(file.overlay(config::File {
        model: cli.model.clone(),
        mode: cli.mode,
        state_dir: cli.state_dir.clone(),
        ..config::File::default()
    }))
}

fn build_app(settings: &config::File) -> Result<App, String> {
    let model = ModelSpec::parse(settings.model.as_deref().unwrap_or(DEFAULT_MODEL))
        .map_err(|e| e.to_string())?;
    let examiner = RigExaminer::new(model).map_err(|e| e.to_string())?;
    let defaults = soqrates_core::Config::default();
    let core = soqrates_core::Config {
        max_questions: settings.max_questions.unwrap_or(defaults.max_questions),
        pass_threshold: settings.pass_threshold.unwrap_or(defaults.pass_threshold),
        mode: settings.mode.map(Into::into).unwrap_or(defaults.mode),
        ..defaults
    };
    let dir = settings
        .state_dir
        .clone()
        .unwrap_or_else(config::default_state_dir);
    Ok(Soqrates::new(
        FileStore::new(dir),
        examiner,
        SystemClock,
        core,
    ))
}

/// The identity to act as locally (ADR-0003 D7).
fn local_author(vcs: Vcs, settings: &config::File) -> Result<AuthorId, String> {
    let identity = soqrates_core::local_identity(
        local::remote_host(vcs).as_deref(),
        &local::commit_email(vcs)?,
        &settings.parsed_identities()?,
    );
    Ok(AuthorId(identity.to_string()))
}

/// 16 characters from the OS random source, e.g. `K7QX-M2PA-...`.
fn link_code() -> Result<String, String> {
    const ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| e.to_string())?;
    let chars: Vec<char> = bytes
        .iter()
        .map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char)
        .collect();
    Ok(chars
        .chunks(4)
        .map(|c| c.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join("-"))
}

async fn run_hook(cli: &Cli, kind: HookArg) -> Result<ExitCode, String> {
    let (vcs, kind) = match kind {
        HookArg::PreCommit => (Vcs::Git, HookKind::PreCommit),
        HookArg::PrePush => (Vcs::Jj, HookKind::PrePush),
    };
    let settings = settings(cli, local::repo_root(vcs).as_deref())?;
    let diff = Diff(local::diff(vcs)?);
    if diff.is_empty() {
        return Ok(ExitCode::SUCCESS);
    }
    let Some(mut prompter) = prompter::TtyPrompter::open() else {
        eprintln!("soqrates: no terminal to ask questions on; skipping.");
        return Ok(ExitCode::SUCCESS);
    };
    let app = build_app(&settings)?;
    let author = local_author(vcs, &settings)?;
    let decision = hook::run_hook(&app, &mut prompter, kind, &author, &diff)
        .await
        .map_err(|e| e.to_string())?;
    Ok(match decision {
        HookDecision::Allow => ExitCode::SUCCESS,
        HookDecision::Block => {
            eprintln!("soqrates: blocking mode and the session was not passed.");
            ExitCode::FAILURE
        }
    })
}

async fn run_ci(cli: &Cli, repo: &str, pr: Option<u64>) -> Result<ExitCode, String> {
    let number = match pr {
        Some(n) => Some(n),
        None => {
            let path = std::env::var("GITHUB_EVENT_PATH")
                .map_err(|_| "no --pr and no GITHUB_EVENT_PATH".to_string())?;
            let text = std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
            let event = serde_json::from_str(&text).map_err(|e| format!("{path}: {e}"))?;
            event::pr_number(&event)
        }
    };
    let Some(number) = number else {
        println!("soqrates: not a pull request; nothing to do.");
        return Ok(ExitCode::SUCCESS);
    };
    let settings = settings(cli, std::env::current_dir().ok().as_deref())?;
    let app = build_app(&settings)?;
    let scm = GitHubProvider::from_env().map_err(|e| e.to_string())?;
    let pr = PrRef {
        repo: repo.to_string(),
        number,
    };
    let status = ci::run_check(&app, &scm, &pr)
        .await
        .map_err(|e| e.to_string())?;
    println!("soqrates: {repo}#{number}: {status:?}");
    Ok(ExitCode::SUCCESS)
}

async fn run_identity(cli: &Cli, command: &IdentityCommand) -> Result<ExitCode, String> {
    let vcs = Vcs::Git;
    if let IdentityCommand::Use { identity } = command {
        let parsed = Identity::parse(identity).map_err(|e| e.to_string())?;
        let host = parsed
            .host()
            .ok_or("only forge identities (host:account) can be configured")?
            .to_string();
        let path = config::user_config_path();
        let mut file = config::File::load(&path)?;
        file.identities.insert(host.clone(), parsed.to_string());
        file.save(&path)?;
        println!("soqrates: using {parsed} for repositories on {host}.");
        return Ok(ExitCode::SUCCESS);
    }
    let settings = settings(cli, local::repo_root(vcs).as_deref())?;
    let app = build_app(&settings)?;
    let author = local_author(vcs, &settings)?;
    let err = |e: soqrates_core::Error| e.to_string();
    match command {
        IdentityCommand::Use { .. } => unreachable!(),
        IdentityCommand::List => {
            for identity in app.identities(&author).await.map_err(err)? {
                println!("{identity}");
            }
        }
        IdentityCommand::RequestLink => {
            let code = app
                .request_link_code(&author, link_code()?)
                .await
                .map_err(err)?;
            println!(
                "Link code for {}: {}\nPresent it from your other identity within 15 minutes:\n  soqrates identity link {}",
                author.0,
                code.code(),
                code.code()
            );
        }
        IdentityCommand::Link { code } => {
            app.present_link_code(&author, code).await.map_err(err)?;
            println!("soqrates: linked {}.", author.0);
        }
        IdentityCommand::Unlink => {
            app.unlink(&author).await.map_err(err)?;
            println!("soqrates: unlinked {}.", author.0);
        }
        IdentityCommand::Delete { yes } => {
            if !yes {
                return Err("this deletes your learner profile and links; pass --yes".into());
            }
            app.delete_learner(&author).await.map_err(err)?;
            println!("soqrates: deleted.");
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// The installed git hook. It fails open: without `soqrates` on `PATH` (as
/// in many IDE commits) it allows the commit (ADR-0001 D9).
const HOOK_SCRIPT: &str = "#!/bin/sh\n\
command -v soqrates >/dev/null 2>&1 || exit 0\n\
exec soqrates hook pre-commit\n";

fn install_hook(force: bool) -> Result<ExitCode, String> {
    let path = local::git_hook_path("pre-commit")?;
    if path.exists() && !force {
        return Err(format!(
            "{} exists; pass --force to replace it",
            path.display()
        ));
    }
    std::fs::create_dir_all(path.parent().expect("has parent")).map_err(|e| e.to_string())?;
    std::fs::write(&path, HOOK_SCRIPT).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;
    }
    println!("soqrates: installed {}", path.display());
    Ok(ExitCode::SUCCESS)
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match &cli.command {
        Command::Hook { kind } => match run_hook(&cli, *kind).await {
            // A hook fails open: a Soqrates problem never blocks a commit.
            Err(e) => {
                eprintln!("soqrates: {e} (allowing the commit)");
                Ok(ExitCode::SUCCESS)
            }
            ok => ok,
        },
        Command::Ci { repo, pr } => run_ci(&cli, repo, *pr).await,
        Command::Mcp => match settings(&cli, None).and_then(|s| build_app(&s)) {
            Ok(app) => soqrates_mcp::SoqratesMcp::new(Arc::new(app))
                .serve_stdio()
                .await
                .map(|()| ExitCode::SUCCESS)
                .map_err(|e| e.to_string()),
            Err(e) => Err(e),
        },
        Command::Identity { command } => run_identity(&cli, command).await,
        Command::InstallHook { force } => install_hook(*force),
    };
    result.unwrap_or_else(|e| {
        eprintln!("soqrates: {e}");
        ExitCode::FAILURE
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_parses() {
        Cli::try_parse_from(["soqrates", "hook", "pre-commit"]).unwrap();
        Cli::try_parse_from(["soqrates", "ci", "--repo", "o/r", "--pr", "3"]).unwrap();
        Cli::try_parse_from(["soqrates", "--mode", "blocking", "mcp"]).unwrap();
        Cli::try_parse_from(["soqrates", "identity", "use", "github.com:1001"]).unwrap();
    }

    #[test]
    fn hook_script_fails_open_without_soqrates() {
        let out = std::process::Command::new("/bin/sh")
            .args(["-c", HOOK_SCRIPT])
            .env("PATH", "/nonexistent")
            .status()
            .unwrap();
        assert!(out.success());
    }

    #[test]
    fn link_codes_are_long_and_varied() {
        let a = link_code().unwrap();
        let b = link_code().unwrap();
        assert_eq!(a.len(), 19);
        assert_ne!(a, b);
    }
}
