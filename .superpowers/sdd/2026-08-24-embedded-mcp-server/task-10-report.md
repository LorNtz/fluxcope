# Task 10 Report

## RED intent

Task 10 tests define a bounded, revision-safe body-resource layer shared by the TUI and MCP broker. They require raw and decoded range paging, one aggregate admission budget, exact capture generation/revision checks, terminal decoded-body caching, canonical `wirelens://` resource URIs, and a public `get_capture` tool whose body links are immediately readable.

## RED coverage

- Raw pages slice retained preview chunks before transport encoding and report requested/actual ranges without flattening the full preview.
- Decoded pages cover identity, gzip, zlib and raw deflate, brotli, zstd, stacked encodings, binary bodies, malformed/unsupported encodings, eight-layer enforcement, 16 MiB decoded-output limiting, UTF-8 boundary correction, and strict manual range failures.
- Shared admission tests cover TUI/MCP queue, active, and queued-input saturation; cancellation/deadline while queued or decoding; reservation rollback; active worker lifetime; and runtime startup sharing one admission object.
- Runtime scheduler/cache tests cover exact generation/revision/side/representation keys, live-body no-cache behavior, terminal hits/revalidation, byte-accounted replacement/LRU eviction, deletion/clear/feed-gap purge, stale worker rejection, and shutdown.
- URI tests cover canonical IPv4/IPv6 round trips, default/explicit ranges, exact path and identity segments, unknown/duplicate/malformed query rejection, normalized traversal rejection, and canonical next-page links.
- Private RPC tests cover strict `ReadCaptureBody` requests/results, operation and instance identity, response bounds, raw/decoded page metadata, retention/revision failures, and source-versus-decode truncation semantics.
- MCP tests cover strict `get_capture`, annotations, exact-run selection, side-specific raw/decoded links, empty retained bodies, empty `resources/list`, the single Task 10 content template, resource capabilities, text/blob/MIME/base64/meta output, relay telemetry, typed errors, and broker disconnect cancellation.

## RED evidence

The focused Task 10 suites initially failed because the shared body-work admission API, bounded decoded paging, `ReadCaptureBody` private operation, revision-safe scheduler/cache, `BodyResourceUri`, resource handlers, and public `get_capture` contract did not exist. The fresh test-author subagent changed only tests and minimal test wiring and ran no formatter, build, lint, test, or Git command.

## Modified files

```text
src/capture/body_work.rs
src/capture/body_work/tests.rs
src/capture/change.rs
src/capture/decode.rs
src/capture/decode/task10_tests.rs
src/capture/mod.rs
src/capture/model.rs
src/control/body.rs
src/control/body/task10_tests.rs
src/control/mod.rs
src/control_rpc/framing.rs
src/control_rpc/protocol.rs
src/control_rpc/protocol/task10_tests.rs
src/control_rpc/server.rs
src/control_rpc/server/task10_tests.rs
src/mcp/body.rs
src/mcp/body/task10_tests.rs
src/mcp/broker.rs
src/mcp/broker/tests/task10_tests.rs
src/mcp/capture.rs
src/mcp/capture/tests.rs
src/mcp/mod.rs
src/runtime/control.rs
src/runtime/control/task10_tests.rs
src/runtime/mod.rs
src/runtime/startup_tests.rs
tests/mcp_broker_child.rs
what_i_just_did.md
```

## Implementation notes

