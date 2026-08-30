# Task 9 Report

## RED intent

The Task 9 tests require an exact-run public `wait_for_capture` MCP tool backed by a strict private RPC operation, a bounded runtime wait loop, and the existing compiled capture-query semantics. A wait must subscribe to the shared capture-change feed before its initial newest-first query, return the newest capture satisfying both the query and requested milestone, then recheck only changed capture sequences until success, timeout, cancellation, retention loss, feed uncertainty, or instance replacement.

## RED coverage

- Capture milestones derive from one metadata snapshot: `request_seen`, timing-based `response_started`, and two-sided `exchange_terminal` accepting complete, failed, or cancelled streams.
- Response status/header filters remain false until response metadata exists.
- Runtime tests cover subscribe-before-query race closure, newest satisfied capture selection behind a newer pending capture, changed-sequence-only rechecks, unrelated runtime progress, default/max/zero timeout handling, cancellation while sleeping, blocking-worker permit ownership, pending-capture removal, unrelated eviction, retryable materialization loss, bounded feed gaps, and the real publisher-feed path.
- Private RPC tests cover strict snake-case arguments, unknown-field rejection, milestone validation, a 330-second wait deadline cap without changing ordinary 30-second calls, tagged result identity, matched/capture shape consistency, and compact metadata-only results.
- Private server coverage proves socket disconnect cancellation and private-call permit restoration.
- MCP tests cover timeout normalization, typed zero-timeout errors, exact selector routing, public-call permit restoration on child disconnect, endpoint-generation replacement without retargeting, tool schema/annotations/description, and absence of public `get_capture`.

## RED evidence

Main ran:

```text
cargo test wait_for_capture --all-features
```

Compilation fails only because the Task 9 production contract is absent: `CaptureMilestone`, `WaitForCaptureRequest`, `WaitForCaptureResult`, `ControlOperation::WaitForCapture`, `ControlResult::WaitForCapture`, the runtime feed dependency/wait path, and the MCP input/result/tool route are not yet implemented.

The test author ran no formatter, build, lint, test, or Git commands.

## Modified files

```text
src/capture/mod.rs
src/control/mod.rs
src/control_rpc/client.rs
src/control_rpc/protocol.rs
src/control_rpc/protocol/tests.rs
src/control_rpc/protocol/task9_tests.rs
src/control_rpc/server.rs
src/control_rpc/server/task9_tests.rs
src/mcp/mod.rs
src/mcp/broker.rs
src/mcp/broker/tests/task9_tests.rs
src/mcp/capture.rs
src/runtime/control.rs
src/runtime/control/task9_tests.rs
src/runtime/mod.rs
tests/mcp_broker_child.rs
.superpowers/sdd/2026-08-24-embedded-mcp-server/task-9-brief.md
.superpowers/sdd/2026-08-24-embedded-mcp-server/task-9-report.md
```

## Expected focused verification commands for Main

```text
cargo test wait_for_capture --all-features
cargo test runtime::control --all-features
cargo test control_rpc::protocol --all-features
cargo test mcp::broker --all-features
cargo test --test mcp_broker_child --all-features
```

## Implementation notes

- Added strict wait milestones, timeout normalization, request/result consistency, and compact metadata result conversion.
- Added the private `WaitForCapture` operation/result with strict wire decoding, operation identity checks, and an isolated 330-second deadline cap.
- Injected the publisher-owned capture change feed into the runtime control service and implemented subscribe-before-query initial matching, bounded pending-sequence state, changed-sequence-only rechecks, successful unmatched timeout, typed cancellation/removal/materialization/feed-gap failures, and four-worker admission release before feed sleep.
- Added exact-run public MCP routing, closed schemas, fixed absolute call deadlines, generation-replacement conflict conversion without retargeting, and the annotated `wait_for_capture` tool while keeping `get_capture` private.
- Wrapped the broker server transport so transport EOF cancels the same rmcp service token before shutdown draining; this propagates disconnect cancellation to in-flight request contexts without polling.
- Corrected the child schema test to resolve valid local Schemars `$ref` nodes before checking required selector fields and milestone enums.

## Initial Main GREEN evidence

- `cargo test wait_for_capture --all-features`: 23 passed.
- Focused runtime control, private protocol, MCP broker, and broker-child suites pass: 35 + 26 + 30 + 2 tests.
- `cargo test --all-targets --all-features`: 608 passed.
- Strict Clippy has no Task 9 finding after removing the obsolete `ServiceExt` import; the command remains non-zero only for deliberately staged later-task and pre-existing warnings.

## Completion

Status: `GREEN — review pending`
