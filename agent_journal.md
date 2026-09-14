
### MCP workflow: Add installable managed MCP CLI entrypoints
Date: 2026-09-10 17:43

- Added mcp tools [NAME], mcp call TOOL --arguments JSON|@PATH|-, mcp read URI with bounded --timeout-secs while retaining bare mcp broker.
- Moved rmcp client capabilities into production dependencies and routed client commands through the library dispatcher.

### MCP workflow: Replace mandatory full-config mapping workflow
Date: 2026-09-10 17:43

- Updated built-in MCP prompts to prefer managed CLI fallback, scoped reads and typed preview, preserving exact identities, revision checks and verification boundaries.
- Removed prose/source-text assertions and duplicated test-only prompt inventories; retained actual prompt discovery and error contracts.

### MCP workflow: Update MCP child contract checks
Date: 2026-09-10 17:44

- Included mutation preview in advertised read-only tool contract.
- Removed child protocol test assertions that pinned prompt wording rather than observable behavior.

### MCP workflow: Implement bounded rmcp CLI lifecycle and normalized results
Date: 2026-09-10 17:48

- Added managed child pipes with rmcp ClientInfo negotiation and SDK framing, bounded strict response decoding, request limits, deadline/Ctrl-C handling, stderr streaming, and bounded child cleanup.
- Kept protocol/ID handling in rmcp; used explicit Child ownership instead of TokioChildProcess because its hidden transport ignores malformed JSON and buffers unbounded lines (approved by Main).
- Added bounded JSON object arguments from inline/@file/stdin, compact fresh schema discovery, normalized structured content, domain diagnostic preservation and conservative mutation uncertainty with no retries.

### MCP workflow: Add scoped mapping snapshots and isolated revision-safe mutation previews
Date: 2026-09-10 17:49

- Added shared scoped views that copy only selected presets/tables while retaining gates, original indexes, inclusion flags and omission counts.
- Added typed MCP preview operation sharing existing named-tool mutation conversion and strict private RPC request/result variants.
- Added runtime preview snapshot admission using shared revision/draft/pending guards without transaction tokens or draft locks; bounded worker applies an isolated mutation and uses the same policy compilation as commits.

### MCP workflow: Align MCP agent instructions with selected APIs
Date: 2026-09-10 17:50

- Updated the reviewed local wirelens-mcp skill and tool reference with managed CLI fallback, scoped inspection, typed preview, conditional rereads and precise verification boundaries.
- Recorded accepted suggestions and deliberately deferred persistence/security features in the supplied workflow review document.

### MCP workflow: Defend managed CLI consumer boundaries
Date: 2026-09-10 17:53

- Added tests/mcp_client.rs covering fresh compact discovery, exact normalized instance calls, file/stdin argument ownership, strict JSON/object/oversize errors, protocol/resource failures, and stalled stdin deadlines.
- Added client module tests for structured/mirrored/fallback content normalization, domain conflict diagnostics, malformed/oversized response frames, post-dispatch disconnect uncertainty, and direct child cleanup on initialization timeout.
- Did not run builds, tests, formatters, or linters during concurrent implementation; Main owns integrated verification.

### MCP workflow: Preserve existing MCP domain error codes
Date: 2026-09-10 17:56

- Aligned resource-read regression with broker invalid_argument domain errors rather than relabeling all JSON-RPC errors protocol_error.
- Restored standard non-executable permissions on the newly created Rust integration test source.

### MCP workflow: Expose mapping preview and preserve expanded mapping wire contracts
Date: 2026-09-10 17:56

- Advertised readonly preview_mapping_mutation with exact identity/revision, named-operation schema, bounded URLs and compact result; retained normal MCP Json results and bounded broker conversion.
- Migrated private request encode/decode/result dispatch, snapshot consumers and audit read metadata for preview.
- Added focused tests for scoped disabled duplicate order and omission semantics, URL byte/count bounds, isolated/no-op previews, draft and stale revision conflicts, preservation of unrelated presets on commit, remote-then-local indexes, and strict public/private inputs. Validation commands intentionally deferred to Main.

