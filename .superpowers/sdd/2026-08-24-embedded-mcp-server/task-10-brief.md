# Task 10 Brief: Page Raw and Decoded Body Resources

## Goal

Implement Task 10 of the approved embedded-MCP plan through strict TDD: one per-instance bounded body-work scheduler shared by the TUI decoder and MCP, cancellable/layer-bounded content decoding, revision-safe body snapshots with a terminal decoded-body cache, strict `wirelens://` content-resource URIs, MCP resource reads, and a public `get_capture` tool whose result links to readable first pages.

This task owns raw/decoded content paging only. JSON-pointer/form resources, body search/extraction, mapping tools, final status/metrics, and prompts remain later tasks.

## Authoritative sources

- `docs/superpowers/plans/2026-08-24-embedded-mcp-server.md`, Task 10 (lines 1071-1173).
- `docs/superpowers/specs/2026-08-24-embedded-mcp-server-design.md`, especially sections 10.2, 11, 13, 14, and 15.
- Current capture/runtime/private-RPC/MCP contracts in the tree after Task 9.
- This brief resolves choices left implicit by prose. If it conflicts with the approved plan/spec, the plan/spec wins.

## Required public/domain contracts

### Body request and page domain (`src/control/body.rs`)

Add:

- `BodyRepresentation::{Raw, Decoded}` serialized as `raw` / `decoded`.
- `BodyContentRequest { capture_id, capture_revision, side, representation, offset, length }`.
- `BodyRange { offset, length }` for both requested and actual byte ranges.
- `BodyPageSource` carrying the source `stream`, `observed_bytes`, `retained_bytes`, source truncation flag/reason, decoded encoding chain, and decoded-output-limit flag.
- `BodyPage { content, media_type, requested_range, actual_range, total_bytes, next_offset, source }`.

`BodyPage::content` is the selected representation bytes before MCP transport encoding. Keep it binary-safe (a byte buffer serialized through the private RPC, not a lossy string). Raw range offsets apply to retained source bytes. Decoded range offsets apply after `Content-Encoding` decoding.

Constants:

- default page length: 8 KiB;
- maximum page length: 64 KiB;
- maximum decoded content: 16 MiB;
- maximum `Content-Encoding` layers: 8;
- terminal decoded cache: 32 MiB per instance.

Validation:

- `length == 0` and `length > 64 KiB` are `invalid_argument`;
- an offset beyond the selected representation length returns an empty actual range at `total_bytes`, not a panic or wrap;
- `next_offset` is the actual end when more selected bytes remain, otherwise `None`;
- for decoded valid UTF-8, manual start/end boundaries must be UTF-8 character boundaries. A misaligned range is `invalid_argument` with `nearest_start` (floor boundary) and `nearest_end` (ceiling boundary). Tool-generated first/next URIs must always be aligned;
- raw/binary ranges are byte-exact and need no UTF-8 alignment.

Media type is the trimmed retained `Content-Type` header value when present; no sniffing or JSON/form display formatting occurs.

### Runtime authority and private RPC

Add runtime requests:

- `GetCaptureBodyMetadata { capture_id, side }`;
- `GetCaptureBodySnapshot { capture_id, expected_revision, side }`.

Both replies include exact `InstanceScope`, capture ID/revision, selected side status, selected-side headers, and retained length as needed; only the snapshot reply owns the selected `CapturedBodyPreview`. Runtime lookup remains the retention/revision authority and uses one atomic `CaptureSnapshot`; stale revision is `capture_revision_conflict`, missing/evicted capture is `capture_not_found`/the current project spelling.

Add one private operation/result pair:

- `ControlOperation::ReadCaptureBody(Box<BodyContentRequest>)`;
- `ControlResult::ReadCaptureBody { instance, page: Box<BodyPage> }`.

The broker resolves endpoint/run and issues exactly one typed private body operation. Operation kind/request parsing/deadline/result-kind/instance validation must stay strict and round-trip through framing. Ordinary 30-second private/public deadline rules apply.

### Shared body-work admission (`src/capture/body_work.rs`)

One `Arc<BodyWorkAdmission>` is created during runtime startup and passed to both `start_decode_service` and `ControlServiceContext`.

Fixed aggregate per-instance limits across TUI and all MCP body work:

