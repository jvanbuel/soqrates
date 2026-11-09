# Using Soqrates in your repository

Soqrates runs as a GitHub Action (ADR-0006). On every pull request it asks the
author a few questions about the concepts in their change, as a comment. The
author answers by ticking boxes or with a `/soqrates answer` comment, and gets
the results in a new comment. `/socrates answer`, any letter case, and, when
Soqrates posts as your own app, `@<app slug> answer` work too. A mistyped
command gets a hint, and an answer comment can be edited until it is graded.
While the answers are graded, the comment that submitted them shows 👀, then
👍 once the results are posted. The `soqrates` commit status reports the
outcome.

## 1. Add your model key

Repository **Settings** → **Secrets and variables** → **Actions** → **New
repository secret**: `ANTHROPIC_API_KEY`, with an Anthropic API key. For
another provider, see [Inputs](#inputs).

## 2. Add the workflow

Save this as `.github/workflows/soqrates.yml` on your **default branch**.
GitHub runs `issue_comment` workflows from the default branch, so answering
only works once the file is there.

```yaml
name: soqrates

on:
  pull_request:
    types: [opened, synchronize, reopened]
  issue_comment:
    types: [created, edited]

permissions:
  contents: write       # push the soqrates-state branch; check out the PR's code
  pull-requests: write  # read the pull request and its diff
  issues: write         # post the questions and results as comments
  statuses: write       # set the `soqrates` commit status

jobs:
  soqrates:
    # Run on pull requests by people (not Dependabot and other bots), on
    # `/soqrates ...` and `/socrates ...` comments (startsWith ignores case)
    # and comments starting with a mention that say "answer", also when the
    # PR author edits them, and when Soqrates' own question comment is
    # edited with "Grade my answers" ticked, so ticking an option doesn't
    # start a run.
    if: >-
      (github.event_name == 'pull_request' && github.event.pull_request.user.type != 'Bot') ||
      (github.event.issue.pull_request && (
        ((github.event.action == 'created' ||
            (github.event.action == 'edited' &&
              github.event.comment.user.id == github.event.issue.user.id)) &&
          (startsWith(github.event.comment.body, '/soqrates') ||
            startsWith(github.event.comment.body, '/socrates') ||
            (startsWith(github.event.comment.body, '@') &&
              contains(github.event.comment.body, ' answer')))) ||
        (github.event.action == 'edited' &&
          endsWith(github.event.comment.user.login, '[bot]') &&
          contains(github.event.comment.body, 'soqrates:session=') &&
          contains(github.event.comment.body, '- [x] **Grade my answers**'))
      ))
    # One run at a time, so updates to the learner state never race.
    concurrency:
      group: soqrates-state
      cancel-in-progress: false
    runs-on: ubuntu-latest
    steps:
      - uses: jvanbuel/soqrates@v0
        with:
          anthropic-api-key: ${{ secrets.ANTHROPIC_API_KEY }}
```

Keep the `concurrency` group: the state branch expects one writer at a time,
and a push that loses a race fails rather than overwrite the other run's
state.

That's all. The action needs no `actions/checkout`: it reads the diff through
the GitHub API, so no code from a pull request runs with your secrets.

`@v0` follows the latest 0.x release. Pin an exact release (`@v0.1.0`) or a
commit SHA if you prefer.

## What leaves your repository

- **The pull request diff** goes to the model provider you choose (Anthropic
  by default), which writes and grades the questions.
- **Code of the pull request** that the examiner chooses to read goes
  there too: files, grep results, and git log and blame. The action checks
  out the pull request's head into `.soqrates/code` for this, keeping no
  credentials in it, and never builds or runs anything from it. The examiner
  reads only what git holds at the commit asked about, at most 100 KB per
  examiner step. Set `read-code: false` to send only the diff
  ([ADR-0008](adr/0008-examiner-reads-the-codebase.md)).
- **Learner state** is kept on the `soqrates-state` branch of your
  repository: one JSON document per learner profile, identity link and
  pending session. **Anyone who can read the repository can read that
  branch**, including what each author got wrong. Keep it in a database
  instead with `database-url` (below).
- Nothing goes to the Soqrates project.

## Inputs

| Input | Default | |
|---|---|---|
| `model` | `anthropic:claude-sonnet-5-5` | `provider:model`: `anthropic:…`, `openai:…` or `ollama:…`. Selects the model only; credentials are separate inputs. |
| `anthropic-api-key` | | For `anthropic:` models. From a secret. |
| `openai-api-key` | | For `openai:` models. From a secret. |
| `ollama-base-url` | | Your Ollama server, for `ollama:` models. |
| `ollama-api-key` | | If your Ollama server needs one. From a secret. |
| `mode` | `advisory` | `advisory`: a failed session reports success with a note, so it never blocks a merge. `blocking`: a failed session fails the check. |
| `database-url` | | Keep learner state in a database instead of the state branch, e.g. `postgres://…`. From a secret. It must persist between jobs, so `sqlite:` URLs are refused on GitHub-hosted runners. |
| `state-branch` | `soqrates-state` | The branch holding learner state when no `database-url` is given. |
| `read-code` | `true` | Let the examiner read the pull request's code, from a checkout of its head that is only read. `false` to send only the diff. |
| `code-history` | `100` | Commits of history the checkout fetches, for git log and blame. `0` for all. |
| `github-token` | `${{ github.token }}` | Reads the pull request and compares its pushes, posts comments and the status, and pushes the state branch. |
| `app-id`, `app-private-key` | | Post as your own GitHub App instead of github-actions[bot]; see [Posting as your own app](#posting-as-your-own-app). |
| `app-slug` | the app's slug | Authors may answer with `@<app-slug> answer`. Taken from the app token when `app-id` is set; set it only to override that. |

Questions are multiple choice. To give experts open questions instead, set
`SOQRATES_OPEN_QUESTIONS: "true"` in the job's `env` (ADR-0002 D5).

Without the key of the chosen provider, for example on pull requests from
forks, which get no secrets, the action skips the check with a notice.

For example, with OpenAI and state in PostgreSQL:

```yaml
      - uses: jvanbuel/soqrates@v0
        with:
          model: openai:gpt-5.2
          openai-api-key: ${{ secrets.OPENAI_API_KEY }}
          database-url: ${{ secrets.SOQRATES_DATABASE_URL }}
```

## Posting as your own app

By default comments and the status appear as **github-actions[bot]**. To post
under your own name and logo, create a GitHub App as described in
[github-app.md](github-app.md), then pass its credentials:

```yaml
      - uses: jvanbuel/soqrates@v0
        with:
          anthropic-api-key: ${{ secrets.ANTHROPIC_API_KEY }}
          app-id: ${{ vars.SOQRATES_APP_ID }}
          app-private-key: ${{ secrets.SOQRATES_APP_PRIVATE_KEY }}
```

Authors can then also answer with `@<your app's slug> answer`, which GitHub
autocompletes. Soqrates never accepts a bare `@soqrates`: a person of that
name would be notified of every answer, while an app is not notified when it
is mentioned. With github-actions[bot], use `/soqrates answer`.

## Supported runners

Linux x64 and arm64 (glibc 2.35 or newer, e.g. `ubuntu-22.04` and later), and
macOS arm64. The action downloads the `soqrates` release matching its own
version and checks its SHA-256 checksum and its build provenance attestation
before running it.
