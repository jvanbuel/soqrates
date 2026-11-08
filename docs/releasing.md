# Releasing Soqrates

Releases are made with [release-plz](https://release-plz.dev) (ADR-0006 D5),
configured in `release-plz.toml` and run by `.github/workflows/release-plz.yml`.

## How a release happens

1. **Every push to `main`** opens or updates a release PR, labelled
   `release`. It bumps the workspace version in `Cargo.toml` (every crate
   inherits it) and adds the new section to `CHANGELOG.md`, from the commits
   since the last `vX.Y.Z` tag.
2. **Merge the release PR** when you want to release. The push to `main`
   then:
   - tags `vX.Y.Z` and creates the GitHub release, with the changelog
     section as its notes;
   - runs `release.yml`, which builds `soqrates` for Linux x64 and arm64 and
     macOS arm64, attests where each archive was built, attaches the
     archives and their SHA-256 checksums to the release, and moves the
     major tag (`v0`) to it.
3. Users of `jvanbuel/soqrates@v0` get the new release on their next run.

Nothing is published to crates.io. release-plz reads the previous version
from the `vX.Y.Z` tags, and only `soqrates-cli` gets a tag and a changelog.

The first release PR proposes `v0.1.0`, the current version, since there is
no tag yet.

## Version bumps and the changelog

release-plz picks the next version from the commit messages, following
[Conventional Commits](https://www.conventionalcommits.org):

| Commit message | Before 1.0 | From 1.0 |
|---|---|---|
| `fix: ...`, or no prefix | patch | patch |
| `feat: ...` | patch | minor |
| `feat!: ...` or `BREAKING CHANGE:` in the body | minor | major |

You can also edit the version and changelog in the release PR before merging.

## One-time setup

- **Settings → Actions → General → Workflow permissions**: tick **Allow
  GitHub Actions to create and approve pull requests**, so release-plz can
  open the release PR.
- **Optional: `RELEASE_PLZ_TOKEN` secret.** A PR opened with the workflow
  token doesn't trigger other workflows, so CI doesn't run on the release PR.
  To have it run, add a fine-grained personal access token for this
  repository with **Contents** and **Pull requests** read and write as the
  repository secret `RELEASE_PLZ_TOKEN`; release-plz then opens the PR with
  it. The Soqrates check skips pull requests by bots either way.

## Dry run

Run the **release** workflow by hand (Actions → release → Run workflow): it
builds and packages the three binaries and publishes nothing.
