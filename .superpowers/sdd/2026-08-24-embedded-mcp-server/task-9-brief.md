# Task 9 Brief: Wait for Matching Capture Milestones Through MCP

## Goal

Add the tests-first `wait_for_capture` feature from Task 9 of the approved embedded-MCP plan. A caller targets one exact live Wirelens run, supplies the existing AND-combined capture query, chooses a lifecycle milestone, and waits without parking `AppRuntime`. The operation must not miss a capture change between feed subscription and the initial search.

## Authoritative sources

- `docs/superpowers/plans/2026-08-24-embedded-mcp-server.md`, Task 9 (lines 998-1067).
- `docs/superpowers/specs/2026-08-24-embedded-mcp-server-design.md`, sections 8.3, 9.1-9.3, 10.2, 13, and 15.
- Task 7 capture snapshot/change-feed contracts and Task 8 query/search/private-RPC/MCP contracts already in the tree.

If a choice below is more specific than the prose plan, this brief is the Task 9 execution contract.

## Public contract

### Milestones

Add a strict snake-case enum:

```rust
CaptureMilestone::{RequestSeen, ResponseStarted, ExchangeTerminal}
```

Milestone truth derives from one `CaptureSnapshot`:

- `request_seen`: every retained snapshot satisfies it.
- `response_started`: `snapshot.timing.time_to_response.is_some()`. Do not infer this only from retained response metadata, because metadata can be unavailable under capture budget pressure.
- `exchange_terminal`: both request and response `BodyStreamState` values are terminal. Complete, failed, and cancelled terminal outcomes all satisfy it.

The existing `CaptureQuery` is evaluated first; a result is returned only when both the full query and the selected milestone are true. In particular, status/response-header/response-text filters cannot match before the response exists.

### Wait request/result

Add strict serializable/private-RPC request and result shapes equivalent to:

```rust
WaitForCaptureRequest {
    query: CaptureQuery,
    milestone: CaptureMilestone,
    timeout_ms: Option<u64>,
}

WaitForCaptureResult {
    matched: bool,
    capture: Option<CompactCapture>,
}
```

The public MCP input contains:

```json
{
  "instance": { "proxy_endpoint": "0.0.0.0:8989", "run_id": "..." },
  "query": {},
  "milestone": "request_seen",
  "timeout_ms": 30000
}
```

`instance.proxy_endpoint` and `instance.run_id` are structurally required in the advertised JSON schema, exactly as for `set_recording_enabled`. Unknown fields are rejected. `query` may default to the empty AND query; `milestone` is required. Omitted timeout defaults to 30 seconds. `timeout_ms == 0` is invalid. Values above 300,000 ms are defensively clamped to 300,000 ms (and the advertised schema documents 1..=300,000 for well-behaved clients).

Every successful result repeats resolved endpoint/run identity and returns:

```json
{
  "instance": { "proxy_endpoint": "...", "run_id": "..." },
  "matched": true,
  "capture": { "...compact metadata only...": "..." }
}
```

or, on normal timeout:

```json
{
  "instance": { "proxy_endpoint": "...", "run_id": "..." },
  "matched": false,
  "capture": null
}
```

`matched` and `capture` must agree (`true`/`Some`, `false`/`None`). Timeout is a successful tool result, never `deadline_exceeded`.

The MCP tool is named `wait_for_capture`, is read-only, non-destructive, non-idempotent, and closed-world. Its description must state exact-run targeting, milestone semantics, maximum wait, and timeout-as-unmatched behavior.

## Deadline contract

The requested wait itself is capped at five minutes. The broker gives the overall public/private call a fixed 30-second setup/serialization allowance beyond the normalized wait duration, so the target's successful `{matched:false}` timeout can win before transport teardown. Use one absolute deadline for public validation, instance resolution, private RPC, target dispatch, and response serialization. Private protocol deadline clamping must recognize `WaitForCapture` and allow at most `5 minutes + 30 seconds`; all existing operations remain capped at 30 seconds.

Cancellation/disconnect wins promptly over search admission, runtime commands, feed waiting, and the timeout. Detached blocking query workers retain their permits until actual completion, as in Task 8.

## Runtime/control architecture

### Explicit feed injection

The `CapturePublisher` already owns the single live `CaptureChangeFeed`. Expose a cheap clone accessor and pass that clone explicitly at runtime startup into a new instance control-service dependency such as:

```rust
ControlServiceContext {
    runtime: RuntimeControlClient,
    capture_changes: CaptureChangeFeed,
}
```

`RuntimeControlHandler` owns/clones this context. Do not create a second feed, use a global, or try to discover the feed after the control service was already constructed.

### Subscribe-before-query algorithm

The target control handler performs the wait outside `AppRuntime`:

