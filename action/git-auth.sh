# shellcheck shell=bash
# Sourced by the state scripts: git with the workflow token, never written to
# disk or to the remote URL.

state_remote() {
  echo "${STATE_REMOTE:-${GITHUB_SERVER_URL:-https://github.com}/${GITHUB_REPOSITORY}.git}"
}

git_auth() {
  if [ -z "${GITHUB_TOKEN_FOR_STATE:-}" ]; then
    git "$@"
    return
  fi
  local basic
  basic=$(printf 'x-access-token:%s' "$GITHUB_TOKEN_FOR_STATE" | base64 | tr -d '\n')
  echo "::add-mask::$basic"
  git -c "http.extraheader=AUTHORIZATION: basic $basic" "$@"
}
