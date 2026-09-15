# Fluxcope's only cargo-dist installer extension: require SHA-256 verification.
# macOS ships shasum, whereas cargo-dist 0.33.0 only probes sha256sum.
if ! command -v sha256sum >/dev/null 2>&1; then
    if command -v shasum >/dev/null 2>&1; then
        sha256sum() { shasum -a 256 "$@"; }
    else
        echo 'Fluxcope: SHA-256 verification requires sha256sum or shasum; installation stopped.' >&2
        exit 1
    fi
fi
