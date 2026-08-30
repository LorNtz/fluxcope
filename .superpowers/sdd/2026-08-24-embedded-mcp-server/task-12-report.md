# Task 12 Report: Locate Text and Extract Selected Body Values

## RED evidence

Main observed both focused Task 12 commands fail only because the production Task 12 APIs were absent. The initial pure-domain suite covered Unicode mapping and overlaps, binary fallback, compact JSON/form selection, 4096-byte inline behavior, canonical selection URIs, and UTF-8 boundary hints.

## Implementation

- `src/control/body.rs`: strict search/extraction/selection contracts; linear Unicode case-folded overlapping search with original UTF-8 byte mapping, exact aggregate counts, bounded aligned context and decoded-window links; binary-safe fallback errors; exact JSON/form selection; inclusive 4096-byte inline threshold; canonical selection URI parser/builder; UTF-8 selected-page construction.
- `src/control/json_walk.rs`: extended the existing one-pass event walker with exact scalar token views and a bounded selection observer. Selected JSON is emitted event-by-event as compact JSON; neither the parent document nor selected subtree is deserialized as `serde_json::Value`. Miss hints retain only escaped prefix and bounded child segment/type metadata.
- `src/runtime/control.rs`: added search, extraction, and selection-page scheduler methods through the existing `BodyJobScheduler::inspect_decoded_body` admission/cache/revision path. Active leases cover decode, search/extraction, result construction, and page copying. Returned results/pages own only bounded output windows.
- `src/control_rpc/protocol.rs`: added boxed strict private search/extract/selected-read operation and result variants, ordinary deadlines, closed argument parsing, and stable `body_not_textual`/`not_found` codes.
- `src/mcp/body.rs`, `src/mcp/broker.rs`: added closed MCP inputs/outputs, read-only/idempotent/non-destructive/closed-world tools, exact forwarding/identity checks, two selected-resource readers, selected-page metadata, and exactly three body templates.
- `tests/mcp_broker_child.rs`: updated the exact process tool/template contract and focused required-field/range assertions.

## Preserved invariants

- One shared Task 10 admission queue/decoder/cache; no secondary queue or decoder.
- Exact endpoint/run/capture/revision/side targeting and freshly returned instance identity.
- Revision-safe terminal caching and live recomputation remain inside the existing scheduler.
- Search retains at most 50 matches with 1 KiB context per side and continues for exact totals.
- Selected inline output is at most 4096 bytes; resource pages own at most 64 KiB and never retain the decoded parent.
- Structured extraction rejects incomplete/limited input and errors contain no body values.
- JSON selection uses streaming events rather than parent/subtree `serde_json::Value` materialization.

## Tests added (not run)

- Expanded pure body Task 12 tests for Unicode ranges, overlaps, binary fallback, JSON root/scalar/object/array and miss hints, repeated form values, threshold behavior, canonical IPv6 URI round-trip, and UTF-8 boundary hints.
- Added strict Task 12 private RPC request round-trip/unknown-field coverage.
- Added MCP input operation/schema coverage.
- Updated process-child exact tool and three-template assertions.

Per assignment, no Cargo command, formatter, Clippy, build, test, or commit was run. Scheduler stale-revision/lease integration fixtures and a broker fake-dispatch Task 12 test remain to be added during Main's bounded compile/fix pass.

## Status

Implementation saved for Main-owned compilation and focused verification.

## Compile fix round 1

Main's formatter parse pass found one missing closing brace at the end of `src/control_rpc/protocol/task12_tests.rs`. The test function is now closed. Structural parser inspection of all newly added Task 12 test files and the modified production modules found no analogous unmatched delimiter. No command was run in this delegated fix round.

## Missing-coverage test pass

Added deterministic runtime scheduler coverage for Task 12 stale-revision authority, selected UTF-8 page construction/next URI, and cancellation with shared active-permit release; broker fake-probe coverage for exact search/extract/selected-resource dispatch and returned identity; and focused malformed/depth/size/media-type/cancellation JSON/form extraction coverage, including cancellation while scanning one large form field. No command was run in this delegated pass.

## Coverage compile fix

Added the missing `serde_json::json` macro import to the new broker Task 12 dispatch test. The new runtime test already imports `json`; its other non-prelude dependencies and the broker test's `Bytes` dependency are explicitly imported, while the broker harness types follow the existing descendant-test `use super::*` convention. No command was run.

## Scheduler boundary correction and source-limit coverage

The Task 12 scheduler cancellation regression now asserts the repository's established observable boundary: cancellation while the exact revision-pinned snapshot command is pending maps to `instance_unavailable` with a cancellation message, and shared active/queued admission returns to zero. It no longer claims a blocking worker started. Added scheduler-level source-preview-limit coverage and moved complete-source rejection to metadata revalidation before snapshot acquisition for extraction/selection reads; the stable `resource_limit` error carries revision/truncation metadata and no body values. No command was run in this delegated pass.

## Source-limit receiver assertion fix

The source-limit regression now asserts only that no runtime command is queued after early metadata rejection, accepting either an empty or disconnected receiver because the spawned handler may already have dropped its last sender. No command was run.

## Main verification

- `cargo test task12_ --all-features`: 22 passed.
- `cargo test search_capture_body --all-features`: 3 passed.
- `cargo test extract_capture_body --all-features`: 4 passed.
- `cargo test mcp::body --all-features`: 15 passed.
- `cargo test runtime::control --all-features`: 67 passed.
- `cargo test control_rpc::protocol --all-features`: 34 passed.
- `cargo test --test mcp_broker_child --all-features`: 2 passed.
- `cargo fmt --all -- --check`: passed.
- `cargo clippy --all-targets --all-features -- -D warnings`: passed.
- `cargo test --all-targets --all-features`: 724 passed.

## Review

Initial implementation is GREEN. Required design/maintainability and performance/memory review is pending.