1. Subscribe to `CaptureChangeFeed` **before** the initial runtime status/search command.
2. Compile the existing query under the same bounded blocking/search admission used by Task 8.
3. Search immutable snapshots newest-to-oldest in existing 32-row runtime batches. Each batch is matched off the runtime/UI thread. Return the newest snapshot satisfying query + milestone.
4. Remember only retained sequences that currently satisfy the full query but have not reached the milestone. This pending set is bounded by capture retention; remove entries that cease to match, become terminal without satisfying the query/milestone, or emit a removal event.
5. Release search-worker admission before sleeping. Waiting on the feed must not occupy a runtime command slot or one of the four capture-search workers.
6. Await the next feed event, normalized wait timeout, or cancellation/disconnect.
7. For `Admitted`/`RecordUpdated`, issue only a short `GetCapture` snapshot request for that changed sequence (no full-store rescan), then match query + milestone off-thread under bounded search admission. Multiple revisions may coalesce; use the latest internally consistent snapshot. A later feed event covers an update racing after the snapshot.
8. For `RetentionEviction`, `ExplicitDelete`, or `Clear`, return typed `capture_not_found` with capture ID/revision details only when the removed sequence was a remembered pending query match; unrelated removals do not fail the wait.
9. If an update event cannot be materialized because the record was removed before its check, return typed retryable `capture_not_found`/retention details rather than silently claiming no match: the service can no longer prove whether that change satisfied the wait.
10. A feed `Gap` returns typed retryable `service_unavailable` with expected and oldest available epochs. Never hide the uncertainty with a full rescan.
11. On normalized timeout, return the current instance identity plus `{matched:false,capture:null}`.

Pending-sequence memory must remain bounded by retained captures. Do not accumulate every observed sequence/revision for the full five-minute wait.

### Generation behavior

The public broker requires endpoint and run ID before dispatch. If the old instance disappears during a wait and the same endpoint is now owned by a different run, convert the failed old-run call into typed non-retryable `instance_generation_conflict` with requested/current run IDs. Do not retarget the wait. If no replacement generation can be proven, preserve the typed unavailable/service failure.

## Strict private RPC and schemas

Add `WaitForCapture` to:

- `ControlOperationKind`, `ControlOperation`, request decoding/encoding, deadline clamping, and operation/result identity matching.
- `ControlResult` and `instance_scope()`.
- `RuntimeControlHandler` dispatch.
- MCP capture input/result conversion and broker tool router/list.

Retain `deny_unknown_fields`, body-size checks, the existing 1 MiB private request / 8 MiB response limits, and typed error payloads. The wait returns only `CompactCapture`; never inline headers or body payloads.

## Required RED tests

Write deterministic tests first and prove they fail because Task 9 APIs/behavior are absent. Prefer child test modules already used by capture/control/runtime/MCP. Test observable behavior through real feeds, runtime gateways, protocol serde, and in-process MCP child transport where practical.

Minimum coverage:

1. **No missed subscribe/query race:** deterministically inject a matching capture after subscription but before/during the initial search; the wait returns it. Do not use timing sleeps as proof.
2. Existing newest matching terminal capture is returned even when a newer query match has not reached the milestone.
3. `response_started` is detected from timing even if response metadata is absent/truncated; status/response-header filters do not match before response metadata exists.
4. `exchange_terminal` accepts complete, failed, and cancelled outcomes and rejects a half-terminal exchange.
5. A changed sequence is rechecked without a second full-store scan; feed sleep holds neither runtime command capacity nor search-worker admission.
6. Default timeout and >5-minute clamping are normalized correctly; zero is rejected.
7. Paused-time timeout returns successful `matched:false` with `capture:null` and identity.
8. Cancellation/socket disconnect interrupts a blocked wait promptly and does not leak public/private/search permits.
9. A pending matching capture evicted/deleted/cleared during wait returns typed `capture_not_found`; unrelated eviction is ignored.
10. Update-materialization loss and feed gap return typed retryable errors with bounded details.
11. Endpoint replacement during wait produces `instance_generation_conflict`, never retargeting to the new run.
12. Protocol strictness: operation/result round trips, unknown-field rejection, wrong operation/result rejection, wait deadline cap while ordinary operations remain 30 seconds.
13. Public MCP tool list/schema/annotations and child transport: tool is advertised, selector fields are required, invalid zero timeout returns typed MCP data, and normal timeout returns structured unmatched success.
14. Runtime startup wires the publisher's exact feed into the control handler; an injected live publisher event wakes the real wait path.
15. High-frequency unrelated changes either remain bounded or produce an explicit feed-gap error; no unbounded queue/vector grows.

Tests must avoid asserting only fake call counts unless the count proves the architectural contract (for example, no full-store rescan). Hooks/seams used only to schedule the subscribe-before-query race belong under `#[cfg(test)]` and must execute the same production wait implementation.

## Non-goals

- No body search/extraction/resources (Tasks 10-12).
- No mapping mutation (Tasks 13-14).
- No cross-instance aggregate waits.
- No target-application triggering.
- No polling loop, permanent query index, second feed, HTTP listener, or resource subscriptions.
- No unrelated cleanup of deliberately staged later-task warnings.

## TDD and handoff workflow

1. Test author changes tests/minimal test fixtures only and reports `NEEDS_RED_RUN`.
2. Main runs `cargo test wait_for_capture --all-features` and confirms expected RED.
3. Implementer changes only Task 9 production surfaces plus required tests/fixtures, without running project commands or committing.
4. Main runs focused/full verification, then two review subagents (design/maintainability and performance), adopts valid findings, and reruns verification.
5. Main appends `what_i_just_did.md`, updates the SDD ledger/report, and commits Task 9.

## Main verification commands

```bash
cargo fmt --all -- --check
cargo test wait_for_capture --all-features
cargo test control_rpc::protocol --all-features
cargo test --test mcp_broker_child --all-features
cargo test --all-targets --all-features
cargo clippy --all-targets --all-features -- -D warnings
```

Strict Clippy may still report deliberately staged Task 10+ dead code or unrelated pre-existing warnings; Task 9-owned warnings must be zero.