- Added a shared `BodyWorkAdmission` with fixed active/queue/queued-input bounds. Queue admission does not retain input bytes while waiting for an active permit, and cancellation/deadline/shutdown interrupt every wait.
- Unified TUI and MCP decoding around one bounded, cancellation-aware decoder. It enforces the content-encoding layer cap, checks cancellation at bounded input/output checkpoints, emits typed malformed/unsupported/cancelled/limit failures, and never mutates source bytes.
- Added chunk-aware raw paging plus UTF-8-safe decoded paging. Strict manual ranges reject invalid UTF-8 starts; generated next pages remain readable without pre-decoding.
- Added `ReadCaptureBody` request/result contracts and a runtime scheduler that performs metadata-before-work revision validation, shares admission with TUI decoding, retains active permits through blocking worker exit, and returns range/source/decode metadata without confusing retained-source truncation with decoded-output limits.
- Added a byte-accounted terminal decoded-body cache keyed by exact instance generation, capture ID/revision, side, and representation. Ordered generation-indexed LRU eviction is amortized O(log n); capture-change epoch validation prevents stale decode completion from resurrecting deleted, evicted, replaced, or feed-uncertain entries.
- Added strict canonical body resource URI parsing and rendering, including standard bracketed IPv6 authorities, explicit canonical offset/length output, and exact identity-preserving next links.
- Registered `get_capture`, body resource reads, one Task 10 resource template, empty concrete resource listing, and resource capabilities without subscriptions/list-change flags. Raw/binary data is base64 after slicing; valid decoded UTF-8 is text with retained media type.
- Recorded body-resource relay bytes only after successful page conversion and returned stable typed public error data without local filesystem/socket details.

## Initial review findings and corrections

### Design and maintainability

- Restored the specified 32 MiB aggregate queued-input bound instead of widening the shared budget.
- Replaced separate MCP and TUI decoding behavior with one decoder and one typed error model.
- Prevented intermediate content-encoding layers from materializing unbounded output.
- Cached decoded UTF-8 validity and separated source truncation metadata from decoded-output limiting.
- Added feed/epoch-aware cache invalidation and rejected non-canonical normalized traversal resource paths.
- Replaced detached blocking cancellation behavior with explicit worker-cancellation ownership and normalized cancellation errors.

### Performance and memory

- Released TUI-local queued-byte reservations when shared MCP admission rejects.
- Charged each decoded layer against the fixed output cap and checked cancellation between bounded input/output chunks.
- Coalesced capture-change invalidation before locking the cache and replaced scan-based LRU eviction with ordered generation state.
- Prioritized queue cancellation before newly available capacity and prevented stale workers from inserting obsolete cache entries.

### Final performance re-review corrections

- Copied each decoded response page into at most 64 KiB of page-owned storage so a small private response cannot pin a full 16 MiB decoded backing allocation after its active lease is released.
- Borrowed successful decoded `Bytes` through TUI formatting instead of cloning them into another full-size `Vec`.
- Made UTF-8 validation, plain-text copies, form output, JSON input parsing, and JSON output writes observe cancellation at bounded checkpoints while the blocking worker retains its active lease.
- Added deterministic page-ownership and mid-format cancellation regressions.
- Replaced whole-field URL-form parsing with a bounded streaming delimiter/percent/UTF-8 pass after re-review identified that one 16 MiB field could still ignore cancellation.
- Buffered Serde JSON input in 32 KiB refills after re-review identified per-byte reader/atomic overhead.
- Routed empty-segment separator advances through the same checkpoint and retained one bounded decoder buffer across pairs after follow-up re-review found a cancellation bypass and per-pair allocation hot path.

## Main verification

- `cargo fmt --all -- --check`: passed.
- `cargo test control::body --all-features`: 4 passed.
- `cargo test capture::decode --all-features`: 25 passed.
- `cargo test capture::body_work --all-features`: 8 passed.
- `cargo test mcp::body --all-features`: 9 passed.
- `cargo test runtime::control --all-features`: 63 passed.
- `cargo test control_rpc::protocol --all-features`: 32 passed.
- `cargo test --test mcp_broker_child --all-features`: 2 passed.
- `cargo test --all-targets --all-features`: 684 passed.
- `cargo clippy --all-targets --all-features -- -D warnings`: passed.

## Final review

Final design/maintainability and performance/memory re-reviews passed after all findings were remediated. The last round confirmed bounded separator and ordinary form scanning, complete decoder-state draining with reusable capacity, buffered JSON input, page-owned response windows, and no material correctness, maintainability, time, or memory regression.

## Completion

Status: `GREEN - review clean`
