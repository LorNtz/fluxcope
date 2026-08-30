# Task 8 Report

## RED intent

The Task 8 tests require explicit substring/glob capture filters, Unicode case-folded AND matching, exact and ranged status handling, inclusive UTC/sequence bounds, mapping and lifecycle classification, stable newest-first sequence-cursor pagination, metadata-only compact/detail shapes, bounded 32-row runtime batches, live recording mutation, typed detail conflicts/not-found errors, four-search admission with blocking-worker permit ownership, 4 KiB cancellation checks, strict private RPC arguments/results/deadlines, and the public `get_status`, `set_recording_enabled`, and `search_captures` MCP contract.

## RED coverage

- Capture-domain matrices cover all mapping paths, failed-over-cancelled lifecycle precedence, method/URL/status/header/text/time/sequence AND filters, explicit text modes, invalid query bounds, stable page cursors across a newer arrival, compact/detail data exclusion, duplicate header order, and bounded cancellation observation.
- Store/runtime tests cover cursor lookup boundaries, stored-`Arc` retrieval, newest-first 32-snapshot batches, inclusive batch cursors, recording previous/current idempotence without launch-setting mutation, detail revision success/conflict/not-found/eviction, four active searches, fifth-call cancellation, permit lifetime after outer-future drop, and unrelated runtime status progress while matching is blocked.
- Private protocol tests cover typed Task 8 operations, strict operation-specific arguments, malformed cursors/enums/times/ranges/page sizes, trailing JSON, 30-second clamps, exact tagged result shapes with repeated identity, and distinct structured capture error codes.
- MCP tests cover closed input/result schemas, explicit mutation identity/boolean requirements, snapshot-read omission, stable result shapes, structured MCP error data, real Broker/rmcp transport tool advertisement and annotations, absence of public `get_capture`, explicit routing, and cross-instance capture isolation through the existing fake registry/probe transport pattern.
- Repetitive child-transport permutations for every malformed JSON field are intentionally represented by strict protocol and schema/serde matrices rather than duplicated end-to-end cases.

## RED evidence

Main confirmed the focused suite is RED for the intended missing Task 8 production APIs and behavior. This test-authoring worker did not run commands, formatting, linting, or commits.

Focused commands for Main:

```text
cargo test control::capture_query --all-features
cargo test runtime::control --all-features
cargo test mcp::capture --all-features
```

## Implementation notes

- Added the strict capture-query domain with explicit substring/glob modes, compiled Unicode case-folded AND matching, validated status/time/sequence bounds, mapping/lifecycle precedence, cancellation-aware 4 KiB folding quanta, newest-first sequence cursors, and metadata-only compact/detail DTOs.
- Added direct stored-`Arc` retrieval and an O(log n) newest-at-or-before store lookup. Runtime batches use at most 32 metadata/header snapshots and never run text/glob/header matching on the event loop.
- Added runtime status, explicit live-recording mutation, bounded search batches, and one-snapshot capture detail with typed not-found/revision-conflict errors.
- Added one shared four-search admission per runtime control handler. Blocking workers own their semaphore permit, so cancellation or dropping the outer future cannot release capacity before the blocking matcher exits.
- Added strict private `get_status`, `set_recording_enabled`, `search_captures`, and `get_capture` operations/results with 30-second deadline clamps and typed capture errors.
- Added closed public schemas and annotated `get_status`, `set_recording_enabled`, and `search_captures` tools. `get_capture` remains private-only.
- Public `get_status` deliberately reuses the selector-resolution `DescribeInstance` private snapshot instead of opening a redundant second private status connection; the distinct strict private `GetStatus` operation remains implemented for internal/private callers.
- No formatter, build, lint, test, or Git command was run by this worker.

## Modified files

```text
src/app.rs
src/app/requests.rs
src/capture/mod.rs
src/capture/store.rs
src/control/mod.rs
src/control/capture_query.rs
src/control_rpc/protocol.rs
src/mcp/broker.rs
src/mcp/capture.rs
src/mcp/schema.rs
src/recording.rs
src/runtime/control.rs
src/runtime/event_loop.rs
.superpowers/sdd/2026-08-24-embedded-mcp-server/task-8-report.md
```

## Expected focused GREEN commands for Main

```text
cargo test control::capture_query --all-features
cargo test runtime::control --all-features
cargo test mcp::capture --all-features
```

## Main GREEN evidence

- Corrected two contradictory mismatch fixtures so the UTC and sequence filters remain valid while still excluding the representative capture; inverted ranges continue to be rejected by their dedicated validation cases.
- `cargo test control::capture_query --all-features`: 12 passed.
- `cargo test runtime::control --all-features`: 17 passed.
- `cargo test mcp::capture --all-features`: 7 passed.
- `cargo test control_rpc::protocol --all-features`: 20 passed.
- `cargo test --test mcp_broker_child --all-features`: 2 passed.
- `cargo test --all-targets --all-features`: 572 passed.
- `cargo fmt --all -- --check` and `cargo check --all-targets --all-features` passed.
- Strict Clippy still reports staged dead-code surfaces deliberately introduced by earlier/future MCP tasks, plus pre-existing test-only warnings; Task 8 review will assess its own findings before finalization.

## Completion

Status: `GREEN`
