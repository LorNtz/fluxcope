# Task 13 Report

## Changed files

- `src/settings/mapping_ops.rs`
- `src/settings.rs`
- `src/mapping.rs`
- `src/app/settings_popup.rs`
- `src/app/settings_popup/proxy.rs`
- `src/app/settings_popup/validation.rs`
- `src/app/tests/settings_proxy.rs`
- `.superpowers/sdd/2026-08-24-embedded-mcp-server/task-13-report.md`
- `.superpowers/sdd/2026-08-24-embedded-mcp-server/progress.md`
- `what_i_just_did.md`

`src/app/settings_popup/field_editor.rs` remains unchanged because its preset-name commit already delegates to the migrated proxy handler and contains no proxy draft mutation of its own.

## Domain API

The new crate-internal mapping domain exposes `MappingMutation`, `MutationEffect`, `MappingObjectRef`, `MappingMutationResult`, `MappingMutationError`, `apply_mapping_mutation`, `validate_mapping_candidate`, and `explain_mapping_candidate`. Mutations clone the candidate and replace the caller value only after a successful operation, so errors preserve the complete input. Only `CreatePreset` may create missing proxy settings, complete initial presets are renamed and validated before insertion, and creation does not activate a preset.

The mapping compiler now retains exact matched rule locations. Explanations return effective URL/local path, active gate state, matched preset/table/rule metadata, and typed diagnostics without opening local mapping targets.

## Test matrix

Child tests cover default and complete creation, name trimming and case-sensitive uniqueness, active rename/delete/null/stale recovery, global/remote/local gates, remote/local append/insert/update/delete/enable/move, same-index `Unchanged`, invalid proxy/preset/table/index/field atomicity, type separation, validation diagnostics and locations, invalid request URLs/rules, effective remote/local destinations, matched metadata, gates, and nonexistent local targets without reads.

Popup regressions cover remote/local toggle separation and atomic rejection/reporting of an invalid rule editor commit. Existing popup tests continue to cover selection, stale active recovery, rename hints, gate toggles, rule editing, focus, cursor, selection, and presentation behavior.

## Verification

- `cargo test settings::mapping_ops --all-features`: 15 passed.
- `cargo test mapping::tests --all-features`: 9 passed.
- `cargo test --all-targets --all-features`: 750 passed.
- `cargo fmt --all -- --check`: passed.
- `cargo clippy --all-targets --all-features -- -D warnings`: passed.

The initial delegated handoff contained both the tests and implementation, so Main could not observe the planned missing-API RED without discarding complete work. Review-driven regressions were observed failing before their production fixes: missing `original_url` and trimmed popup active-preset identity.

## Review

- Design/maintainability: PASS after canonicalizing trimmed preset identity across validation, runtime compilation, mutation lookup, and popup consumers, and adding the original request URL to explanations.
- Performance: PASS after restoring the decision-only live mapping path, the all-empty fast return, and the remote-only skip for effective request reconstruction.
- No material findings remain after final scoped re-review.
