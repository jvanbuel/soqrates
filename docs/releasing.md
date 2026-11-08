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

The very first run released the current version, `v0.1.0`, straight away:
with no `vX.Y.Z` tag yet, release-plz treats the version in `Cargo.toml` as
unreleased. Every later release goes through a release PR.

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

Open the release PR as the Soqrates GitHub App, so CI runs on it:

1. Give the app (see [github-app.md](github-app.md)) **Contents** and **Pull
   requests** read and write, and accept the new permissions on its
   installation.
2. Set the repository variable `SOQRATES_APP_ID` and the secret
   `SOQRATES_APP_PRIVATE_KEY`, the same ones the Soqrates check uses to post
   as soqrates[bot].

Without the app, release-plz falls back to the `RELEASE_PLZ_TOKEN` secret (a
fine-grained token with Contents and Pull requests read and write), and then
to the workflow token. The workflow token needs **Settings → Actions →
General → Workflow permissions → Allow GitHub Actions to create and approve
pull requests**, and a PR it opens triggers no other workflows, so CI doesn't
run on the release PR.

The Soqrates check skips release PRs (branches starting with `release-plz-`)
either way.

## Dry run, and rebuilding a release

Run the **release** workflow by hand (Actions → release → Run workflow):

- with the **tag** left empty, it builds and packages the three binaries and
  publishes nothing;
- to rebuild an existing release whose build failed, choose its tag under
  **Use workflow from** (e.g. `v0.2.0`) and enter the same tag as input. It
  builds that tag's binaries and attaches them to the release, so the build
  and its provenance come from the tagged commit. A rebuild run from any
  other ref is refused. The major tag only moves if this is the newest
  release of that major version.

  This works for releases tagged after the rebuild input was added. For an
  older release, cut a new patch release instead.
