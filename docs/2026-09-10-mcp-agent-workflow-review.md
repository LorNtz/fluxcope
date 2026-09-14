# Wirelens MCP and skill improvement notes

Date: 2026-09-10

Status: Discussion notes and proposed improvements; not an approved implementation specification.

## Context and outcome

These notes consolidate a review of an agent-assisted map-local task performed on 2026-09-09 and the follow-up discussion. The task was to derive a `pInTripLayout` mock from an existing mapped file, add a bubble to the route-selection toolbox button, and enable the corresponding Wirelens mapping.

The task succeeded: a new enabled local rule was saved in the active `三拼跨端` preset, and a request through the proxy returned HTTP 200 with JSON matching the complete prepared mock. The original disabled variants were preserved.

The process exposed opportunities in connection setup, tool payload size, workflow guidance, recoverability, and verification. Findings below describe that session and its observed tool surface; they are not a source-code audit or a claim that every issue remains present in later versions.

## Evidence and measurement limits

| Observation | Implication and limits |
| --- | --- |
| No Wirelens tools were exposed directly to the agent. The executable was available, so the agent constructed a shell-based MCP connection. | Connection integration and the fallback workflow added work. This does not establish why native tools were unavailable. |
| The initial broker discovered no MCP-enabled instances. A process check found a running proxy, and the user relaunched it with `--mcp`. | An empty discovery result needs diagnosis; it does not mean an empty capture store. MCP-disabled startup was the obstacle in this session. |
| A raw `tools/list` command reported approximately 24,468 output tokens and was truncated. | Schema discovery was the largest clearly visible individual token waste. This is the tool's reported output estimate, not a billing measurement. |
| Several raw results repeated JSON in both `content[].text` and `structuredContent`. | Printing the raw envelope exposed duplicate data to the model. Later calls extracted only structured results. |
| `get_mapping_settings` returned all six presets. Both validation and explanation accepted the full proposed configuration. | A one-rule change required handling unrelated settings and transmitting the same large proposal twice. |
| A `printf … \| wirelens mcp` attempt returned `registry scan cancelled` and `mapping conversion was cancelled`. | Premature stdin closure was the likely cause in the ad hoc client. This was a transport-lifecycle failure, not evidence that the proposed mapping was invalid. |
| The agent used a persistent terminal and later `stty -icanon -echo`. | Terminal management became part of a task that should only require structured tool calls. |
| Verification first failed inside the sandbox, then succeeded after host approval. | Connection failure and permission restrictions need separate handling. Repeated environmental setup should not be attributed to proxy processing. |
| The successful verification's outer tool call took about 15.5 seconds; the shell command reported approximately 0.119 seconds; the capture reported 1 ms total duration. | These measure different layers. The outer time includes orchestration or approval overhead; the record does not precisely apportion it. |
| The served mock was 153,428 bytes. The comparison ran locally and returned a short success message. | Keeping full response comparison outside model context worked well and should be preserved. |
| The actual verification capture used `local_only` against the effective HTTP URL. The original HTTPS URL's remote-then-local behavior was explained separately. | The local response was traffic-verified. The application's HTTPS/TLS path and UI rendering were not exercised by that check. |
| The rule was persistent but pointed to a file under `/private/tmp`. | Mapping persistence and mock-file durability are separate properties. |

The successful mapping workflow used nine task-level MCP reads/writes: broker status, instance discovery, instance status, mapping settings, validation, explanation, creation, settings reread, and capture search. This excludes initialization, schema discovery, failed attempts, and filesystem/HTTP commands. There is no reliable aggregate token or end-to-end latency measurement for the entire task.

## Recommended work, ordered by priority

### P0: Provide a supported connection and execution path

The skill should define an explicit preference order:

1. Use native Wirelens MCP tools when available.
2. Otherwise use a bundled client helper that launches `wirelens mcp` over managed stdio.
3. If neither path is available, prepare independent offline work where useful and report the concrete connection prerequisite.

The helper should own initialization, response-ID matching, deadlines, stderr handling, and shutdown. It should await initialization before sending the initialized notification and await outstanding responses before closing stdin. It should use pipes rather than an interactive terminal.

The agent should not need to improvise JSON-RPC pipelines, shell quoting, terminal settings, or process lifetime. A helper also provides a stable entry point for host permission handling; it must follow the host's approval requirements rather than bypass them.

Connection diagnostics should distinguish registry access denied, no discovered instances, connection refused, timeout, and protocol errors. Do not classify every empty discovery result as MCP disabled. A setup/doctor facility could explain likely causes and a concrete next step without silently starting proxies or changing trust settings.

### P0: Preview a small mutation against server-owned state

Add a proposed mutation-preview operation accepting the exact instance, `expected_settings_revision`, intended typed operation, and affected URL(s).

