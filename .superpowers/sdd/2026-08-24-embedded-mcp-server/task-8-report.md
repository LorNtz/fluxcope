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

## Completion

Status: `NEEDS_RED_RUN`
