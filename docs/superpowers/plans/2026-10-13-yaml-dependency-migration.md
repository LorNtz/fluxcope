# YAML Dependency Migration Implementation Plan

> **REQUIRED SUB-SKILL:** Use the executing-plans skill to implement this plan task-by-task.

**Goal:** Replace the unmaintained YAML dependency, merge the migration independently, then update PR #39 until it is ready to merge without merging #39.

**Architecture:** Replace `serde_yaml` with `yaml_serde` 0.10.7 and rename all Rust callers directly. Preserve the existing ordered YAML value model, tolerant configuration loading, explicit-default preservation, atomic persistence and transaction boundaries. No alias for the old crate, new serialization abstraction, advisory exception or configuration format change.

**Tech Stack:** Rust, Serde, `yaml_serde` with its default `std` feature, `libyaml-rs`, existing Cargo/cargo-deny and real-process verification.

---

## Decision and compatibility evidence

The user selected the maintained compatible fork after reviewing the alternative of requiring a safe-Rust parser rewrite. `yaml_serde` is maintained by the YAML organization and depends on `libyaml-rs`, a maintained fork of the existing unsafe-Rust parser. This resolves the maintenance dependency path; it does not claim to eliminate unsafe Rust.

- Primary manifest: https://github.com/yaml/yaml-serde/blob/main/Cargo.toml
- Parser lineage: https://github.com/yaml/libyaml-rs
- Original advisory: https://rustsec.org/advisories/RUSTSEC-2024-0449

An external differential probe compared the old and proposed libraries across 19 cases: ordering, Unicode/quoted strings, booleans, numeric boundaries/floats, tags, non-string keys, aliases/literal merge keys, duplicate keys, multiple/empty/malformed documents and typed settings conversion. `yaml_serde` matched all cases. Both serializers reject mapping-valued keys with the same error; this is baseline behavior, not a new regression. `noyalib` was rejected for this migration because it changed scalar key types and rejected complex keys accepted by the existing value model.

Baseline on master `081b3de`: formatting, all 944 tests, all-feature build and the existing real proxy/PTY smoke passed. The known unmaintained-dependency gate was not rerun merely to reproduce its existing failure.

## Task 1: Protect configuration compatibility

**TDD scenario:** Modifying tested code — baseline passed; run the additional behavior regressions against the old dependency before replacement.

**Files:**
- Create `src/settings/tests/yaml_compatibility.rs`.
- Modify `src/settings/tests/mod.rs` only to register the module.

Add isolated file-backed tests through `SettingsManager::load_from_path` and `set_server_port`:
1. Numeric, boolean, null and sequence keys in unknown configuration metadata retain their types, values and order while a known setting changes.
2. Custom scalar/mapping tags survive a settings save and reload.
3. Aliases resolve but literal `<<` entries do not start changing effective settings; explicit known-field edits remain authoritative.

Reuse the existing temporary-path and YAML inspection helpers. Assert consumer-visible saved/reloaded values, not implementation structure or error wording. Run:

```sh
cargo test --locked --all-features --lib settings::tests::
```

Expected: all existing and added settings cases pass on the old implementation.

## Task 2: Complete the dependency cutover

**TDD scenario:** Modifying tested code with preserved behavior.

**Files:**
- `Cargo.toml`, `Cargo.lock`
- `src/settings.rs`
- `src/settings/tests/{mod,basic,proxy,yaml_compatibility}.rs`
- `src/app/tests/settings_proxy.rs`
- `src/runtime/settings/test_support.rs`

Replace `serde_yaml = "0.9"` with `yaml_serde = "0.10.7"`. Use a syntax-aware identifier replacement for `serde_yaml` -> `yaml_serde` in the affected Rust files. Do not modify application algorithms or introduce package aliases. Refresh the lockfile without unrelated dependency upgrades.

Verify no production/dev/benchmark dependency on `serde_yaml` or `unsafe-libyaml` remains. Run focused settings and transaction tests plus the full Cargo gate after the coherent cutover. Install/use the repository-pinned cargo-deny tool if it is not already available; leave advisory policy unchanged.

## Task 3: Review and prove the migration

**Files:** Existing `CHANGELOG.md`; other consumer documentation only if made inaccurate.

Obtain separate read-only design/maintainability and performance reviews. Reviewers do not run builds, tests, linters or formatters; the integration owner runs checks after all applicable followups.

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
cargo build --locked
cargo build --locked --all-features
cargo build --locked --release --all-features
cargo bench --locked --features benchmark --bench core_paths -- --test
cargo-deny --locked check advisories licenses sources
python3 scripts/metadata_check.py
python3 scripts/smoke.py --binary "$CARGO_TARGET_DIR/debug/fluxcope" --version 0.2.1
python3 scripts/smoke.py --binary "$CARGO_TARGET_DIR/release/fluxcope" --version 0.2.1
```

In an isolated HOME, exercise actual TUI/MCP settings save and restart, explicit defaults and nested key ordering, malformed-prefilter recovery, Unicode mapping/preset values, and persistent/read-only/temporary modes. Use disposable fixture traffic and preserve user configuration/clipboard state. Benchmark test mode is a harness check, not a performance comparison.

After smoke proof, document the maintained YAML backend in the existing changelog without claiming an unsafe-free parser or unsupported performance improvement. Keep the local journal out of commits.

## Task 4: Publish and merge the migration

Read repository-local author/committer identity and verify Git's effective identity. Commit only the intended migration, tests, plan and changelog. Push without force and open a separate PR against current master. Verify the published file list and every required check, including advisory quality, Linux/macOS tests, package and release preview. Use the local author email for the authorized squash merge; do not bypass branch protection.

If a real compatibility or security tradeoff appears, ask the user before reducing behavior or changing risk policy. Otherwise resolve relevant findings and continue.

## Task 5: Unblock PR #39

Fetch the merged master into the existing `refactor/yagni-cleanup` worktree and merge it without force-pushing. Resolve overlap by keeping both the YAML replacement and all seven refactor intents. Do not reintroduce the old dependency, stale crate paths or compatibility aliases.

Run the integrated Cargo/advisory gates, benchmark harness and actual proxy/settings persistence smoke. Obtain the required Rust reviews for any non-mechanical resolution. Update #39's verification/blocker description to reflect observed results. Push normally, wait for required CI, and verify an open, non-draft, conflict-free, up-to-date PR with passing required checks. Leave #39 unmerged.