The server would apply that operation to an isolated snapshot and return:

- Validation diagnostics and relevant gates.
- A compact change description.
- Predicted original/effective URLs and matched rules.
- The revision against which the preview was evaluated.

This removes the need for the agent to reconstruct every preset and submit the full proposal to two different tools. Preserve unrelated state, revision checks, instance identity, and TUI draft protection. Any subsequent commit must reject a stale preview or revision.

Preview is not a file-readability check or a traffic test unless those checks are explicitly requested and separately reported.

### P1: Make reads and discovery proportional to the task

Support scoped mapping inspection by preset, table, and URL. A URL-focused result should include relevant remote rewrites, disabled local candidates, active selection, gates, rule identities/order, and revision. Filtering must not conceal an earlier rule that changes precedence.

Make status summaries compact by default or provide a task-focused summary. Fetch detailed limits, worker metrics, and audit history when they are relevant to diagnosis. Preserve warnings that affect the operation.

For discovery, cache full schemas locally and show the model only tool names/descriptions and the needed input schemas. Avoid printing the entire inventory or repetitive output schemas into the conversation. Simplify repeated schema structure where practical while retaining validation and interoperability. Cache invalidation must account for server version or tool-list changes.

### P1: Normalize outputs before model consumption

The client/helper should check JSON-RPC errors and `isError`, then expose `structuredContent` once, falling back to text when necessary. Preserve warnings, omitted counts, truncation information, and conflict details. Never silently swallow parse errors.

The MCP tools specification recommends serialized JSON text alongside structured content for backward compatibility. Deduplicate at the adapter/model-context boundary rather than simply deleting the compatibility representation from every server response.

Complete schemas, configuration, and large response bodies can remain available locally while the model receives the relevant fields and a bounded summary. An output budget must not hide errors or give a false impression of completeness.

### P2: Manage mock assets and verification explicitly

Consider managed mock assets that can clone an existing mapped file and apply a bounded JSON patch. Return a durable path or asset identity, source and result hashes, and changed JSON pointers. Enforce the intended patch scope and retain the original fixture.

This would reduce filesystem orchestration and avoid persistent mappings accidentally depending on temporary files. Fixture payloads should remain local; summaries should avoid exposing unrelated traffic or credentials.

A supported verification helper or MCP operation could check file readability, evaluate mapping selection, exercise an authorized local request, compare the response locally, and return compact evidence. A local-only probe should refuse unmatched mappings and upstream fallback. An actual request through the original HTTPS URL is a separate check requiring the appropriate existing routing and trust setup.

## Skill changes that prevent the observed detours

The reviewed skill already instructed the agent to prefer `structuredContent`; the agent did not consistently follow it. More prose alone is unlikely to eliminate this class of mistake. Move repetitive mechanics into an executable helper and make the skill select that path.

| Detour | Instruction/helper change |
| --- | --- |
| Missing native tools led to raw shell transport experiments. | Document one supported fallback and its prerequisites. |
| “Live schemas take precedence” led to a complete schema dump. | Distinguish authoritative data from data that needs to enter model context. Consume schemas locally and surface only relevant inputs. |
| One-shot stdin pipeline caused cancelled requests. | Prohibit ad hoc one-shot pipelines and PTYs for MCP calls; let the helper manage lifecycle. |
| Raw results duplicated JSON and metadata. | Normalize every result automatically and expose full diagnostics on demand. |
| Permission failure led to another connection attempt. | Classify the failure, then use the host's escalation mechanism when required. Do not broaden access automatically. |
| A failed `curl` response was passed directly to `JSON.parse`. | Check transport success before parsing, then compare the payload locally. |
| Parsing loops ignored exceptions. | Return explicit protocol/parse failures; never interpret missing parsed output as success. |

Proposed skill wording, contingent on shipping the helper:

> **Execution path**
>
> Use native Wirelens MCP tools when available. Otherwise use the bundled client helper. Do not construct ad hoc JSON-RPC pipelines or run the broker in an interactive terminal.
>
> Discover and cache schemas through the client. Show the model only relevant tool descriptions and required input schemas; do not print the complete tool inventory.
>
> Normalize every result before displaying it: check protocol errors and `isError`, then emit `structuredContent` once or use text as a fallback. Never silently discard parsing errors.
>
> Keep complete configuration objects inside the helper when validating proposals. Display the requested change, relevant mapping decisions, revision, and diagnostics.
>
> Classify connection failures before retrying. Preserve instance identity and follow the host's permission requirements. After an interrupted mutation, reread current state before considering a retry.

An illustrative helper error contract, not an existing API:

```json
{
  "ok": false,
  "stage": "connect",
  "code": "registry_access_denied",
  "retry_requires": "host_permission",
  "mutation_state": "not_started"
}
```

