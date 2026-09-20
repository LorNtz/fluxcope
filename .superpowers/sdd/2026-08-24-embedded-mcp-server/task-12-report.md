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

## Review fix round 1/5

Replaced full-body folded-unit materialization with streaming Unicode KMP and a query-length original-boundary ring; added the fixed 8192-byte query contract and bounded covering-window/schema regressions. Search results now expose `decoded_output_limited` through runtime, RPC serialization, MCP output, and tests so totals are identifiable as decoded-prefix totals. Form selection now uses complete component decoding (literal/encoded `=`, `+`, percent escapes, UTF-8 repair), incremental 32 KiB delimiter scans with deterministic cancellation barriers, and a capped JSON-array writer that reserves its closing byte and reports expansion overflow as `json_size_limit`. Added runtime decoded-limit, MCP schema/output, and child-contract regressions. No command, formatter, commit, or work-record operation was run.

## Main review-fix verification

Main fixed the first focused compile failure, then tightened the form path beyond the initial patch: component decoding now checks cancellation and repairs UTF-8 in bounded chunks, caps decoded component output, reuses one decoder across fields, and avoids reserving the full 16 MiB output limit for small selections. The capped writer remains length-bounded while growing on demand.

- `cargo test task12_ --all-features`: 29 passed.
- `cargo fmt --all -- --check`: passed.
- `cargo clippy --all-targets --all-features -- -D warnings`: passed.
- `cargo test --all-targets --all-features`: 731 passed.

All six initial specialist findings are addressed. Focused re-review is pending.

## Review fix round 2/5

Design re-review approved round 1. Performance re-review confirmed every original finding fixed but found that exact-capacity growth could reallocate once per small JSON escape fragment. The capped writer now doubles geometrically up to its hard limit and directly tracks growth under tests. A 600 KiB control-character serialization completes in at most eight growth operations.

- `cargo test task12_ --all-features`: 30 passed.
- `cargo fmt --all -- --check`: passed.
- `cargo clippy --all-targets --all-features -- -D warnings`: passed.
- `cargo test --all-targets --all-features`: 732 passed.

Performance re-review of the geometric-growth correction is pending.

## Final review

- Design/maintainability re-review: PASS. Decoded-limit metadata, complete form values, capped serialization, matcher ranges/windows, error safety, wire contracts, and real production-path coverage are approved.
- Performance/memory re-review: PASS. Geometric writer growth, query-bounded matcher state, fixed query/wire bounds, bounded form cancellation, shared lease lifetime, and result/page ownership are approved.

Task 12 is GREEN and review-clean.
