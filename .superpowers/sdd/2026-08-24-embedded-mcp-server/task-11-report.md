# Task 11 implementation report

Status: **DONE_WITH_CONCERNS**

## Files and symbols

- `Cargo.toml`, `Cargo.lock`
  - Added the exact direct dependency `json-event-parser = "0.2.3"` and its lock entry.
- `src/control/json_walk.rs`
  - Added strict `JsonPointer` parsing/serialization and `JsonPointerPattern` parsing with one-segment `*`, literal-star `~2`, RFC 6901 `~0`/`~1`, root/empty-segment handling, and recursive-wildcard rejection.
  - Added `JsonWalkObserver`, one `SliceJsonParser` event loop, bounded path/container state, depth/input/path/result budgets, and event-level cancellation checks.
  - Added `FieldFinder` and `PatternProbe` observers and typed request/result/status/shape/hint types.
  - Added focused behavior tests for pointer escapes, wildcard/literal star, exact/root/array probes, Unicode casefold field discovery, bounds, misses, malformed/binary/size/depth/cancellation, and serialized-value non-leakage.
- `src/control/mod.rs`
  - Registered `json_walk`.
- `src/control_rpc/protocol.rs`
  - Added `FindJsonPointers` and `ProbeJsonPointerPattern` operation kinds, requests, results, envelope serialization/parsing/deadline routing, and stable JSON body error codes.
- `src/runtime/control.rs`
  - Added scheduler dispatch for both operations.
  - Added revision/side revalidation through the existing `BodyJobScheduler`, shared `BodyWorkAdmission`, decoded cache/admission plan, capture snapshot validation, and bounded blocking workers.
  - Active body-work leases remain owned by the blocking worker through decode, traversal, and result construction; cancellation/deadline paths set the worker flag and await worker exit.
- `src/mcp/body.rs`
  - Added strict MCP inputs and outputs with required instance/run/capture/revision/side targeting and operation construction.
- `src/mcp/body/task11_tests.rs`
  - Added schema, strict-input, typed-operation, limit/pattern-validation, and no-value-field tests.
- `src/mcp/broker.rs`
  - Added broker implementation methods and registered `find_json_pointers` and `probe_json_pointer_pattern` with read-only, non-destructive, idempotent, closed-world annotations and value-free descriptions.

## Invariants self-review

- The complete JSON graph is never deserialized; traversal is driven event-by-event by one `SliceJsonParser` walker.
- Decoded structured input is capped at 16 MiB; nesting is capped at 512 containers.
- Current escaped path is capped at 64 KiB. Examples, field matches, next hints, object summaries, and retained result text all have hard caps; aggregate counts continue after output caps are reached.
- Pointer examples and field matches contain pointer/type/shape/encoded-size metadata only. String, number, boolean, and null values are never retained in results or serialized.
- Pattern semantics are single-segment only: whole unescaped `*` is wildcard, `~2` is literal star, and `**` is rejected.
- Endpoint/run selection stays broker-side; capture/revision/side travel in typed RPC requests. Runtime metadata and snapshots validate exact capture ID, side, revision, and instance generation before body work.
- Both operations use Task 10's shared scheduler admission, decoded cache, decoding policy, cancellation token, deadline, and active lease rather than introducing another admission service.
- Success results, including no-match results, carry capture revision, truncation flags, inspected bytes, and stable matched/no-match status.
- Malformed JSON, non-UTF-8 content, decoded-size overflow, depth overflow, cancellation, and deadline paths map to typed control errors.

## Tests added (not run)

Per assignment constraint, no Cargo command, rustfmt, Clippy, build, or test was run. Added deterministic tests covering:

- RFC 6901 root, empty segment, `~0`, `~1`, invalid escapes, array indices.
- Wildcard versus `~2` literal-star and recursive wildcard rejection.
- Exact no-wildcard probing and root probing.
- Exact and Unicode casefold field discovery without array-index false matches.
- Scalar encoded-size and object/array shape metadata.
- Example/match/hint/object-summary/result byte bounds on a large document while preserving exact totals.
- Miss longest-prefix/next-segment hints.
- Malformed JSON, binary/non-UTF-8 input, 16 MiB cap, 512-depth cap, and early cancellation.
- No matched scalar/string value in serialized walker or MCP output.
- MCP required selector/revision/side schema fields, unknown-field rejection, typed operation construction, invalid pattern/limit rejection.

## Known concerns

- Programmatic validation was explicitly prohibited, so compile/test status is unobserved. The first Main-owned verification pass should pay particular attention to `schemars` transparent derivation for `JsonPointerPattern`, the exact `json-event-parser` error/event API, and exhaustive-match fallout from the two new protocol variants.
- A dedicated end-to-end broker fake-probe dispatch test was not completed before the parent-requested handoff. Broker production dispatch and MCP input operation tests are present, but the broker method round trip itself still needs a focused test.
- `BodyJobScheduler` shares Task 10 admission/cache/metadata/snapshot/decode primitives, but its structured-inspection orchestration is a sibling scheduler method rather than a completed refactor of decoded page reads into one generic orchestration function.
- The Task 10 decoded-page cache-hit path still releases its active lease before page construction; Task 11 cache-hit traversal does retain the lease as required. Task 11 did not change unrelated Task 10 read behavior.