### MCP workflow: Reject incompatible mapping payloads through explicit private RPC version negotiation
Date: 2026-09-10 18:00

- Bumped private RPC_VERSION to 2 and made registry descriptor RPC version share that constant; descriptor schema and MCP protocol versions remain unchanged.
- Migrated private-wire, broker/schema and benchmark fixtures to the current RPC constant; request/response mismatch tests now explicitly reject old version 1.
- Existing registry version validation prevents old runtime descriptors from reaching incompatible mapping result decoding.

### MCP workflow: Resolve integrated compilation errors
Date: 2026-09-10 18:05

- Used the SDK default/with_cursor API for non-exhaustive pagination arguments.
- Imported private RPC_VERSION into broker test scope after the v2 wire-contract migration.

### MCP workflow: Address client wait and preview performance review
Date: 2026-09-10 18:13

- Raised CLI default deadline to 60 seconds and maximum to 360 so normal 30-second and explicit five-minute capture waits retain setup/response headroom.
- Preview now reuses its compiled policy and full-candidate validation across all requested URLs; existing explanation delegates to the same tracing helper.

### MCP workflow: Capture oversized mapping projection regression
Date: 2026-09-10 18:13

- Added a bounded-memory behavior regression: included oversized rules must fail with resource_limit, while an explicitly omitted oversized table remains inspectable.

### MCP workflow: Bound mapping projection allocations before cloning
Date: 2026-09-10 18:15

- Owner-side scoped projection now admits copied structure/string bytes against the response budget before allocating each preset or rule; omitted tables do not consume the budget.
- Oversized projection regression failed before this fix; existing capped framing still separately enforces encoded JSON/envelope bytes.
- Kept single-URL explanation consuming its validation without an extra diagnostic clone while preview reuses compiled mapping state.

### MCP workflow: Format integrated MCP implementation
Date: 2026-09-10 18:16

- Ran cargo fmt after integrating client, mapping, protocol and review fixes; no unrelated documentation formatting.

### MCP workflow: Resolve independent design and performance reviews
Date: 2026-09-10 18:17

- Design reviewer confirmed the revised CLI deadline default/ceiling addresses normal and five-minute capture waits.
- Performance reviewer confirmed single-compilation preview and bounded pre-clone projection admission address both reported resource regressions.
- Updated local MCP skill/reference with explicit long-wait deadline guidance.

### MCP workflow: Resolve final clippy diagnostic
Date: 2026-09-10 18:18

- Collapsed the client failure/dispatched branch as requested by clippy without changing conservative mutation uncertainty.
- Observed the real default capture wait return matched:false after 30.08 seconds through the updated CLI; explicit --timeout-secs360 accepted.

### MCP workflow: Complete MCP workflow verification and cleanup
Date: 2026-09-10 18:21

- Final cargo test passed: 855 library tests plus 10 integration tests; clippy --all-targets --all-features -D warnings and cargo fmt --check passed.
- Live managed-client smoke verified two-instance isolation, scoped disabled duplicates/order, nonmutating 16-URL preview, stale revision/run rejection, unchanged unrelated settings, and actual remote-then-local HTTP response with 153469-byte whole-body equality.
- Verified 8192-byte resource paging with metadata, normal unmatched default wait, explicit long deadline acceptance, and successful empty discovery after shutdown.
- Both reviewers confirmed their fixes; removed owned smoke processes and temporary fixture directories. Updated supplied review document with decisions, evidence, and private-RPC-v2 restart guidance. HTTPS/TLS and app UI were not exercised.
### Fluxcope rebase: integration change
Date: 2026/09/17 10:34

- Replaying MCP process modes on master: retained Fluxcope package/release metadata and upgraded network dependencies; combined master clap 4.6 with MCP derive support. Preserved master lockfile pending incremental dependency reconciliation.

