# ADR-0006: Distributing Soqrates as a GitHub Action

- **Status:** Accepted
- **Date:** 2026-11-07
- **Refines:** ADR-0001 D8 (CI round trip), D14 (dogfooding)

## Context

Soqrates only runs on its own repository. The dogfooding workflow checks out
the repository and builds `soqrates` from source. In any other repository,
that step would build the wrong code. There is no reusable action, and no
release binary.

The bot identity has the same limit. A GitHub App's private key belongs to
whoever registered the app. Other organisations can't post as this
project's soqrates[bot] from their own Actions runs; each would have to
register an app of its own (`docs/github-app.md`).

There are two ways to offer Soqrates to other teams:

- **A GitHub Action that users run themselves.** It needs no
  infrastructure from this project. Each user brings their own model key and
  keeps their own state. The PR diff still goes to the model provider they
  choose (Anthropic by default, OpenAI, or a self-hosted Ollama), and learner
  data lives where they put it: the state branch of their repository, or
  their own database. This project never sees either.
- **A hosted public GitHub App.** Installation is one click, and every
  repository gets the same soqrates[bot]. It needs a webhook server and a
  multi-tenant store. The project pays for the LLM calls and holds
  customers' private diffs and learner data, which calls for a privacy
  policy and an operations commitment.

The action is far cheaper, and teams with private code are more likely to
adopt it, because they choose where their diffs and learner data go. It also prepares the hosted app: a release pipeline, a stable
CLI, and a database store (ADR-0005).

## Decisions

### D1: Soqrates ships as a reusable action, `jvanbuel/soqrates@vX`
**Enforcement:** automated

The repository root holds a composite action, `action.yml`. It runs
`soqrates ci` for the pull request of the triggering event. Users keep
their own workflow file, with the triggers and the job-level `if:` filter
for comment edits, since an action cannot declare its own triggers. The
guide `docs/using-the-action.md` gives that file in full, to copy as is.
The action never checks out
the user's code: Soqrates reads the diff through the API, so no code from a
pull request runs with the workflow's secrets. It fetches the state branch
into a directory of its own, so it needs no checkout either.

### D2: The action runs a prebuilt, checksummed release binary
**Enforcement:** automated

Each version tag (`v0.2.0`) publishes `soqrates` archives for Linux x86_64
and aarch64 (built on Ubuntu 22.04, for older glibc) and macOS arm64 on
GitHub Releases, each with a SHA-256 checksum and a build provenance
attestation. The action reads its own version from the `Cargo.toml` of its
checkout and downloads the matching archive. Before running it, it checks
the checksum (integrity) and the attestation (built by this repository's
release workflow). It never compiles. The same archives serve local hooks.

The `binary` input runs a given binary instead. It exists for testing the
action from source (D5), not for users.

### D3: Users bring their own model key and state
**Enforcement:** automated

The action takes:

- `model`, the model spec, which only selects the provider and model
  (default `anthropic:claude-sonnet-5-5`, or e.g. `openai:gpt-5.2`,
  `ollama:qwen3`);
- the credentials of that provider, as separate inputs passed from secrets:
  `anthropic-api-key` or `openai-api-key`, or for Ollama an optional
  `ollama-base-url` and `ollama-api-key`. The action exports them under the
  names the examiner reads (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`,
  `OLLAMA_API_BASE_URL`, `OLLAMA_API_KEY`). The model spec never holds a
  credential;
- the mode (`advisory` by default);
- where state lives: the `soqrates-state` branch of the repository by
  default, or `database-url` for a database (ADR-0005), passed from a
  secret.

State must outlive the job: answering the questions runs a later job,
which has to find the pending session. The state branch does. A
`database-url` must therefore name a database that persists between jobs,
such as a hosted PostgreSQL. A SQLite URL only works on a self-hosted runner
whose database file stays on disk between jobs. On GitHub-hosted runners the
file is gone after the job, so the action refuses a `sqlite:` URL there
(`RUNNER_ENVIRONMENT` is `github-hosted`), and says why.

Without the credentials of the chosen provider the action skips the check
with a notice instead of failing, as on pull requests from forks, which get
no secrets.

Wherever state lives, the action and its guide say what it holds and who
can read it. Anyone who can read the repository can read the state branch,
including what each author got wrong.

The action fetches and pushes the state branch with the workflow token, so
the job needs `contents: write`.

### D4: Comments post as github-actions[bot], or as the user's own app
**Enforcement:** automated

By default the action posts with the workflow token. With the optional
inputs `app-id` and `app-private-key`, it mints a token for the user's own
GitHub App first, as the dogfooding workflow does today. That token then
posts as that app, under its name and with its logo. A shared soqrates[bot]
across organisations needs the hosted app (D6).

### D5: Versions follow semver, with a moving major tag
**Enforcement:** automated

Releases are made with [release-plz](https://release-plz.dev). On every
push to `main` it keeps a release PR open that bumps the workspace version
and updates `CHANGELOG.md` from the commits. Merging that PR tags `vX.Y.Z`,
matching the crate version, and creates the GitHub release; the release
workflow then builds and attaches the binaries and moves `vX` to the latest
release of that major version. Until 1.0.0 that tag is `v0`. Soqrates isn't
published to crates.io: release-plz reads the previous version from the git
tags, and only `soqrates-cli`, the binary users run, gets a tag and a
changelog. Breaking changes to inputs, to the comment format, or to the
state's layout (the file store's documents, the database schema) need a new
major version. This repository's own workflow uses the action from its
checkout (`uses: ./`), with a binary built from the same source, so every
pull request tests the action's steps as users will run them.

### D6: The hosted public app waits for demand
**Enforcement:** manual. A product decision.

A hosted soqrates[bot] is reconsidered when teams ask for one-click
installation or a shared bot identity. It would also make the learner store
of ADR-0003 real: one profile that follows a developer across repositories
and organisations. It would reuse the core, the GitHub adapter and the
database store, and add a webhook server that calls `run_check` with
per-installation tokens.

## Consequences

- A release workflow and binary builds join CI; their cost is paid once per
  release, not on every pull request. release-plz derives the next version
  from the commit messages, so a `feat:` or `fix:` prefix (Conventional
  Commits) gives better version bumps and changelog entries than a plain
  sentence, which counts as a patch.
- The CLI's `ci` command, its flags and environment variables become a
  public interface, covered by D5.
- `docs/github-app.md` becomes a guide for users' own apps, not only for
  this repository.
- Until D6, each organisation sees its own bot name. Learner profiles stay
  per repository, or per database when several repositories share one.