- 2 active blocking body workers;
- 8 queued jobs;
- 32 MiB total queued input charge.

Admission lifecycle:

1. Before waiting for active capacity, determine the input charge and atomically own one queued slot plus the queued-byte lease.
2. Queue/byte saturation rejects without unbounded waiting. TUI `request` remains nonblocking and returns `false`; MCP returns typed `resource_limit` unless cancellation/deadline already won.
3. Wait for an active permit with cancellation/deadline awareness.
4. Only after active acquisition, release queued slot and queued-byte leases.
5. Move the active permit into the blocking worker/completion guard. Dropping/cancelling the awaiting RPC must not release it before the worker exits.
6. Never wait for queued-byte capacity while holding an active permit.

Tests need crate-visible observability only under `#[cfg(test)]` (active/queued permits and queued bytes); do not widen production APIs solely for tests.

### Shared content decoder (`src/capture/decode.rs`)

Both TUI and MCP call:

```rust
pub(crate) fn decode_content_bytes(
    input: &CapturedBodyPreview,
    headers: &CapturedHeaders,
    policy: &ContentDecodePolicy,
    cancelled: &AtomicBool,
) -> Result<DecodedBytes, DecodeContentError>;
```

Requirements:

- supports gzip/x-gzip, zlib or raw deflate, brotli, zstd, and identity;
- reverses the retained encoding chain correctly;
- rejects more than eight non-identity layers with typed unsupported-body-encoding failure;
- checks cancellation between layers and at least every 32 KiB of input/output work;
- uses bounded read/write chunks and enforces the 16 MiB output cap at every layer;
- returns selected decoded bytes, normalized decoded encoding chain, output-limit flag, and typed failure/cancellation;
- never pretty-prints JSON, formats forms, expands tabs, emits display placeholders, or mutates source bytes.

All codec work remains inside `spawn_blocking` call sites. TUI formatting continues after this content-only function and preserves its current visible behavior.

### Revision-safe scheduler/cache

A per-instance `BodyJobScheduler` in the runtime control service owns the shared admission handle plus a 32 MiB byte-bounded LRU of terminal decoded representations.

- Key includes exact instance generation (endpoint + run), capture ID, capture revision, side, and representation.
- Metadata inspection before admission must not clone the body preview. Charge retained source length on a miss; charge cached decoded length on a hit.
- After active acquisition, revalidate store retention, revision, chosen side, required charge, and cache key. If required charge/availability changed while queued, drop active and retry from queue admission; never acquire queued bytes while active.
- Raw paging iterates `CapturedBodyPreview::chunks()` without flattening the full body.
- Decoded work executes in `spawn_blocking`, with the active lease and cancellation flag owned by the worker until it exits.
- Cache decoded content only when the selected side is terminal (`complete`, `failed`, or `cancelled`). Live decoded reads are never cached.
- Cache replacement/eviction is byte-accounted LRU. Capture change-feed eviction/delete/clear and newer revisions purge obsolete entries; cache never resurrects evicted/restarted captures.
- A feed gap invalidates the cache rather than serving uncertain history.

### Resource URI and MCP representation (`src/mcp/body.rs`)

Advertise exactly this Task-10 template:

```text
wirelens://{+proxy_endpoint}/runs/{run_id}/captures/{capture_id}/revisions/{capture_revision}/bodies/{side}/content/{representation}{?offset,length}
```

`BodyResourceUri::content(...)` emits a canonical URI with explicit offset and length. `parse` accepts only:

- `wirelens` scheme;
- canonical socket authority including standard bracketed IPv6;
- exact path segments shown above (no empty/missing/extra segments or traversal);
- valid run ID, positive capture ID as accepted by `CaptureSequence`, revision, side, and representation;
- optional single `offset` and optional single `length`, defaulting to `0` / `8192`;
- no userinfo, fragment, unknown query key, duplicate key, malformed percent encoding, overflow, or semantically invalid length.

Resource reads resolve as `SelectorRequirement::Resource` (exact endpoint/run), use the standard 30-second public call admission/deadline/cancellation path, and perform one private `ReadCaptureBody` call.

MCP output:

- raw is `blob` with base64 applied only after slicing;
- decoded valid UTF-8 is `text` with retained MIME type;
- decoded non-UTF-8 is `blob`;
- resource URI in returned content is the canonical requested URI;
- `_meta.wirelens` contains requested/actual range, selected `total_bytes`, canonical `next_uri`, endpoint, run ID, capture ID/revision, side, representation, source stream state, observed/retained bytes, source truncation/reason, decoded encoding chain, and decoded-output-limit flag.

Expected public resource failures map to JSON-RPC `ErrorData` containing the same stable `ControlError` code/retryable/details representation already used for tool errors. Do not expose filesystem/socket internals.

### `get_capture` public tool and resources capabilities

Add a strict input requiring exact endpoint/run plus `capture_id` and optional `expected_revision`. Register `get_capture` with annotations: read-only `true`, destructive `false`, idempotent `true`, open-world `false`.

Return:

- selected instance identity;
- existing consistent `CaptureDetail` metadata;
- request/response body resource links grouped by side, with `raw` and `decoded` concrete first-page URIs where that side exists. Request links exist for a retained request. Response links exist only after response metadata exists. Empty retained bodies still have valid links.

`resources/list` returns an empty list. `resources/templates/list` returns only the content template in Task 10. Server capabilities advertise resources support without subscription or list-change flags. Task 12 will add the two selection templates atomically with handlers; do not advertise them now.

## Required RED coverage

Write deterministic behavior tests before production implementation. Cover at minimum:

1. Raw chunk paging occurs before base64 transport and does not flatten the full preview.
2. Decoded ranges for gzip, zlib/raw deflate, brotli, zstd, identity, malformed/unsupported encoding, binary data, over-eight-layer chains, and 16-MiB output limiting.
3. UTF-8 boundary correction details for `"中a"` with offset `1`, length `2`; zero/oversized length; offset/range/next-offset behavior; retained media type.
4. URI canonical round trip including IPv6 endpoint plus offset/length; defaults; every strict rejection listed above; next URI preserves exact identity/side/representation.
5. Stale revision and retention loss; live no-cache; terminal hit/replacement/LRU eviction/purge/feed-gap behavior.
6. Shared TUI/MCP active/queue/input saturation; no queued-byte wait under active; cancellation/deadline during queue wait and decompression; active permit retained until worker exit; broker disconnect cancellation.
7. Private protocol operation kind, strict required fields, unknown-field rejection, request/result round trip, response operation/instance identity validation, and response byte/frame bound.
8. Public `get_capture` strict schema, exact-run requirement, tool annotations, selected revision/side links, and link/read consistency.
9. `resources/list` empty; templates list has exactly the content template; capabilities omit subscribe/list-changed; resource read returns correct text/blob/MIME/base64/meta and typed errors.
10. Runtime startup shares the exact same admission object between TUI decoding and MCP service. Production-path tests preferred over source-text assertions.

No timing sleeps as race proof. Use barriers/notifies, paused Tokio time, occupied permits, controllable probes, and deterministic change-feed events.

## Files and scope

Expected production files are those listed by Task 10. Modifying closely related child test modules and adding minimal `#[cfg(test)]` adapters is allowed. Avoid mapping/JSON/body-search implementation and unrelated cleanup.

## Required workflow

1. A fresh test-author subagent writes only the RED tests/minimal test module wiring.
2. Main runs the focused RED commands and commits the exact-run RED state.
3. A fresh implementer subagent implements production code only to satisfy the brief/tests, and does not run project commands or commit.
4. Main runs focused/full checks and commits GREEN.
5. Main packages the Task 10 commit range and requests one design/maintainability review and one performance/memory review. Reviewers run no commands and make no edits.
6. Main applies every Critical/Important finding, reruns verification, updates report/work record, and obtains clean re-review.

## Main verification

```bash
cargo fmt --all -- --check
cargo test control::body --all-features
cargo test capture::decode --all-features
cargo test capture::body_work --all-features
cargo test mcp::body --all-features
cargo test runtime::control --all-features
cargo test control_rpc::protocol --all-features
cargo test --test mcp_broker_child --all-features
cargo test --all-targets --all-features
cargo clippy --all-targets --all-features -- -D warnings
```

Strict Clippy may still report deliberately staged later-task or unrelated pre-existing warnings. Task-10-owned findings must be zero.