## Fix round 1

Main's first focused compile reported nine errors. This round made the following exact fixes without running commands:

- `JsonPointer` and `JsonPointerPattern` deserialization now passes `ControlError::message()` to Serde instead of requiring `ControlError: Display`.
- All four Task 11 `BodySide` request/input schema fields use the repository's `#[schemars(with = "String")]` convention; `BodySide` itself was not widened with a global schema implementation.
- `ControlErrorCode::as_str` now has stable wire names for `undecodable_body`, `malformed_json`, `json_depth_limit`, and `json_size_limit`.
- Added `src/mcp/broker/tests/task11_tests.rs` and registered it in the broker test module. The focused fake-probe test dispatches both public broker implementations and asserts exact capture ID, revision, side, field mode/limit, and pointer pattern on the typed RPC operations.
- Inspected production `ControlOperation`, `ControlOperationKind`, and `ControlResult` matches. Protocol kind/identity/envelope parsing/serialization, runtime dispatch, and broker result extraction represent both new variants; existing broker probe fallback remains intentionally wildcarded.
- Corrected walker observer start/end symmetry: container end callbacks now correspond to every observed start even when child descent is pruned, preventing bounded object-summary stack state from becoming misaligned on divergent pattern branches.

No command was run in this fix round, per the delegated constraint. The stale concerns above about missing broker dispatch coverage and the originally reported compile diagnostics are resolved by source changes but remain unverified until Main reruns the focused commands. The scheduler-structure concern remains substantive: Task 11 uses the exact Task 10 admission/cache/revalidation/decode primitives and retains the active lease through result construction, but decoded page reads and structured inspection still have sibling orchestration methods rather than one generic method.

## Focused verification and warning cleanup

Main reran the focused verification after fix round 1:

- `cargo test control::json_walk --all-features`: **8 passed**.
- `cargo test mcp::body --all-features`: **13 passed**.
- Focused broker fake-dispatch test: **1 passed**.

The successful build reported only three Task 11 dead-code warnings. `MAX_JSON_RESULT_BYTES`, `JsonPointer::segments`, and `JsonTypeCounts::count` are test assertions/helpers rather than production API, so this round marked exactly those items `#[cfg(test)]`. The production retained-text/result budgets remain unchanged and enforced. No command was run by the delegated agent for this cleanup.


## Strict Clippy enum-size fix

Main's strict Clippy pass identified that the two structured payloads enlarged `ControlResult`, which cascaded into large-enum warnings for outer dispatch/test enums. The fix is at the source: `ControlResult::FindJsonPointers.result` and `ControlResult::ProbeJsonPointerPattern.result` are now `Box<...>`, consistent with existing large `GetCapture` and `ReadCaptureBody` payloads. Runtime constructors box the completed result, broker handlers validate then unbox into their public MCP output types, and the fake-probe broker dispatch test constructs boxed control results. No lint was suppressed and no unrelated outer enum was boxed. No command was run by the delegated agent.

## Child-process MCP contract update

Main confirmed strict Clippy passes. The full suite then reached 694 tests before the child-process MCP contract test failed because its Task 10-era exact tool list omitted Task 11. The integration test is renamed to `mcp_child_negotiates_earlier_protocol_and_exposes_task11_contract`; its sorted exact list now includes `find_json_pointers` and `probe_json_pointer_pattern`. Focused stable schema assertions cover both new tools' required exact instance/capture/revision/side targeting, operation-specific field, read-only/value-free descriptions, field match-mode enum, optional 1..=20 finder limit, and string pointer-pattern schema. The existing common loop continues to verify closed-world schemas and read-only/non-destructive/idempotent/closed-world annotations without duplicating walker or broker unit behavior. No command was run by the delegated agent.

## Main verification

- `cargo fmt --all -- --check`: passed.
- `cargo test control::json_walk --all-features`: 8 passed.
- `cargo test mcp::body --all-features`: 13 passed.
- `cargo test broker_dispatches_both_json_tools --all-features`: 1 passed.
- `cargo test control_rpc::protocol --all-features`: 32 passed.
- `cargo test runtime::control --all-features`: 63 passed.
- `cargo test --test mcp_broker_child --all-features`: 2 passed.
- `cargo clippy --all-targets --all-features -- -D warnings`: passed.
- `cargo test --all-targets --all-features`: 697 passed.

## Review

Design/maintainability and performance/memory task review is pending. The remaining scheduler-structure concern is intentionally submitted to review: Task 11 uses the same bounded primitives and active lease but has a sibling structured-inspection orchestration method.

## Design/performance review fix pass

Six review findings were addressed in `src/control/json_walk.rs` without changing the Task 10 scheduler or MCP contracts:

- Scalar `scalar_encoded_bytes` now comes from the parser's exact consumed source offsets and raw token boundaries, so original string escapes and number spellings are counted rather than reconstructed from decoded values.
- Pattern matching now advances one incremental prefix state per observed value. It no longer reparses the complete pointer or rescans the complete pattern at each event, and exact non-matching branches prune observer work.
- Matched-object summaries are tied to the current object depth and record direct children only; nested descendants cannot leak into an ancestor summary.
- Every retained path/hint/key is charged using its JSON-escaped wire length plus conservative structural overhead. A final exact serialized-size enforcement pass evicts bounded collections while incrementing the corresponding omitted counter, guaranteeing the 64 KiB result ceiling.
- Traversal pointer state is capped at 32 KiB, below the result ceiling, and returns a stable `resource_limit` error before an adversarial key can grow retained path state beyond that limit.
- Added focused tests for exact escaped-string/number/boolean/null token spans, maximum-depth pruned probing, direct-child summaries, JSON-escaped output budgeting, miss-hint churn, and traversal path limits.

No command was run in this delegated fix pass, per the task constraint.

### Remaining concern from this review pass

`json-event-parser` 0.2.3's low-level slice API exposes an event only after scanning a complete token and does not preserve a partial token across independently discarded fixed-size slices. The walker checks cancellation before every parser event and during its own source/path scans, but one parser call can still scan one complete scalar token (bounded by the 16 MiB decoded-input ceiling) before the next cancellation check. A fixed-quantum cancellation guarantee inside a single very large scalar therefore remains incomplete; fixing it cleanly requires either a cancellation-aware buffered `Read` adapter plus independent exact raw-token position tracking, or an upstream parser API that exposes resumable token scanning and consumed offsets together.

## Round-1 compile repair

Main's first compile attempt after the six-finding review remediation stopped on the non-ASCII `é` inside a raw byte string test literal. The exact-token-span fixture now uses a UTF-8 raw string followed by `.as_bytes()`, preserving the original JSON source bytes and the asserted encoded token sizes: 6 bytes for `"é\n"`, 8 for `-1.20e+3`, 5 for `false`, and 4 for `null`.

The surrounding six new tests and their corresponding walker, incremental pattern-state, direct-child summary, wire-budget, path-limit, and final-size-enforcement edits were inspected for analogous obvious syntax/type mistakes; none were found by source inspection. No command was run, per the delegated constraint. The fixed-quantum cancellation concern above remains unchanged.

## Round-1 ownership and temporary-lifetime compile repair

The nine diagnostics from Main's next compile were repaired at their shared sources without running commands:

- `WalkPath<'a>` now derives `Clone` and `Copy`. Its three fields are immutable borrowed/scalar views (`&str`, `Option<&str>`, and `usize`), so copies are zero-cost bitwise view copies with no owned pointer allocation. This structurally resolves all eight move-after-use diagnostics across observer start/scalar/container and pattern-probe paths.
- `append_pointer_segment` now binds its four-byte UTF-8 scratch array before selecting the encoded segment text. The borrowed `encode_utf8` result therefore cannot outlive its backing array, resolving the temporary-lifetime diagnostic while preserving cancellation and pointer-size checks.
- The file's other `WalkPath` consumers and UTF-8 encoding sites were inspected. They share the same immutable view semantics, and there is no second temporary scratch-array site or path clone to change.

All six review-finding fixes and their tests remain intact. No bounds, cancellation behavior, or event hot-path ownership model was weakened, and no command or commit was run per the delegated constraint.

## Round-1 strict Clippy cleanup

Main's strict Clippy rerun reported exactly two diagnostics in `src/control/json_walk.rs`. `WalkPath::to_owned(&self)` is now `WalkPath::into_owned(self)`, and all four call sites use the value-taking conversion. Since `WalkPath` is `Copy` and contains only borrowed/scalar views, consuming it is a zero-cost view copy; construction of the owned `JsonPointer` remains unchanged.

The source-token whitespace scan now uses `scanned.is_multiple_of(32 * 1_024)` instead of a manual modulus comparison, preserving the same cancellation check cadence at exact 32 KiB multiples. No lint was suppressed, and no behavior, bound, or allocation policy changed. No command or commit was run per the delegated constraint.

## Round-1 main verification

- `cargo fmt --all -- --check`: passed.
- `cargo test control::json_walk --all-features`: 14 passed.
- `cargo clippy --all-targets --all-features -- -D warnings`: passed.
- `cargo test --all-targets --all-features`: 703 passed.

Scoped design and performance re-review passed. Both reviewers confirmed every first-round finding is addressed. They also confirmed that the parser's at-most-one-token cancellation delay is bounded by the 16 MiB decoded-input cap, satisfies Task 11's explicit between-event cancellation contract, and cannot leak a worker lease.

## Final review

- Design/security/correctness: PASS. Malformed-body data no longer appears in errors; original token spans drive encoded sizes; borrowed callback paths remain valid; object summaries are direct-child-only; path and serialized result bounds are enforced.
- Performance/memory: PASS. Traversal is linear in decoded input with depth-bounded state; retained output and final enforcement are bounded; aggregate totals remain exact; nonmatching scalar tokens are not rescanned.
- Remaining Critical/Important findings: none.

**Completion:** GREEN — review clean.