# Task 12 Brief: Locate Text and Extract Selected Body Values

## Objective

Add bounded, revision-safe MCP body text location and exact JSON/form extraction on top of the Task 10 shared body scheduler and Task 11 streaming JSON walker. The feature must never return an unbounded parent body, must preserve exact capture targeting, and must keep the active shared body-work lease through decode, search/extraction, result construction, and selected-resource page construction.

## Required public/domain contracts

### Body text search

Add strict `SearchCaptureBodyRequest` and `SearchCaptureBodyResult` wire types.

The request requires:

- capture ID;
- exact capture revision;
- request or response body side;
- a non-empty Unicode query;
- retained match limit, default 10 and range `1..=50`;
- context bytes per side, default 160 and range `0..=1024`.

The result contains:

- capture revision;
- total matches and omitted matches;
- decoded bytes inspected;
- source truncation/stream metadata consistent with Task 10 body replies;
- retained matches in source order.

Each retained match contains:

- exact original decoded UTF-8 byte range;
- original matched spelling;
- bounded UTF-8-aligned context before and after;
- one concrete larger-window decoded-content resource URI whose offset/length cover the returned context and match.

Matching uses Unicode case folding. Folded match ranges must map back to original UTF-8 byte boundaries and preserve original spelling. Multiple and overlapping matches count independently. Search continues after retained results fill so `total_matches` and `omitted_matches` remain exact. Cancellation is checked during matching at bounded progress intervals no greater than 32 KiB of source/folded input work.

An empty query or out-of-range limit/context is `invalid_argument`. Decoded non-UTF-8 or otherwise non-textual content returns typed `body_not_textual` metadata with a concrete raw-content fallback URI; it must not expose binary bytes in an error. Source preview truncation is reported independently from decoded-output limiting.

### Exact extraction

Add strict `ExtractCaptureBodyRequest`, `ExtractSelector`, and `ExtractCaptureBodyResult` wire types.

`ExtractSelector` is a closed tagged enum:

- `json_pointer { pointer }`, using Task 11 strict RFC 6901 parsing and limits;
- `form_field { key }`, with a non-empty key capped at 4 KiB.

The request requires capture ID, exact revision, side, and selector. Extraction runs only on decoded bounded input and rejects source/decode limits according to the existing structured-input policy rather than claiming a partial complete value.

JSON extraction:

- selects exactly one subtree, including root selection with the empty pointer;
- streams selected events directly to deterministic compact JSON;
- never materializes the parent document or selected subtree as `serde_json::Value`;
- supports scalar, object, and array selections;
- validates the complete source document and selection before success;
- checks cancellation between bounded parser/output steps;
- caps selected representation at the existing 16 MiB structured-input ceiling.

Form extraction:

- accepts only `application/x-www-form-urlencoded` decoded input;
- uses standard `+` and percent-decoding with UTF-8 repair semantics consistent with existing form display;
- preserves all matching repeated values in source order;
- serializes selected values as one deterministic compact JSON string array;
- checks cancellation at least between fields and during any large field scan;
- does not retain nonmatching decoded values.

On success, return capture revision, selector kind, selected representation byte size, stream/source metadata, media type, and exactly one of:

- inline UTF-8 selected representation when its encoded byte length is `<= 4096`;
- a concrete first-page selection resource URI when its encoded byte length is `> 4096`.

The 4096-byte boundary is inclusive for inline output. JSON scalar output remains valid compact JSON (strings stay quoted); form output is always a JSON string array.

A missing JSON pointer returns a typed `not_found` error with the longest valid escaped pointer prefix and bounded next child keys/types from Task 11 traversal, never the parent value. A missing form key returns typed `not_found` with no other field values. Errors include capture revision and source-truncation state where available, but never selected or parent body content.

## Selected-value resources

Advertise exactly these three RFC 6570 body templates after both new read paths work:

```text
wirelens://{+proxy_endpoint}/runs/{run_id}/captures/{capture_id}/revisions/{capture_revision}/bodies/{side}/content/{representation}{?offset,length}
wirelens://{+proxy_endpoint}/runs/{run_id}/captures/{capture_id}/revisions/{capture_revision}/bodies/{side}/extract/json-pointer{?pointer,offset,length}
wirelens://{+proxy_endpoint}/runs/{run_id}/captures/{capture_id}/revisions/{capture_revision}/bodies/{side}/extract/form-field{?key,offset,length}
```

Selection resource reads:

