#!/usr/bin/env bash
# Commits and pushes the learner state, if it changed (ADR-0006 D3).
#
# The state branch has a single writer: the caller's workflow runs one job at
# a time (`concurrency` with `cancel-in-progress: false`). If something else
# pushed the branch meanwhile, the push is rejected and the step fails rather
# than overwrite that state; the next run starts from the pushed state.
#
# Env: STATE_DIR, STATE_BRANCH, STATE_REMOTE, GITHUB_TOKEN_FOR_STATE,
# GITHUB_EVENT_NAME.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=action/git-auth.sh
. "$here/git-auth.sh"

cd "$STATE_DIR"
git add -A
if git diff --cached --quiet; then
  exit 0
fi
git -c user.name="soqrates[bot]" -c user.email="soqrates@users.noreply.github.com" \
  commit -q -m "soqrates: update learner state (${GITHUB_EVENT_NAME:-manual})"
git_auth push -q origin "HEAD:refs/heads/${STATE_BRANCH}"
