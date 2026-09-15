
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

### Task: Prepare the first Fluxcope release from master
Date: 2026/09/14 19:46

- Created an isolated release worktree from master d353c7e. Per the maintainer decision, the first release excludes the unmerged MCP feature.
- Carried the tracked journal across branches and retained release design references.
- Renamed active Rust identities, crate imports, default state paths and fixtures to Fluxcope; existing Wirelens state is not migrated.

**Result**: Identity changes prepared; validation follows with the pinned toolchain.

### Task: Define first-release package and CLI metadata
Date: 2026/09/14 19:49

- Pinned Rust 1.98.0 and added the Cargo package allowlist, licensing and Fluxcope release metadata.
- Added Clap help/version handling before runtime creation; retained the existing TUI invocation.
- Replaced broad dotfile ignores with explicit local-state exclusions so CI configuration can be tracked.
- Added the public README and license files.

**Result**: Package and CLI preparation ready for verification.

### Task: Enforce private first-release application state
Date: 2026/09/14 19:52

- Added shared owner-only directory/file storage, atomic configuration and CA writes, and symlink/hardlink rejection.
- Moved runtime logs into the Fluxcope state directory and made them owner-only.
- Added storage permission regression tests and replaced one real-world hostname fixture with example.com.

**Result**: Release storage requirements implemented; tests and review pending.

### Task: Validate release storage and synthetic fixtures
Date: 2026/09/14 19:53

- Fixed the synthetic hostname filter and test-only CA import after compiler feedback.
- All-ref secret findings were traced to a local stash; the 72-commit master history scan reported no secrets.

**Result**: Starting locked tests for the master-based release changes.

### Task: Provide local just release commands
Date: 2026/09/14 20:00

- Added a standard-library GitHub transport and shared release identity validation.
- Added just doctor/pr/ship/release/prepare/status/recover commands with explicit repository checks, normal pushes, PR reuse, two-stage review and head-locked squash merges.
- Kept local uncommitted files and history untouched by release commands.

**Result**: Local client implemented; CI coordination and behavioral tests follow.

### First release: packaged binary smoke harness
Date: 2026/09/14 20:12

- Added isolated HOME and PTY verification for explicit binaries, local HTTP/HTTPS mapping fixtures, CA persistence/download, private state and terminal restoration.
- Fixed local release reporting to use the reviewed merged version after PR updates; sanitized displayed control characters.

**Result**: harness prepared for local and release artifact verification.

### First release: private storage review fixes
Date: 2026/09/14 20:13

- Design review: validate private configuration before tolerant parsing; reject FIFO storage without blocking.
- Performance review: no material regression found in changed paths.
- Updated smoke platform check to accept the chosen minimum OS version.

**Result**: both Rust review findings addressed; verification follows.

### First release: cargo-dist configuration and macOS cost decision
Date: 2026/09/14 20:16

- Configured cargo-dist 0.33.0 native artifacts and installers without generated CI; preserved unwinding in dist profile.
- Recorded the accepted master-only, no-MCP scope and macOS 15 standard-runner policy.
- Actual binary smoke passed on macOS 15.5 ARM64 with isolated state and verified TLS chain/hostname.

**Result**: build plan resolves all four archives, shell installer and personal-tap formula.

### First release: pinned build tools and Cargo package verification
Date: 2026/09/14 20:18

- Pinned official cargo-dist/release-plz/cargo-deny downloads by SHA-256, and native glibc 2.28 images by digest.
- Added crate allowlist, metadata/digest checks and optional exact-package install.
- Corrected the portable FIFO test to use mkfifoat.

**Result**: reusable CI installation and package verification commands added.

### First release: source and tag authorization gates
Date: 2026/09/14 20:21

- Added shared read-only gates for exact merged PR, intent/manifest/lock agreement, source CI workflow/attempt checks, tag peeling, and crate-first distribution.
- Tag dispatch rejects unknown or moved tags and mismatched workflow refs.

**Result**: CI authorization building block added; negative regression tests will validate boundaries.

### First release: authorized network dependency security upgrade
Date: 2026/09/14 20:23

- Dependency audit identified vulnerable legacy Hudsucker/Hyper/rustls/WebSocket versions. User approved upgrading and adapting in this release.
- Selected Hudsucker 0.25, Hyper 1, rustls 0.23 and rcgen 0.14 while retaining existing HTTP/2 and proxy features.

**Result**: migration started; publishing remains gated on regression tests and dependency checks.

### First release: ordinary CI and network migration validation
Date: 2026/09/14 20:30

