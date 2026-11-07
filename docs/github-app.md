# Posting as soqrates[bot]

By default the dogfooding workflow posts with the workflow's built-in token,
so comments and the `soqrates` status appear as **github-actions[bot]**. To
post as **soqrates[bot]** with the Soqrates logo, the way review bots like
CodeRabbit do, register a GitHub App once.

## 1. Create the app

GitHub → your avatar → **Settings** → **Developer settings** →
**GitHub Apps** → **New GitHub App** (or, for an organisation:
organisation **Settings** → **Developer settings** → **GitHub Apps**).

| Field | Value |
|---|---|
| GitHub App name | `soqrates` (it must be unique on GitHub; if taken, e.g. `soqrates-bot`, and the bot is named after it) |
| Homepage URL | `https://github.com/jvanbuel/soqrates` |
| Webhook | untick **Active** (the workflow drives everything) |
| Repository permissions | **Commit statuses**: Read and write · **Issues**: Read and write · **Pull requests**: Read and write · **Contents**: Read-only · **Metadata**: Read-only (default) |
| Where can this app be installed? | Only on this account |

Click **Create GitHub App**. Then, on the app's page:

- **Display information** → upload `assets/logo/soqrates-avatar.png` as
  the logo, and pick `#FBF7F0` as the badge background colour.
- **Private keys** → **Generate a private key**. A `.pem` file downloads.
- Note the **App ID** near the top of the page.

## 2. Install it on the repository

On the app's page → **Install App** → your account → **Only select
repositories** → `soqrates` → **Install**.

## 3. Give the workflow the credentials

In the repository → **Settings** → **Secrets and variables** → **Actions**:

- **Variables** tab → new repository variable `SOQRATES_APP_ID` = the
  App ID.
- **Secrets** tab → new repository secret `SOQRATES_APP_PRIVATE_KEY` = the
  full contents of the `.pem` file, including the `BEGIN`/`END` lines.

The next Soqrates run posts as soqrates[bot]. Without these two settings the
workflow keeps working as github-actions[bot].

## Notes

- The token the workflow creates from the app lasts an hour and only has
  the permissions above, on the repositories the app is installed on.
- Pushing the learner state to the `soqrates-state` branch still uses the
  workflow's own token, so the app doesn't need write access to contents.
- Unlike the built-in token, edits made with an app token do trigger
  workflows. Soqrates' own edits (freezing the question comment after
  grading) are filtered out by the workflow's `if:` because the graded
  comment has no ticked "Grade my answers" box.
