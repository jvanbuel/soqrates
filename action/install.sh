#!/usr/bin/env bash
# Puts the soqrates binary for this action's version on disk and prints its
# path to $GITHUB_OUTPUT (ADR-0006 D2). Never compiles.
#
# Env: SOQRATES_BINARY (optional: run this binary instead, e.g. one built
# from source for testing the action), ACTION_PATH, ACTION_REPOSITORY,
# GH_TOKEN, RUNNER_OS, RUNNER_ARCH, RUNNER_TEMP, GITHUB_OUTPUT.
set -euo pipefail

if [ -n "${SOQRATES_BINARY:-}" ]; then
  if [ ! -x "$SOQRATES_BINARY" ]; then
    echo "::error::binary '$SOQRATES_BINARY' does not exist or is not executable"
    exit 1
  fi
  echo "path=$(cd "$(dirname "$SOQRATES_BINARY")" && pwd)/$(basename "$SOQRATES_BINARY")" >> "$GITHUB_OUTPUT"
  exit 0
fi

# The version is the one of the action's own checkout, so `@v0.2.0`, the
# moving `@v0` and a commit SHA all run the binary released from that source.
version=$(sed -n 's/^version = "\([^"]*\)"$/\1/p' "$ACTION_PATH/Cargo.toml" | head -n 1)
if [ -z "$version" ]; then
  echo "::error::no version in $ACTION_PATH/Cargo.toml"
  exit 1
fi

case "${RUNNER_OS}-${RUNNER_ARCH}" in
  Linux-X64) target=x86_64-unknown-linux-gnu ;;
  Linux-ARM64) target=aarch64-unknown-linux-gnu ;;
  macOS-ARM64) target=aarch64-apple-darwin ;;
  *)
    echo "::error::no soqrates release for ${RUNNER_OS}/${RUNNER_ARCH}; use a Linux (x64, arm64) or macOS (arm64) runner"
    exit 1
    ;;
esac

repo="${ACTION_REPOSITORY:-jvanbuel/soqrates}"
asset="soqrates-${target}.tar.gz"
dir="${RUNNER_TEMP}/soqrates-v${version}"
mkdir -p "$dir"
cd "$dir"

if ! gh release download "v${version}" --repo "$repo" --clobber \
  --pattern "$asset" --pattern "${asset}.sha256"; then
  echo "::error::cannot download $asset from release v${version} of $repo. Use the action at a released tag, e.g. jvanbuel/soqrates@v${version%%.*}."
  exit 1
fi

# Integrity: the archive matches the checksum published with it.
if command -v sha256sum > /dev/null; then
  sha256sum --check --strict "${asset}.sha256"
else
  shasum -a 256 --check "${asset}.sha256"
fi
# Provenance: the archive was built by this repository's release workflow.
gh attestation verify "$asset" --repo "$repo"

tar -xzf "$asset"
chmod +x soqrates
echo "path=${dir}/soqrates" >> "$GITHUB_OUTPUT"
