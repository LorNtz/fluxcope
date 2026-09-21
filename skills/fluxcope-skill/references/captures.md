# Capture inspection and recording

## Find the relevant exchange

Start with `search_captures`, using the narrowest useful URL, method, status, or sequence filters supported by its schema. Follow its returned cursor only when more matches are needed. Use the selected row's `capture_sequence` as the `capture_id` for detail tools, and carry its `capture_revision` into revision-sensitive reads.

For new traffic, use `wait_for_capture` with the required milestone and an explicit timeout. Set `sequence_min` to the latest observed sequence plus one when earlier captures must not satisfy the wait. Choose `request_seen`, `response_started`, or `exchange_terminal` according to the evidence needed; terminal does not imply a successful response. A timeout means no matching capture was observed in that window, not that no traffic occurred.

The CLI helper defaults to a 60-second deadline. For a five-minute wait, allow protocol overhead with `--timeout-secs 360`.

Read `get_capture` for the selected exchange, supplying `expected_revision` when working from a specific search result. Inspect method, effective URL, status, headers, and lifecycle state before fetching a body. Distinguish request arrival, response arrival, and exchange completion when explaining incomplete traffic.

## Read only the body evidence needed

Use the body tools with the selected instance, `capture_id`, `capture_revision`, and request/response side:

| Question | Tool |
| --- | --- |
| Where is a JSON field? | `find_json_pointers` |
| Which entries match a known pointer shape? | `probe_json_pointer_pattern` |
| Where does text occur in the decoded body? | `search_capture_body` |
| What is the value at an exact JSON pointer or form key? | `extract_capture_body` |

Pointer discovery returns locations and metadata; extract the chosen value afterward. Pointer patterns use RFC 6901-style paths, with each `*` matching one path segment; JSONPath and recursive `**` are unsupported. Inspect the live schema for modes and bounds.

Use a body resource when the task needs a broader view or extraction returns a `resource_uri`. Read the returned `fluxcope://` URI through MCP resources/read or `fluxcope mcp read`. These are broker resources, not web URLs. Prefer decoded content for analysis; use raw content when the original bytes matter.

Follow `_meta.fluxcope.next_uri` for additional pages and retain the reported byte ranges and limitations. Do not derive byte offsets from decoded character counts. Raw resources contain base64 bytes; decoded resources can also return base64 when the content is not UTF-8. A complete page is not proof that the original body was fully captured: check source truncation, decoded output limits, and stream state before claiming a field is absent.

On `capture_revision_conflict`, refresh the same capture and repeat the affected read against its new revision. Keep evidence from different revisions distinct. If the capture was evicted or deleted, report the lost evidence; collect a new exchange only within the user's requested workflow.

## Recording and empty results

Read `get_status` to check recording, retained captures, and relevant limits when results are unexpectedly empty. Check the search query and, when available through the settings UI or source configuration, recording URL filters. Confirm the tested application routes traffic through the selected proxy. HTTPS inspection also depends on the client trusting Fluxcope's public CA certificate; follow the project's client setup instructions if the user requests this configuration.

For an authorized recording change, call `set_recording_enabled` on the selected run and inspect its returned state. Recording-off traffic still forwards and applies mappings; turning recording off does not clear existing captures. Report any recording change made during the task.

Finish with the capture identity, the observed request/response facts, and the evidence supporting the diagnosis. Separate observed behavior from hypotheses, especially when a body is truncated, a response is unfinished, a filter excludes traffic, or retention has removed older exchanges.
