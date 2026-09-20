# Task 8 Brief: Search, Inspect, and Control Captures Through MCP

## Goal

Add the RED tests for Task 8 of `docs/superpowers/plans/2026-08-24-embedded-mcp-server.md`: strict capture query compilation and classification, stable newest-first pagination, bounded runtime batching/search admission, compact status and live recording control, private capture detail, and the public MCP `get_status`, `set_recording_enabled`, and `search_captures` tools.

This phase is **tests only**. Production implementation comes only after Main verifies the exact RED state.

## Read First

- `docs/superpowers/plans/2026-08-24-embedded-mcp-server.md`, Task 8.
- `docs/superpowers/specs/2026-08-24-embedded-mcp-server-design.md`, sections 7, 9.2–9.3, 10.1–10.2, 13, 15, 18.
- `.superpowers/sdd/2026-08-24-embedded-mcp-server/task-7-report.md`.
- Current `src/control/mod.rs`, `src/runtime/control.rs`, `src/runtime/event_loop.rs`, `src/capture/{model,store}.rs`, `src/control_rpc/protocol.rs`, `src/mcp/{schema,broker}.rs`, and adjacent tests.

## Constraints

- Tests only. Do not implement production behavior, run commands, format, lint, commit, edit docs/AGENTS/work records, or change dependencies.
- Preserve unrelated working-tree changes.
- You may add only the minimum module/test wiring needed to make new tests visible (`#[cfg(test)] mod tests`, an otherwise empty Task 8 module, test-only constructors/seams). Do not define the missing production result/query types merely to make compilation advance.
- Tests must consume real `CaptureRecord`/`CaptureSnapshot`/`CaptureStore`, real private protocol encoding/validation, the real runtime gateway, and real rmcp child transport where applicable. Do not duplicate production classifiers/matchers/paginators in test helpers.
- Test names must state observable contracts. Avoid sleeps where a barrier/channel/gate can make ordering deterministic.
- Keep maximum-sized data bounded by approved limits. No unbounded fixtures.

## Domain Contract to Pin

Production should expose a cohesive capture-query domain under `src/control/capture_query.rs` (names below are contractual; field layout may be adjusted only if tests retain all semantics):

- `CaptureQuery` and `CompiledCaptureQuery`.
- `CaptureTextFilter` with explicit `substring` or `glob` mode, not heuristic glob detection.
- `CaptureHeaderFilter { name, value }`, where `value` is an optional Unicode-case-folded substring.
- `CaptureStatusFilter` supporting exact or inclusive minimum/maximum status; invalid bounds reject before runtime work.
- `MappingPath::{Unmapped, RemoteOnly, LocalOnly, RemoteThenLocal}`.
- `CaptureLifecycle::{Live, Complete, Failed, Cancelled}`.
- `CaptureSearchCursor` based only on the next older `CaptureSequence`; it must not be an offset.
- `CaptureSearchPage { captures, next_cursor }`, `CompactCapture`, `CaptureDetail`, and bounded body descriptors with no payload bytes.

All provided filters combine with AND. Pin these semantics:

1. Method matching is case-insensitive over the HTTP method token.
2. Original/effective URL filters are independently targetable and support explicit substring/glob modes.
3. Unicode text matches case-folded method, original/effective URLs, status text, and retained request/response header names/values.
4. Header name is case-insensitive; optional value is Unicode-case-folded substring. Duplicate retained headers remain distinct in detail and any one matching occurrence satisfies the filter.
5. Exact status and inclusive ranges work; response-dependent filters do not match before response metadata exists.
6. UTC start lower/upper bounds and capture-sequence lower/upper bounds are inclusive and validated.
7. Mapping classification: URI changed + local path => remote-then-local; URI changed only => remote-only; local path only => local-only; neither => unmapped.
8. Lifecycle precedence: any failed body side => failed; else any cancelled side => cancelled; else both sides terminal complete => complete; otherwise live.
9. Search output is newest-first. Default page size 20, maximum 100. Cursor pagination remains stable when newer captures arrive between pages and does not duplicate/skip older records.
10. Matching cancellation is checked between fields/candidates and at least every 4 KiB while case-folding/scanning a maximum retained header value.

`CompactCapture` must include capture sequence/revision, method, original/effective URL, mapping path, optional response status, lifecycle, timing, and request/response observed/retained/truncation/stream state. It must serialize no header values and no body payload.

`CaptureDetail` must be built from one `CaptureSnapshotMode::MetadataOnly` snapshot and include retained request/response headers, timing, mapping/lifecycle/truncation, body descriptors, and current revision, but no body bytes. Optional expected revision mismatch returns typed `capture_revision_conflict` with current revision. Missing/evicted capture returns typed `capture_not_found`.

## Runtime and Scheduling Contract to Pin