- Migrated proxy bodies to Hyper 1 frames with bounded backpressure, preserved trailers/errors and adapted CA generation/download lifecycle.
- Upgraded network dependencies; 339 application tests and clippy passed.
- Added ordinary Linux/macOS CI, package checks, dependency audit, metadata consistency checks and release gate regression tests.
- Retained only the paste maintenance advisory as a documented expiring exception; no vulnerability exceptions.

**Result**: upgraded application and ordinary CI prepared; artifact/release orchestration remains in progress.

### First release: verified distribution artifacts
Date: 2026/09/14 20:33

- Added cargo-dist local/global orchestration, native archive smoke, Mach-O/glibc dependency checks and source/digest agreement across platform evidence.
- Added native manylinux build entry without publishing credentials.
- Allowed webpki root-certificate data under its declared CDLA-Permissive-2.0 license.

**Result**: artifact validation implementation ready for platform CI integration.

### First release: network migration review
Date: 2026/09/14 20:34

- Design review confirmed request association, frame/trailer/error forwarding, CA provider and cancellation behavior.
- Performance review identified CA listener transient errors; restored the prior connection-error retry and cancellable one-second backoff.
- Noted upstream Hudsucker 0.25 accept-loop busy retry under process FD exhaustion; no maintained fixed version was found, and a private fork would not provide matching crates.io source without publishing another dependency. Kept this as an explicit upstream limitation for release assessment.

**Result**: local review issue fixed; upstream resource-exhaustion limitation remains documented.

### First release: platform workflow and maintainer instructions
Date: 2026/09/14 20:37

- Added reusable four-platform archive/installer workflow with native macOS 15 and pinned glibc 2.28 container validation.
- Documented local commands, GitHub authentication, repository/App/tap setup, one-time crate bootstrap, verification and recovery.
- Recorded the upstream descriptor-exhaustion limitation without adding a private dependency fork.

**Result**: reviewable setup guidance available; publishing workflows and remote setup remain unfinished.

### First release: persistent version preparation
Date: 2026/09/14 20:41

- Added release-plz update/set-version configuration and a PR preparation wrapper with explicit intent retention, allowed-file boundaries and expected-head GitHub commits.
- Separated local dirty artifact reports from publishable exact-source evidence.

**Result**: preparation implementation added; race/refresh edge cases require verification before enabling remotely.

### Release gates and installer integrity
Date: 2026/09/14 20:51

- Guarded native release-plz source selection against ambiguous PRs and non-squash ancestry.
- Added a minimal macOS shasum adapter with a fail-closed preflight and real valid/corrupted archive installer checks.
- Local doctor now checks the active GitHub identity instead of failing on an unrelated inactive expired account.

**Result**: implementation added; regression validation follows.

### Exact source publication evidence
Date: 2026/09/14 20:53

- Added bounded CI artifact reads tied to workflow run attempt, clean source SHA and package digest.
- Added read-only source routing/preflight, native Trusted Publishing, create-if-absent tags and bounded dispatch confirmation.
- CI conditionally verifies installation from the exact Cargo package for release or packaging changes.

**Result**: source orchestration implementation added; workflow wiring and regression checks follow.

### Source workflow permission boundaries
Date: 2026/09/14 20:55

- Wired source routing, version preparation, CI preflight, bootstrap pause, stable OIDC publishing, RC-only tags and confirmed distribution dispatch.
- Only the stable publishing job requests OIDC; preparation and tag creation use separately restricted App environments.

**Result**: workflow added; validation and independent review pending.

### Distribution and immutable publication
Date: 2026/09/14 20:58

- Added release PR preview, exact-tag four-platform build, archive/installer attestations and a separate draft publisher.
- Publisher checks native smoke evidence, all asset digests and provenance before uploading; existing mismatched assets are never overwritten.
- Public verification downloads without credentials and checks immutability, version/tag/source and attestation identity.
- Restricted App token selection to explicit API mutations while API reads retain the workflow read token.

**Result**: distribution implementation added; tap workflow, reporting and regression validation continue.

### Personal Homebrew tap publication
Date: 2026/09/14 21:02

- Added a tag-gated shared workflow using a separate tap App credential and cargo-dist formula snapshot.
- Added idempotent formula commits and native four-platform Homebrew installation tests tied to the exact tap commit and binary digest.
- Completed remote master/MCP branch secret scan: 118 commits, no findings (private stash excluded).

**Result**: Homebrew implementation added; complete workflow validation remains.

### Unified release result observer
Date: 2026/09/14 21:04