### Fluxcope rebase: integration change
Date: 2026/09/17 10:36

- Merged isolated configuration modes with Fluxcope startup and master private-filesystem protections. Retained owner-only atomic settings writes, master clap 4.6, and the feature configuration lease dependency; preserved master network lockfile rather than restoring obsolete dependency versions.

### Fluxcope rebase: integration change
Date: 2026/09/17 10:38

- Resolved all five src/ca.rs conflict blocks: retained rcgen 0.14 Certificate/KeyPair APIs, Fluxcope CA DN, byte/PEM accessors and PEM filename confinement; integrated feature serialized initialization, incomplete-authority refusal, bounded 2 MiB reads and unchanged feature privacy/concurrency tests. Reused private_fs ensure/open/write for owner/regular-file/no-follow/single-link protections instead of duplicate CA helpers; kept directory fsync after persistence. No validation/staging/rebase performed. Existing upstream private_fs::open_file already chmods validated files to 0600, so later f19a261 permission migration must account for that existing equivalent behavior; no separate migration introduced.

### Fluxcope rebase: integration change
Date: 2026/09/17 10:38

- Resolved src/runtime/mod.rs conflicts at registry commit: retained ProxyStartup/SettingsSession, bound std listener and actual endpoint identity, endpoint-specific log path (removed invalid settings.path()); converted retained listener to Tokio for Hudsucker 0.25 Proxy::builder, preserved rcgen Issuer/rustls provider connector and Hyper 1 certificate server. Installed aws_lc_rs default provider at runtime startup; no validation/staging performed.

### Fluxcope rebase: integration change
Date: 2026/09/17 10:38

- Resolved src/logging/writer.rs conflict: endpoint-isolated path/rotation retained; open_log_file performs private_fs::ensure_directory then private_fs::open_file in one spawn_blocking closure. Removed redundant feature Unix OpenOptions/directory helper and unreachable weaker non-Unix branch; upstream owner UID, 0700/0600, NOFOLLOW, singly-linked regular-file and NONBLOCK checks now protect per-instance logs. No validation/staging performed.

### Fluxcope rebase: integration change
Date: 2026/09/17 10:39

- Per Main clarification, removed newly introduced global install_default from runtime::run. Master has no global provider install; runtime preserves its explicit aws_lc_rs provider passed to RcgenAuthority and with_rustls_connector without adding process-global side effects. Final runtime smoke remains Main-owned.

### Fluxcope rebase: integration change
Date: 2026/09/17 10:40

- Merged MCP broker module registration with the upstream private_fs module; retained both broker dispatch and owner-only file protection. Continued using master lockfile as the dependency-resolution baseline.

### Fluxcope rebase: integration change
Date: 2026/09/17 10:40

- Merged capture snapshot/timing support without downgrading the modern HTTP/TLS stack. Kept Hyper 1 streaming channel exports together with MCP capture-change feed exports; added the bytes dependency required by capture snapshots.

### Fluxcope rebase: integration change
Date: 2026/09/17 10:40

- Combined capture milestone notification exports with master Hyper 1 body-channel exports while replaying wait_for_capture.

### Fluxcope rebase: integration change
Date: 2026/09/17 10:41

- Merged paged-body capture subscriptions with Hyper 1 body helpers. Kept master private_fs log admission on a bounded blocking task instead of restoring the weaker duplicated file-open implementation.

### Fluxcope rebase: integration change
Date: 2026/09/17 10:42

- Integrated MCP acceptance benchmarks with the Fluxcope crate name while preserving master dist profile and release packaging. Kept benchmark feature gating and master dependency lock baseline.

### Fluxcope rebase: integration change
Date: 2026/09/17 10:43

- Integrated legacy CA permissions migration through master private_fs::open_file, which validates ownership/link safety and chmods the same opened file to 0600 before reading. Retained the feature regression checking unchanged certificate/key bytes after migration.
