#!/usr/bin/env bash
# Native manylinux execution; the container receives no GitHub credentials.
set -euo pipefail
export PATH="/cargo-bin:$PREVIEW_TOOLS:$PATH"
export RUSTUP_HOME=/rustup
export CARGO_HOME="$PREVIEW_SOURCE/target/ci-cargo"
export GIT_OPTIONAL_LOCKS=0
mkdir -p "$CARGO_HOME" "$PREVIEW_SOURCE/target/ci-home"
env HOME="$PREVIEW_SOURCE/target/ci-home" /opt/python/cp311-cp311/bin/python3 \
  "$PREVIEW_CONTROLLER/scripts/preview_build.py" "$PREVIEW_PHASE" \
  --source "$PREVIEW_SOURCE" --identity "$PREVIEW_IDENTITY" \
  --target "$PREVIEW_TARGET" --directory "$PREVIEW_OUTPUT"