- Added exact workflow ID/event/ref/source/attempt filtering and a source-commit Release check.
- Observer consumes bounded JSON evidence only; registry, tag, immutable release and tap commit facts are independently checked.
- Failed/cancelled/partial/bootstrap states remain incomplete instead of presenting a green source job as a completed release.

**Result**: reporting implementation added; recovery and integration checks continue.

### Adopted release design and performance reviews
Date: 2026/09/14 21:22

- Recovered preparation branches and PRs left between push/create/label stages; unchanged prepared trees retain their existing head and checks.
- Published native archive checksum sidecars and validated manifest references. Verified replacement builds may reset an unpublished draft as a whole; published assets remain immutable.
- Cached exact-package compilation without sharing temporary source/install directories.
- Distinguished missing evidence from transport failures and observed successful distribution/registry jobs independently of later tap failures.

**Result**: both review agents completed; actionable findings adopted, regression tests follow.

### Registry verification and explicit recovery
Date: 2026/09/14 21:42

- Added exact-version cargo install plus PTY smoke as part of stable installation verification.
- Recovery now derives a concrete operation from exact-source CI, registry, tag, draft/public release and active-run facts; stale plans fail closed.
- Recovery writes are behind a read-only gate and preserve per-version publication identity.

**Result**: orchestration added; final integration corrections and tests remain.

### Recovery integration corrections
Date: 2026/09/14 21:44

- Local recovery displays and confirms the concrete inferred operation, preserving it across remote dispatch.
- Scoped the recovery App token to tag creation; workflow dispatch retains its separate Actions credential.
- Corrected reusable stable installation evidence lookup for normal release and recovery runs.

**Result**: local workflow syntax, Python checks and Rust formatting passed before these final integration corrections; added scenarios will verify them.

### Release failure scenario tests and CI secret scan
Date: 2026/09/14 22:03

- Added regression cases for source identity after master advances, native checkout hazards, tag idempotency, partial publication, active/stale recovery, untrusted observers and installer fallback/fail-closed behavior.
- Added SHA-verified pinned gitleaks to source CI, scanning only reachable candidate history with redacted output.

**Result**: tests added; execution follows.

### Recovery review fixes
Date: 2026/09/14 22:24

- Created the tap report directory in its fresh runner job.
- Added read-only verification of already published immutable assets, with exact-source evidence, so post-publication network failures are recoverable for stable and RC versions.
- Source CI still in progress is now reported as running rather than causing local watchers to stop as failed.

**Result**: review findings adopted; native release-plz and app checks are running independently.

### Failed-job rerun evidence and tap ordering
Date: 2026/09/14 22:27

- Associated artifacts with each producer job execution instead of assuming every successful stage reruns with the newest workflow attempt.
- Serialized all tap updates and rejected downgrading a newer formula. Smoke reporting now creates its own parent directory.
- Real release-plz update succeeded in an isolated validation repo; filtered documentation-only commits so master MCP design documents are not described as shipped MCP functionality.

**Result**: app regression passed 339 tests and strict clippy; release integration tests will exercise these corrections.

### Durable source evidence and preparation bounds
Date: 2026/09/14 22:30

- Preserved verified source/package metadata as an immutable release asset so verification can survive CI artifact expiry.
- Enforced monotonic versions relative to the prior stable release.
- Cached release-plz preparation builds and allowed the local ship waiter to cover first-run dependency compilation.

**Result**: metadata and lifecycle implementation extended; validation follows.

### Interactive first-crate bootstrap
Date: 2026/09/14 22:35

- Added just release-bootstrap with a disposable exact-source checkout, Cargo package/install verification and CI checksum comparison before requesting a token.
- Tokens are accepted only through a hidden terminal prompt, passed only to cargo publish, never persisted with cargo login, and followed by an explicit revocation reminder.
- Renamed GitHub repository to LorNtz/fluxcope while retaining private visibility; updated origin and initialized the agreed public personal tap with README only.

**Result**: helper and account setup guide prepared; no crate, app tag, formula or app release has been published.

### Durable final release result
Date: 2026/09/14 22:37

- Added a separate restricted notes archive job that preserves verified package, immutable asset and exact tap commit facts after raw CI artifacts expire.
- Completion is recorded after archival succeeds; archival failures remain recoverable through report refresh.
- Corrected release note entry detection for release-plz Markdown version links.

**Result**: evidence lifecycle implemented; final native fixture checks, setup and remote CI remain.

### Explicit replacement classification
Date: 2026/09/14 22:40

- Added an explicit, separately confirmed public classification for incomplete versions whose source must be replaced.
- Original release state remains incomplete; preparation accepts the scoped classification and carries the replaced version/source/reason in the next reviewable intent.

