# Mapping changes

## Read the affected scope

Use `get_mapping_settings` with the relevant preset and table when known. Read the returned settings revision, configuration mode, persistence behavior, active preset, enable flags, and any omission indicators. Broaden the read only if the decision needs omitted context.

Tables are `remote` and `local`. Rule indexes are zero-based within the chosen preset/table and include disabled rules. For a move, `to` is the final index. Use the current returned ordering when selecting an index.

Preserve unrelated rules and flags. Creating a preset does not activate it, and activating it does not enable every mapping gate. Map-local is evaluated after map-remote, so inspect the effective remote URL when reasoning about a local match. If the requested behavior needs additional activation or enable changes, include only those required by the user's intent.

## Preview and commit

1. Choose the typed mutation matching the request, and inspect its schema.
2. Call `preview_mapping_mutation` with the selected instance, `expected_settings_revision`, the exact proposed operation, and representative URLs. Include an affected URL and a nearby URL that should remain unaffected when scope is significant.
3. Inspect the predicted mapping, validation findings, and gate state. Preview changes no settings, performs no file reads or network requests, and reserves no revision.
4. Commit with the corresponding typed tool, using the same operation fields and evaluated `expected_settings_revision`. For example, an operation whose `kind` is `create_mapping_rule` is committed through `create_mapping_rule`, with the operation's payload fields and no `kind` wrapper.
5. Inspect the actual mutation outcome, persistence result, and returned revision. Verify the affected settings or URL explanation when the returned result does not establish the requested behavior.

For several mutations, each call is separate. Use the revision returned by the preceding successful call and preview the next operation against that revision. If a later step fails, report which earlier changes took effect; do not claim the whole sequence rolled back.

Use `explain_mapping` to inspect a particular URL against current or proposed settings. `validate_mapping_settings` accepts a complete proposed proxy configuration; use it when validating that complete configuration is useful, rather than reconstructing the whole configuration for a small typed edit.

## Handle uncertainty before another write

- **Revision conflict:** re-read the affected scope and reconcile the user's intended change with the new state. Re-preview the revised operation. Ask only if concurrent changes leave the desired outcome ambiguous.
- **Unfinished TUI draft, validation, persistence, or generation error:** inspect the returned error and current state. Resolve the stated condition before another mutation; preserve any reported partial outcome.
- **Timeout or lost acknowledgment:** the write may have happened. Re-read the same run and affected settings before deciding whether another call is needed. Never replay a mutation merely because its response was lost.
- **Replaced run:** report the identity change and establish the intended target again before writing.

Use the MCP mutation interface for live changes so revision checks and persistence handling remain in effect. Editing the backing YAML directly is not a substitute for a failed mutation.

## Verify the result at the right level

For map-local, separately verify that the chosen file exists and is readable when local filesystem access is available. A successful preview does not check the file. When a mapping must survive restarts, use an appropriately durable fixture path; persisting a rule does not persist the response file.

Respect the returned persistence mode: settings can change for the running process even when its source file is read-only or its configuration is ephemeral. State whether the outcome is persistent or session-only.

Report the changed preset/rule/flags and resulting revision, then distinguish what was verified: predicted mapping, local file readiness, observed proxy traffic, and application behavior. Trigger new traffic only when the user's task authorizes it; a mapping-only request can finish after its settings and prediction are verified, with live behavior explicitly untested.
