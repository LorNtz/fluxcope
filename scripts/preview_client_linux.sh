#!/usr/bin/env bash
# Pinned manylinux runtime; no publication credentials enter this container.
set -euo pipefail
export HOME=/tmp/preview-client-home
export PYINSTALLER_CONFIG_DIR=/tmp/preview-pyinstaller
mkdir -p "$HOME"
PYTHON=/opt/python/cp311-cp311/bin/python3
if [[ "$CLIENT_PHASE" == build ]]; then
  "$PYTHON" -m venv /tmp/preview-client-venv
  /tmp/preview-client-venv/bin/pip install --require-hashes -r "$CONTROLLER/scripts/preview-client-requirements.txt"
  /tmp/preview-client-venv/bin/python "$CONTROLLER/scripts/preview_client_build.py" --target "$TARGET" --directory "$OUTPUT"
else
  "$PYTHON" "$CONTROLLER/scripts/preview_install_check.py" --target "$TARGET" --directory "$OUTPUT" --report "$OUTPUT/$TARGET-install.json"
fi