**Result**: recovery no longer blocks all future versions when an immutable published source needs a reviewed fix.

### Prepare reviewable application commit
Date: 2026/09/14 22:43

- Aligned current README and repository guidance with Fluxcope identity and the first-release scope.
- Synced design with the reviewed preparation wrapper, installer guard, native checksum sidecars and recoverable publication evidence.

**Result**: application changes are ready for a separate local commit; remote CI and account setup remain pending.

### Reduce stable installation work
Date: 2026/09/14 22:45

- Centralized full public-asset/provenance verification in the Homebrew gate; update/install jobs now fetch only small immutable metadata and let brew download the native archive.
- Retained exact installed binary digest checks, while removing twelve unnecessary cross-architecture archive downloads.
- Limited large temporary build artifacts to seven days; small machine evidence remains for ninety days plus durable release metadata.

**Result**: performance review advice adopted without removing publication checks.

### Real Git preparation and rerun regressions
Date: 2026/09/14 22:49

- Added temporary bare-repository tests for first push, no-op preparation, preserving newer master code/old PR history and stale-lease rejection.
- Added producer-attempt and normal source-CI waiting regressions.

**Result**: integration-oriented tests added; execution follows.

### Release preparation credential isolation
Date: 2026/09/14 22:58

- Split native version/changelog computation from the App-authenticated PR writer into separate jobs; only four bounded metadata files cross the boundary.
- Preserve expected-head leases and reject a changed base or release PR before applying the prepared result.

### Signed durable results and recovery races
Date: 2026/09/14 23:00

- Sign canonical result JSON with the trusted master observer and verify its attestation before using Release notes as durable evidence.
- Regenerate archive inputs on each attempt, avoid rewriting valid archives, and leave actionable partial status if signing never starts; local watching checks the actual observer run.
- Give replacement classification its own run identity so it cannot mark itself as active publication.

### Release retry and bootstrap regression coverage
Date: 2026/09/14 23:05

- Added signed-result tamper, lost-response, cancellation, replacement, bootstrap checksum-before-token and token revocation regression scenarios.
- Guard bootstrap against a pre-existing different stable version; verify exact-package CLI help; permit rebuilding temporary artifacts during retries.
- Old-version Homebrew recovery locates and verifies its historical formula without changing a newer public tap.
- 33 Python release tests passed; native release-plz update with final changelog filters passed in an isolated Git fixture and excluded design-only MCP entries.

### Apply retry and archive repair review fixes
Date: 2026/09/14 23:07

- Accept a completed preparation push on retry only if its full tree and ancestry exactly match the original plan; reconcile its existing PR before finishing labels.
- Permit re-signing damaged archived metadata only when all required live channel evidence independently passes; missing evidence remains a hard failure.

### Reviewed release orchestration verification
Date: 2026/09/14 23:11

- Completed both required design/maintainability and performance reviews, including fixed-plan retries after push/PR/label failures.
- Preserve diagnostic partial state when a signed archive is invalid so recovery can rerun missing independent checks; never use invalid notes as success evidence.
- Synchronized release commands, setup environments, signed evidence retention and bootstrap/recovery instructions with implementation.

### Archive input structure review fixes
Date: 2026/09/14 23:13

- Reject non-object archive/package JSON with an actionable error before signature lookup.
- Refuse unmatched, reversed or duplicated notes markers before remote writes and document the precise manual repair when the display block cannot be identified safely.
- Added real parse/write-path regressions for both boundary cases.

### Final local release CI validation
Date: 2026/09/14 23:14

- Release orchestration tests: 39 passed; actionlint and release metadata checks passed.
- Give the unprivileged Linux container build a writable isolated process home; host credentials are not mounted.
- History secret scan of both remote branches plus the app preparation commit found no leaks; retained detailed privacy findings outside Git for the visibility decision.

### First-release account identities
Date: 2026/09/15 10:37

- Applied the maintainer-confirmed source App login fluxcope-release[bot] and recorded both App IDs in the one-time setup guide.
- Maintainer explicitly authorized publication of the existing repository history and MCP feature branch; the first app version still excludes MCP code.

### Public repository and constrained release environments
Date: 2026/09/15 10:41

- Made LorNtz/fluxcope public after the maintainer confirmed the complete existing history scope.
- Enabled immutable releases, private vulnerability reporting, secret scanning/push protection, squash-only merging and default read-only Actions tokens.
- Created six environments restricted to master or version tags, configured source App 4947987 and tap App 4948017, and supplied precise private-key import instructions.
- Both required reviewers confirmed this identity/configuration change; metadata and all 39 release tests passed.