- require exact endpoint/run/capture/revision/side;
- re-run the exact admitted extraction against the pinned capture revision;
- page only the compact selected representation, never the parent body;
- default to 8 KiB and accept `1..=64 KiB`;
- enforce UTF-8-aligned offsets and return nearest floor/ceiling offsets for invalid manual boundaries;
- return a concrete next-page URI when bytes remain;
- include `_meta.wirelens` requested/actual range, selected representation size, next URI, endpoint/run/capture revision, stream state, observed/retained bytes, source truncation, and media type;
- use MCP text contents: compact JSON for both JSON and form selections.

URI parsing is canonical and closed: canonical IP endpoint authority including bracketed IPv6, canonical decimal IDs, exact path, no encoded path traversal, one selector query key matching the path kind, optional single offset/length, no unknown/duplicate keys, and no userinfo/fragment. Selector query values use standard URL query encoding and round-trip arbitrary valid pointer/key text.

Live values are recomputed; terminal values may reuse the existing revision-safe decoded cache. Capture/feed changes must not resurrect stale cache entries. A stale revision, evicted capture, decode failure, cancellation, deadline, or source limit maps to the existing stable typed error family.

## Scheduler and memory invariants

- Reuse `BodyJobScheduler::inspect_decoded_body`; do not introduce a second body admission queue or decoder.
- Two active workers, eight queued jobs, and 32 MiB queued input remain shared with TUI and all MCP body operations.
- Queue and active permits survive cancellation until their blocking worker actually exits.
- Runtime/AppRuntime work is limited to short metadata/snapshot commands. Unicode folding, search, form parsing, JSON serialization, result budgeting, and selected page construction stay in bounded blocking workers.
- No result/resource page retains a full 16 MiB decoded backing buffer after the active lease exits.
- Search retained matches are capped at 50 and context at 1 KiB per side. Selection tool inline output is capped at 4 KiB; resource pages own at most 64 KiB plus bounded metadata.
- Private results remain under the existing 8 MiB RPC response limit; MCP outputs use closed schemas.

## Strict private RPC and MCP surface

Add private operations/results for:

- `search_capture_body`;
- `extract_capture_body`;
- selection resource reads (one typed operation or closed variants is acceptable if wire schemas stay strict).

Add public MCP tools with the same names. Both tools:

- require an exact endpoint/run selector, capture ID/revision, and side;
- are read-only, idempotent, non-destructive, and closed-world;
- share the one ordinary-call deadline and disconnect cancellation;
- return the freshly probed instance identity from the private response;
- do not serialize captured values into telemetry or logs.

Update the child-process MCP contract to include the two tools and exactly three resource templates. `resources/list` remains empty and resource subscriptions remain disabled.

## Required RED coverage

Tests must be written and observed failing before production implementation. At minimum cover:

1. text search: ASCII case folding, Unicode folding with original-byte range mapping, overlapping matches, retained/omitted totals, context clipping/alignment, larger-window URI, empty/out-of-range validation, cancellation, source truncation, binary fallback;
2. JSON extraction: root/scalar/object/array, escaped pointer, deterministic compact bytes, missing-pointer longest prefix/next hints without parent values, malformed/depth/size/cancellation errors, source truncation, 4096/4097 inline boundary;
3. form extraction: repeated keys/order, plus/percent UTF-8 decoding, empty value, missing key without sibling values, wrong media type, cancellation inside one large field, 4096/4097 boundary;
4. selection paging: exact bytes, page boundaries, UTF-8 invalid-offset hints, next URI, stale revision, canonical parser including IPv6 and traversal rejection, metadata completeness;
5. private RPC: strict operation/result round trip and unknown fields;
6. broker/tool schemas: required endpoint/run/capture/revision/side/selector, bounded limits, read-only annotations, exact result dispatch;
7. process-level MCP: sorted tool list includes both tools, templates list has exactly three entries, each new tool has a focused required-field/schema assertion.

Every test must assert observable production behavior with hand-derived literals. No source-text tests, mock-only assertions, or production test seams.

## Execution order

1. Add all Task 12 behavior/schema tests only.
2. Main runs `cargo test search_capture_body --all-features` and `cargo test extract_capture_body --all-features` and records RED due to missing Task 12 production behavior.
3. Implement domain types and pure search/extraction/paging.
4. Integrate shared scheduler and strict private RPC.
5. Integrate MCP tools, resources, schemas, and child contract.
6. Main runs focused tests, formatting, strict Clippy, and all-target/all-feature tests.
7. Commit one reviewable Task 12 implementation.
8. Run design/maintainability and performance/memory reviews; fix every Critical/Important finding within five rounds and re-verify.

## Completion

Task 12 is complete only when RED was observed, all focused and repository tests pass, strict Clippy and formatting pass, one implementation commit exists, and both required specialist reviews are clean.
