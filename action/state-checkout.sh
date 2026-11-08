#!/usr/bin/env bash
# Fetches the learner-state branch into a fresh directory, or starts it empty,
# and prints the directory to $GITHUB_OUTPUT (ADR-0006 D3). Needs no checkout
# of the user's repository.
#
# Env: STATE_BRANCH, STATE_REMOTE (default: this repository on this server),
# GITHUB_TOKEN_FOR_STATE, RUNNER_TEMP, GITHUB_OUTPUT.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=action/git-auth.sh
. "$here/git-auth.sh"

dir="${RUNNER_TEMP}/soqrates-state"
rm -rf "$dir"
git init -q "$dir"
cd "$dir"
git remote add origin "$(state_remote)"

# Tell "no state branch yet" (exit 2) apart from an error, so a failed
# fetch never starts the state over.
set +e
git_auth ls-remote --exit-code --heads origin "refs/heads/${STATE_BRANCH}" > /dev/null
found=$?
set -e
case "$found" in
  0)
    git_auth fetch -q --depth=1 origin "refs/heads/${STATE_BRANCH}"
    git checkout -q --detach FETCH_HEAD
    ;;
  2)
    echo "::notice::No ${STATE_BRANCH} branch yet; starting with empty learner state."
    git checkout -q --orphan "$STATE_BRANCH"
    ;;
  *)
    echo "::error::cannot read the ${STATE_BRANCH} branch (git exit $found)"
    exit 1
    ;;
esac
echo "dir=$dir" >> "$GITHUB_OUTPUT"