If a connection is lost during a write, report mutation state as unknown until reconciled. A retryable error flag must not authorize blindly repeating a mutation.

## Correctness, recovery, and collaboration

### Stable rule identities and names

The task had two disabled rules with identical source URLs. Stable rule IDs and readable names would make targeting, audit records, and updates less ambiguous. Retain indexes for ordering, not durable identity. Consider how IDs survive config reloads and manual YAML edits before implementing this change.

### Recoverable and atomic mutations

Consider idempotency keys plus an operation-outcome query so a client can resolve a lost acknowledgment without creating a duplicate rule. Define key scope, payload matching, retention, and restart behavior explicitly.

For workflows requiring several related edits, consider an atomic typed-operation transaction. Validate the full change before committing, preserve revision checks, and report persistence failures clearly. File creation and configuration persistence may require separate recovery semantics; do not promise atomicity across them without implementing it.

### Response provenance

Capture evidence could include the settings revision, matched rule IDs, and a hash of the actual local file bytes served. A mapped file can change without changing settings revision, so path and revision alone do not make a response reproducible. Compute or record provenance for the bytes actually served, rather than rereading the file afterward.

### Human/agent coordination

Expose who changed a rule and when. Surface compact conflict details when an agent encounters a TUI draft or intervening edit. Preserve human drafts and stop writes that conflict; the skill should explain the affected objects rather than try alternate mutations to force completion.

### Mock lifecycle and scoped undo

Consider named debugging sessions with explicit expiry or manual cleanup and a scoped undo operation. Expiry should be an explicit option, not a surprise default. Undo should only revert the session's own changes after checking for later edits. Managed assets should not be removed while other mappings still reference them.

### Precise verification outcomes

Report these independently:

- Configuration validation.
- File readability and payload validity.
- Predicted mapping selection, including remote-then-local behavior.
- Actual proxy traffic and its capture identity.
- Response assertions or whole-response comparison.
- Application behavior or UI rendering, when separately exercised.

Serving the correct JSON does not establish that the app displayed the bubble. A direct request to the effective HTTP URL does not establish that the original HTTPS/TLS path succeeded.

## Suggested delivery sequence

1. Ship the client helper and refine the skill's execution path, output normalization, and failure handling.
2. Add scoped reads and mutation preview; update the skill so it uses these instead of the mandatory full-configuration sequence.
3. Add stable identities, mutation-outcome recovery, and capture provenance.
4. Evaluate managed mock assets, constrained verification, session cleanup, and typed multi-operation transactions as separate features.

A possible future workflow is discover, inspect, prepare/preview, commit, and verify: approximately five task-level calls if the operations are designed to support it. This is a design target, not a measured performance claim. Multiple plausible instances or conflicts still require explicit resolution.

The reviewed skill currently prescribes broker status, instance discovery, instance status, settings inspection, full-proposal validation and explanation, and a settings reread after each mutation. API improvements need matching skill updates. Rich commit results can remove an unconditional reread in some workflows, but their returned revision must still be checked by subsequent writes.

## Evaluation and acceptance criteria

Use representative end-to-end agent scenarios:

| Scenario | Expected behavior |
| --- | --- |
| Native tools unavailable | Select supported fallback without inventing transports or commands. |
| No instance or MCP disabled | Diagnose the connection prerequisite without treating discovery as capture evidence. |
| Multiple instances or restarted proxy | Resolve the intended instance; never silently substitute a new run ID. |
| Duplicate URLs and disabled rules | Target the intended rule and preserve unrelated variants/order. |
| Remote-then-local mapping | Evaluate both stages and report the effective URL. |
| Stale revision or unfinished TUI draft | Reject conflicting writes and present actionable context. |
| Missing or changed mapped file | Distinguish configuration validity from serving failure and report payload provenance. |
| Lost mutation acknowledgment | Reconcile the outcome without a duplicate mutation. |
| Large schemas or fixtures | Keep complete data outside model context and return bounded, truthful summaries. |
| Local HTTP test succeeds but app HTTPS/UI is untested | Report the verification boundary accurately. |

Measure model-visible input/output tokens, tool round trips, retries, approval wait, client/orchestration time, and server execution time separately. Include task correctness, preservation of unrelated state, and recovery from faults; call count alone is not a sufficient success metric.

## Decisions to resolve before implementation

- Whether the supported fallback belongs in the skill bundle, the Wirelens CLI, or a shared package.
- The preview/commit contract and how stale previews are rejected.
- Stable ID migration and behavior after manual config edits.
- Idempotency/outcome retention across process restarts.
- Managed asset location, retention, ownership, and cleanup semantics.
- Whether active verification belongs in MCP or a companion helper, and its exact network constraints.
- How to expose compact results without hiding important errors, precedence, or incomplete evidence.