- Add `CaptureStore::next_at_or_before(cursor)` or equivalent O(log n) lookup and prove it returns the newest record at/before the cursor without iteration from the oldest end.
- Internal runtime request/reply variants cover `GetStatus`, `SetRecordingEnabled`, `GetCaptureSearchBatch { cursor, max_rows }`, and `GetCapture { capture_id, expected_revision }`.
- Runtime search batches contain at most 32 immutable metadata/header snapshots, newest first, plus the next older sequence cursor. Runtime never clones the full store and performs no glob/case-fold/header scanning.
- One event-loop control turn stays within the existing turn budget. A deterministic injected slow matcher over a maximum retained header must not block an unrelated runtime `GetStatus`/recording command.
- Private control service admits at most four active capture-search jobs per instance. A fifth waits/cancels/deadlines without exceeding four. A search permit remains owned until its `spawn_blocking` matcher has actually exited; dropping the outer future must not leak or prematurely release the permit.
- Search obtains one bounded runtime batch, matches it outside `AppRuntime`, then repeats only until the page fills or the store is exhausted.
- `SetRecordingEnabled` requires an explicit bool, returns `previous` and `current`, is idempotent, and changes only live `RecordingState`, not `recording.start_record_on_launch` settings.

## Private RPC Contract to Pin

Extend strict private RPC with typed operations/results for `GetStatus`, `SetRecordingEnabled`, `SearchCaptures`, and `GetCapture`.

Tests must prove:

- Operation-specific arguments reject unknown fields, missing explicit booleans/required IDs, malformed cursors/ranges/times/enums, zero/over-limit page sizes, and trailing data.
- Operation-specific deadline clamps are bounded; ordinary status/mutation/search/detail calls never exceed 30 seconds.
- Results round-trip with exact tagged shapes and repeat endpoint/run identity.
- Domain errors are typed: at minimum `capture_not_found` and `capture_revision_conflict` are distinct `ControlErrorCode` values with structured details, never parsed from strings.
- `GetCapture` remains private-only in Task 8; it is not registered as a public MCP tool until Task 10 supplies readable body resource links.

## Public MCP Contract to Pin

Add strict `Deserialize + JsonSchema + deny_unknown_fields` inputs and `Serialize + JsonSchema` results for:

- `get_status`: snapshot-read selector semantics; compact selected-instance identity/config/persistence/recording/capture summary. Task 14 may expand metrics, but current result must not regress existing fields.
- `set_recording_enabled`: endpoint + run ID required, explicit `enabled`, returns repeated identity and previous/current; annotations: read-only false, destructive false, idempotent true, open-world false.
- `search_captures`: snapshot-read selector semantics, strict query, cursor, and limit; repeated resolved identity; compact rows and cursor. Annotations: read-only true, destructive false, idempotent true, open-world false.

Child-transport tests must initialize a real `Broker`, invoke these exact tool names, and prove:

- omitted selector is allowed only when exactly one instance is live;
- mutation requires both endpoint and run ID even with one instance;
- multiple instances require explicit selection and never cross-route captures;
- all schemas are closed and serialize stable snake_case enums/fields;
- `search_captures` rows contain no header values/body payloads;
- `get_capture` is absent from the advertised tool list;
- typed control failures retain `code`, `retryable`, and structured `details` in MCP error data.

## Required Test Coverage

At minimum add deterministic tests for:

1. Mapping and lifecycle classification matrix, including failed-over-cancelled precedence.
2. AND filter matrix: method, original/effective substring and glob, exact/ranged status, header name/value, Unicode case folding, UTC/sequence bounds, response-not-yet-started.
3. Invalid glob, ranges, times, page limits, and malformed enum inputs reject before runtime dispatch.
4. Newest-first page 1/page 2 cursor stability when sequence 41 arrives after paging 40..1; expected IDs 40..31 then 30..21.
5. Compact serialization contains no headers/body bytes; detail preserves duplicate header order but no body bytes.
6. Detail revision success/conflict/not-found/retention-eviction paths.
7. O(log n) store cursor lookup boundary cases and 32-record runtime batch cap.
8. Live recording setter previous/current, idempotence, and launch-setting independence.
9. Four-search global instance admission, fifth-call cancellation/deadline, and permit ownership through blocking completion.
10. Maximum-header slow matching leaves unrelated runtime control responsive; matcher cancellation is observed within a 4 KiB scan quantum.
11. Strict private argument/result JSON and typed errors.
12. Real MCP child calls, selector requirements, annotations/schemas, cross-instance isolation, and absence of public `get_capture`.

## Acceptance / Handoff

- New tests are comprehensive but fail because Task 8 production APIs/behavior are absent.
- Report modified files and the exact focused commands Main should run:
  - `cargo test control::capture_query --all-features`
  - `cargo test runtime::control --all-features`
  - `cargo test mcp::capture --all-features`
- Reply `NEEDS_RED_RUN` with modified files and expected missing APIs. Do not run commands or commit.
