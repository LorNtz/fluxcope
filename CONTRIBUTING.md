# Contributing to Fluxcope

Use the toolchain pinned in `rust-toolchain.toml`. Start the app with `just run`; run `just` to discover the available commands.

Before submitting Rust changes:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --all-features --locked
```

The binary smoke harness uses a temporary HOME, local HTTP fixtures, a client-specific CA trust context and a PTY. It never installs a CA into your system trust store:

```sh
cargo build --locked
python3 scripts/smoke.py --binary target/debug/fluxcope --version 0.1.0
```

Use a feature branch and a Conventional Commit PR title such as `feat: add request mapping` or `fix: preserve response trailers`. Commit the intended changes, then use `just pr` to push and create or reuse the PR. `just ship` also waits for CI and asks separately before merging the feature and publishing its release. See [the release runbook](docs/releasing.md).

Do not commit real captured traffic, credentials, cookies, private keys, local configuration, debug logs or personal work records. Use reserved example domains and synthetic payloads in fixtures. `requirements.md` and `what_i_just_did.md` remain local-only; `agent_journal.md` is tracked and must contain no sensitive material.

Keep changes focused. HTTP body inspection must preserve bytes, trailers, errors, streaming and backpressure; capturing a preview must not truncate forwarded traffic. Add regression coverage when changing these contracts.