## References

- The map-local task and review discussion from 2026-09-09 through 2026-09-10; observations and timings are taken from that call record.
- Reviewed skill: `/Users/didi/.agents/skills/wirelens-mcp/SKILL.md` and its `references/tool-reference.md`. These are local, mutable instruction files, not repository-owned specifications.
- [MCP tools specification: structured content and backward-compatible text](https://modelcontextprotocol.io/specification/2025-11-25/server/tools#structured-content).

## Developer decisions for this implementation

The implementation targets the first two delivery stages, with these boundaries:

- **Ship the fallback in the CLI.** `wirelens mcp` remains the native stdio broker. `wirelens mcp tools [NAME]`, `wirelens mcp call TOOL --arguments JSON|@PATH|-`, and `wirelens mcp read URI` provide an installable fallback without a separately distributed runtime or interactive terminal. Native tools remain preferred.
- **Normalize at the client boundary.** Keep the server's compatible text and structured representations. The CLI emits structured content once, preserves tool/protocol failures, and never automatically retries an interrupted mutation.
- **Discover locally, selectively expose schemas.** The CLI fetches current schemas for each connection and emits a compact catalog or one named input schema. A persistent schema cache adds stale-cache and ownership concerns without reducing model context further, so it is not added.
- **Preview the typed operation against owner state.** `preview_mapping_mutation` takes the exact instance, expected settings revision, one typed operation, and at most 16 affected URLs. It validates an isolated candidate and returns mapping decisions without file reads, traffic, persistence, or revision advancement. Commit uses the existing typed tool and evaluated revision; preview does not reserve state or bypass human drafts.
- **Scope reads by preset and table, not by URL.** Selected tables retain disabled duplicates and original ordering/indexes, with explicit scope/omissions. URL filtering could hide an earlier rule or remote rewrite; use preview/explanation for URL decisions instead.
- **Avoid mandatory status dumps and settings rereads.** Discovery diagnostics and scoped mapping metadata are the normal mapping path. Detailed status remains available for diagnosis. Successful commits already return the revision, outcome, and persistence; reread when indexes or an uncertain outcome need reconciliation.
- **Update executable guidance with the API.** Built-in prompts and the reviewed local skill/reference use the fallback and preview workflow. The local skill files remain outside this repository.

The following suggestions remain separate design work, not claims of this delivery:

- Stable rule IDs/names require a YAML migration and explicit manual-edit identity semantics.
- Idempotency keys, outcome queries, and atomic multi-operation transactions require retention/restart and persistence contracts. This change preserves uncertainty after interrupted writes instead of promising exactly-once execution.
- Capture file-byte hashes and revision provenance require instrumentation at the actual serving path; a later file reread is not a substitute.
- Managed assets, sessions, expiry, scoped undo, and cleanup require ownership/reference rules. No fixture lifecycle is automated.
- File/payload checking and network verification remain explicit external actions with authorization and separately reported evidence. No probe may silently fall back upstream.
- Detailed status/audit data and existing human-draft conflicts remain authoritative. No new per-rule attribution schema or reduced default status response is introduced.

### Verification and upgrade notes

- `cargo test`: 855 library tests and 10 integration tests passed; the suite also runs an isolated child-test subprocess.
- `cargo clippy --all-targets --all-features -- -D warnings` and `cargo fmt --check` passed.
- Both independent design and performance reviews were completed. Adopted fixes give capture waits deadline headroom, reuse compiled mapping/validation across preview URLs, and bound scoped projection allocation before copying. The oversized-projection regression failed before its fix and passed in the final suite.
- A managed-CLI smoke scenario used two isolated loopback proxies. Scoped inspection retained disabled duplicate variants; a 16-URL preview left settings unchanged; stale preview/commit revisions and a restarted run ID were rejected; the unrelated preset and second instance remained unchanged.
- Actual HTTP traffic followed remote-then-local mapping and returned a 153,469-byte JSON fixture, compared byte-for-byte locally. Capture metadata reported `remote_then_local`; an 8 KiB decoded resource page matched the fixture and retained pagination metadata.
- A default unmatched capture wait returned `matched: false` after 30.08 seconds through the client. The CLI accepts `--timeout-secs 360` for longer waits; a full five-minute wait was not exercised.
- The smoke proxies and fixture directories were removed. Empty discovery afterward returned successfully. No application UI or original-URL HTTPS/TLS path was tested, and no user proxy/trust configuration was modified.
- Private control RPC is now version 2 because the owner-side mapping result shape changed; descriptor schema and MCP negotiation versions are unchanged. Reinstall the binary and restart existing MCP-enabled proxies so broker and owner use the same private protocol.
