# Posting as your own GitHub App

By default the Soqrates action posts with the workflow's built-in token, so
comments and the `soqrates` status appear as **github-actions[bot]**. To post
under your own bot name and logo, the way review bots like CodeRabbit do,
register a GitHub App once and give the action its credentials (ADR-0006 D4).
Each organisation registers its own app: a GitHub App's private key can't be
shared, so there is no common soqrates[bot] across organisations.

## 1. Create the app

GitHub → your avatar → **Settings** → **Developer settings** →
**GitHub Apps** → **New GitHub App** (or, for an organisation:
organisation **Settings** → **Developer settings** → **GitHub Apps**).

| Field | Value |
|---|---|
| GitHub App name | e.g. `<your-org>-soqrates` (it must be unique on GitHub; the bot is named after it) |
| Homepage URL | your organisation's page, or `https://github.com/jvanbuel/soqrates` |
| Webhook | untick **Active** (the workflow drives everything) |
| Repository permissions | **Commit statuses**: Read and write · **Issues**: Read and write · **Pull requests**: Read and write · **Contents**: Read-only · **Metadata**: Read-only (default). For this repository's own app, **Contents: Read and write**, so release-plz can open release PRs with it ([releasing.md](releasing.md)). |
| Where can this app be installed? | Only on this account |

Click **Create GitHub App**. Then, on the app's page:

- **Display information** → upload a logo, e.g. this repository's
  `assets/logo/soqrates-avatar.png` with `#FBF7F0` as the badge background
  colour.
- **Private keys** → **Generate a private key**. A `.pem` file downloads.
- Note the **App ID** near the top of the page.

## 2. Install it on the repository

On the app's page → **Install App** → your account or organisation → **Only
select repositories** → the repositories that run Soqrates → **Install**.

## 3. Give the workflow the credentials

In each repository (or once, for the organisation) → **Settings** →
**Secrets and variables** → **Actions**:

- **Variables** tab → new repository variable `SOQRATES_APP_ID` = the
  App ID.
- **Secrets** tab → new repository secret `SOQRATES_APP_PRIVATE_KEY` = the
  full contents of the `.pem` file, including the `BEGIN`/`END` lines.

Then pass them to the action in your workflow:

```yaml
      - uses: jvanbuel/soqrates@v0
        with:
          anthropic-api-key: ${{ secrets.ANTHROPIC_API_KEY }}
          app-id: ${{ vars.SOQRATES_APP_ID }}
          app-private-key: ${{ secrets.SOQRATES_APP_PRIVATE_KEY }}
```

The next run posts as your app. Without these two inputs the action keeps
posting as github-actions[bot]. This repository's own workflow does the same
with the Soqrates project's app.

## Notes

- The token the workflow creates from the app lasts an hour and only has
  the permissions above, on the repositories the app is installed on.
- Pushing the learner state to the `soqrates-state` branch still uses the
  workflow's own token (`github-token`), so the app doesn't need write
  access to contents.
- Unlike the built-in token, edits made with an app token do trigger
  workflows. Soqrates' own edits (freezing the question comment after
  grading) are filtered out by the workflow's `if:` because the graded
  comment has no ticked "Grade my answers" box.
