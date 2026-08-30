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

- Added the strict capture-query domain with explicit substring/glob modes, Unicode case-folded AND matching, validated non-empty status/time/sequence bounds, mapping/lifecycle precedence, newest-first sequence cursors, and metadata-only compact/detail DTOs.
- Substring and header-value matching now streams Unicode case-folded characters through a linear-time matcher instead of allocating a second maximum-size folded header. Cancellation is checked at original-input quanta of at most 4 KiB, including matches that cross a quantum boundary.
- Search pages have a seven-MiB compact-result JSON budget below the private eight-MiB frame limit. Byte-limited pages resume inclusively at the first omitted capture; a single row that cannot fit returns typed `resource_limit`.
- Added direct stored-`Arc` retrieval and an O(log n) newest-at-or-before store lookup. Runtime batches use at most 32 metadata/header snapshots and never run text/glob/header matching or detail DTO materialization on the event loop.
- Added runtime status, explicit live-recording mutation, bounded search batches, and one-snapshot capture detail with typed not-found/revision-conflict errors. Mutation and detail replies carry the authoritative run identity from the same runtime turn, avoiding a racy follow-up status request.
- Added one shared four-search admission per runtime control handler. Admission is acquired before query compilation; blocking workers own their semaphore permit, so cancellation or dropping the outer future cannot release capacity before the blocking matcher exits.
- Boxed the large capture snapshot reply, private search query, and private detail result payloads. Serde remains transparent through each box, so the strict private wire shapes are unchanged while runtime/broker enums stay compact.
- Added a four-worker detail-materialization admission shared by each runtime control handler. A cancelled outer detail call cannot release its permit until the admitted blocking conversion exits; queued cancellations never spawn another worker.
- Added strict private `get_status`, `set_recording_enabled`, `search_captures`, and `get_capture` operations/results with 30-second deadline clamps and typed capture errors.
- Added closed public schemas and annotated `get_status`, `set_recording_enabled`, and `search_captures` tools. `get_capture` remains private-only. Public capture inputs retain structural serde errors, while semantic query/page/byte-limit failures are validated inside the tool and retain typed MCP `code`, `retryable`, and `details` data.
- The mutation input schema now structurally requires both `instance.proxy_endpoint` and `instance.run_id`; snapshot search continues to allow an omitted selector.
- The required run ID remains deserialized and validated as `RunId`; its public JSON Schema is explicitly represented as a string so schema generation does not require exposing an internal `JsonSchema` implementation on `RunId`.
- Public `get_status` deliberately reuses the selector-resolution `DescribeInstance` private snapshot instead of opening a redundant second private status connection; the distinct strict private `GetStatus` operation remains implemented for internal/private callers.
- Narrow adjacent test corrections cover semantic MCP errors through real rmcp transport, structural mutation identity, request/result byte bounds, same-turn mutation/detail identity, real handler admission, streaming header matching, and status-filter non-emptiness.
- Marked the compatibility page matcher and runtime-reply inspection helper test-only, reduced the capture fixture helper to seven arguments, collapsed the matcher-hook branch, and added deterministic detail concurrency/cancellation permit-lifetime coverage.
- Public search semantic validation now runs in `spawn_blocking` only after public-call admission. The blocking worker owns the public permit during validation, including after outer-future cancellation; all matcher patterns have a shared 64-KiB pre-allocation bound with typed field/max/received error details at both public and private trust boundaries.
- Multi-batch page accounting now distinguishes the full single-row limit from remaining page capacity: only a row exceeding the full seven-MiB budget is `resource_limit`, while a smaller row that does not fit the remaining capacity is omitted with its inclusive sequence cursor. A real runtime/handler multi-batch regression covers exact resumption.
- Public search computes its single ordinary deadline before blocking validation. Validation selects the same deadline and cancellation token while the detached worker retains the public-call permit, and forwarding receives that exact deadline rather than a fresh 30-second budget; a paused-time gated regression proves typed timeout, retained permit ownership, and zero registry/probe dispatch.
- No formatter, build, lint, test, or Git command was run by this worker.

## Modified files

```text
src/app.rs
src/app/requests.rs
src/capture/model.rs
src/capture/store.rs
src/control/mod.rs
src/control/capture_query.rs
src/control/capture_query/tests.rs
src/control_rpc/protocol.rs
src/control_rpc/protocol/tests.rs
src/mcp/broker.rs
src/mcp/capture.rs
src/mcp/capture/tests.rs
src/mcp/schema.rs
src/recording.rs
src/runtime/control.rs
src/runtime/control/task8_tests.rs
src/runtime/event_loop.rs
.superpowers/sdd/2026-08-24-embedded-mcp-server/task-8-report.md
```

## Expected focused verification commands for Main

```text
cargo test control::capture_query --all-features
cargo test runtime::control --all-features
cargo test control_rpc::protocol --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo test mcp::capture --all-features
cargo test --test mcp_broker_child --all-features
```

Strict Clippy may continue to report explicitly staged Task 9+ or unrelated pre-existing warnings; the Task 8-owned warnings addressed here should be absent.

## Prior Main GREEN evidence (before review-fix round 2)

- Focused capture query, runtime control, private protocol, MCP capture, and broker-child suites pass: 15 + 22 + 20 + 8 + 2 tests.
- Full all-target/all-feature suite passes: 581 tests.
- `cargo fmt --all -- --check` and `git diff --check` pass.
- Strict Clippy has no Task 8 findings. The command remains non-zero only for deliberately staged later-task APIs and pre-existing unrelated warnings that were already present before Task 8.

## Review corrections

- Preserved typed MCP semantic validation, required mutation identity in the advertised schema, non-empty status filters, and same-turn mutation identity.
- Moved admitted query compilation and detail materialization to bounded blocking workers whose permits survive outer-future cancellation.
- Replaced folded header allocation/scanning with cancellation-aware linear matching, bounded search pages below the private response limit, and inclusive byte-limit cursors.
- Boxed new large protocol/runtime payload variants without changing their serde wire shapes and removed Task 8-specific lint debt.
- Added production-handler admission, cancellation, maximum-header, schema, transport, page-budget, and detached-worker permit-lifetime regressions.

## Review-fix round 2

- Public semantic search validation now runs on a blocking worker after public admission, with the public-call permit retained through worker completion.
- Every capture query pattern is capped at 64 KiB before folded/KMP allocation at both public and private trust boundaries.
- Page matching now distinguishes the full single-row limit from remaining page capacity; a cross-batch row that fits the page but not the remainder is returned on the next inclusive-cursor page.
- Added deterministic public validation permit-lifetime, pattern-bound, and real multi-runtime-batch resume regressions.

## Main GREEN evidence

- Focused capture query, runtime control, private protocol, MCP capture, and broker-child suites pass: 16 + 23 + 20 + 9 + 2 tests.
- Full all-target/all-feature suite passes: 586 tests.
- `cargo fmt --all -- --check` and `git diff --check` pass.
- Strict Clippy again reports no Task 8 findings; only deliberately staged later-task and pre-existing unrelated warnings remain.

## Review-fix round 3

- Public search now creates one ordinary deadline before blocking validation, enforces it during validation, and forwards the same deadline through selector resolution/private RPC.
- A deterministic gated validation timeout regression proves typed `deadline_exceeded`, detached-worker permit retention, and zero registry/probe dispatch.

## Completion

Status: `GREEN — round-three re-review pending`
