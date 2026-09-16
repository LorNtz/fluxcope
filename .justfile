set positional-arguments

# Show available commands.
default:
    @just --list

# Build the release executable.
build:
    cargo build --release --locked

# Run the locally built application.
run:
    ./target/release/fluxcope

# Build and run the application.
rerun: build run

# Print HTTP proxy variables for evaluation in your current shell.
proxy port='7897':
    @python3 -c 'import sys; p=int(sys.argv[1]); assert 1 <= p <= 65535; print(f"export https_proxy=http://127.0.0.1:{p} http_proxy=http://127.0.0.1:{p}")' "$1"

# Diagnose local release tools, GitHub login and repository setup.
doctor:
    @python3 scripts/release.py doctor

# Push committed feature changes and create or reuse a feature PR.
pr:
    @python3 scripts/release.py pr

# Review a feature PR, then review and follow its release.
ship:
    @python3 scripts/release.py ship

# Review, merge and follow the existing remote release PR.
release:
    @python3 scripts/release.py release

# Prepare a release PR; optionally specify X.Y.Z or X.Y.Z-rc.N.
release-prepare version='':
    @python3 scripts/release.py prepare "$1"

# Follow the unified release result; optionally select its version.
release-status version='':
    @python3 scripts/release.py status "$1"

# Inspect and resume an already authorized release.
release-recover version:
    @python3 scripts/release.py recover "$1"

# One-time first crate upload; requires an exact merged release PR and hidden token input.
release-bootstrap version pr:
    @python3 scripts/bootstrap_publish.py "$1" "$2"

# Explicitly permit a reviewed replacement for an incomplete version; never marks it successful.
release-replace version:
    @python3 scripts/release.py replace "$1"

# Push/create a draft PR and publish an isolated branch preview (default: macOS ARM64).
preview *args:
    @python3 scripts/preview.py publish "$@"

# Follow a branch preview; omit the ID to select one for this PR.
preview-status id='':
    @python3 scripts/preview.py status "$1"

# Resume the original failed preview jobs without selecting a newer source.
preview-retry id:
    @python3 scripts/preview.py retry "$1"

# Verify and run a preview with its own settings, certificates and proxy port.
preview-run id='':
    @python3 scripts/preview.py run "$1"
