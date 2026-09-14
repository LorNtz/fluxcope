#!/usr/bin/env bash
# Called inside the pinned native manylinux 2.28 container, with no GitHub credentials.
set -euo pipefail
export PATH="/cargo-bin:/work/target/ci-tools:$PATH"
export CARGO_HOME=/work/target/ci-cargo
export RUSTUP_HOME=/rustup
FLUXCOPE_BUILD_HOME=/work/target/ci-home
mkdir -p "$CARGO_HOME" "$FLUXCOPE_BUILD_HOME"
env HOME="$FLUXCOPE_BUILD_HOME" /opt/python/cp311-cp311/bin/python3 scripts/dist_artifacts.py local --target "$1" --version "$2"
