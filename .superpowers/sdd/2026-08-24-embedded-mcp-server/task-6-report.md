# Task 6 Report: Stateless Multi-Instance MCP Broker

## Result

DONE — initial commit `a95e7f4`, review-fix commit `ac8366c`, lifecycle correction GREEN

## RED evidence

Main confirmed the focused RED build failed cleanly on the intentionally missing Task 6 broker, schema, telemetry, registry/probe adapter, and MCP handler APIs.

## Implementation

- Pinned the approved `rmcp` 3.1.4 release with only `base64`, `macros`, `server`, and `transport-io` in production, plus Schemars 1.1. The separate dev-dependency enables `client` and `transport-child-process` only for the in-memory/child protocol tests; production builds do not carry those test transports. The 3.1.4 tagged source was verified: tools use `ToolRouter`, `wrapper::{Parameters, Json}`, `RequestContext<RoleServer>`, structured output schemas, and `ServiceExt::serve(rmcp::transport::stdio())`.
- Added strict closed MCP selector and result schemas for `list_instances`, `get_broker_status`, and `get_status`, including fixed 256-descriptor, 64 KiB, 32-call, 16-probe, and 30-second limits.
- Added one shared 32-permit public-call semaphore and one shared 16-permit liveness semaphore on cloned broker handlers. Saturation is retryable `service_unavailable`.
- Added deterministic bounded discovery. Endpoint selection calls only `RegistryScanner::read_endpoint`; omitted endpoint and run-only selection use `scan_all`. Filesystem work runs in `spawn_blocking`; all scan, permit, probe, stale cleanup, and private forwarding waits share the caller's one outer deadline and cancellation token.
- Added identity-safe stale cleanup under the registry mutation lock. Cleanup reopens and rechecks endpoint, run ID, and socket path before unlinking.
- Added zero/singleton/ambiguous/current/stale-generation resolution. Snapshot reads may omit run ID; mutation/wait/targeted-body/resource requirements cannot. Supplied generations are never retargeted.
- Added live Describe enrichment, deterministic live/diagnostic ordering, malformed descriptor isolation, and bounded ambiguity details.
- Added a fresh one-operation `ControlRpcClient` call for every probe/tool forward. Client calls now race MCP cancellation and the original deadline; dropping the call closes the private Unix connection.
- Added initialized MCP `clientInfo` validation and propagation as the exact `DeclaredClient` for every private call in one tool invocation. Broker process metadata is not used by MCP handlers.
- Added exact read-only/idempotent/closed-world annotations for only the three stateless tools. Server capabilities advertise tools without list-change and advertise no resources, subscriptions, prompts, logging, completions, or experimental features.
- Added broker-only saturating telemetry for admission, probes, cancellations, connection failures, and relayed bytes; status excludes capture, body, query, header, and mapping data.
- Added Unix stdio dispatch through `wirelens mcp`. Broker stdout is passed only to rmcp stdio; broker startup and transport errors propagate through `anyhow` and therefore remain stderr diagnostics.
- Added child-module and child-process tests covering selection/discovery, admission, Describe enrichment, metadata/fresh forwarding, schemas/status, unsupported capability absence, initialize negotiation, stdout JSON-RPC purity, cancellation, and outer deadlines.
- Main ran and committed the initial GREEN as `a95e7f4`, then confirmed the blocking-review RED. The review-fix implementation below has not run commands or created a follow-up commit.

## Modified files

- `Cargo.toml`
- `Cargo.lock`
- `src/app/tests/control.rs`
- `src/lib.rs`
- `src/control_rpc/client.rs`
- `src/control_rpc/client/tests.rs`
- `src/control_rpc/protocol.rs`
- `src/control_rpc/protocol/tests.rs`
- `src/control_rpc/server/tests.rs`
- `src/instance_registry/mod.rs`
- `src/instance_registry/tests.rs`
- `src/runtime/control.rs`
- `src/runtime/mod.rs`
- `src/instance/mod.rs`
- `src/instance/tests.rs`
- `src/runtime/startup_tests.rs`
- `src/mcp/mod.rs`
- `src/mcp/broker.rs`
- `src/mcp/schema.rs`
- `src/mcp/telemetry.rs`
- `tests/mcp_broker_child.rs`
- `.superpowers/sdd/2026-08-24-embedded-mcp-server/task-6-report.md`

## Exact focused GREEN command

```bash
cargo test mcp --all-features
```


## GREEN and Clippy evidence

Main formatted the Task 6 changes and confirmed the focused MCP suite passes under the approved rmcp 3.1.4 release:

```bash
cargo test mcp --all-features
```

Strict Clippy reaches only expected staged APIs reserved for later MCP tasks. Its two non-staged mechanical findings were fixed: the redundant Unix cfg on the already-gated App control child module was removed, and the startup replacement socket is initialized directly at its first value.

## Self-review

Reviewed the complete Task 6 flow against the brief after GREEN. All MCP-facing calls acquire the one shared 32-call permit; all liveness work shares the 16-probe permit pool, caller cancellation, and one outer deadline. Endpoint selection performs only deterministic endpoint lookup, while broad bounded scans are limited to discovery and omitted/run-only selection. Supplied generations cannot retarget, stale removal rechecks identity under the mutation lock, and each forwarding operation creates one cancellation-safe private client connection.

The rmcp 3.1.4 server advertises exactly the three stateless tools with closed input/output schemas and accurate read-only annotations. It advertises no list-change, subscription, resources, prompts, logging, completion, experimental, or extension capability. Initialized client name/version is validated and propagated unchanged to every private call. Broker status exposes only bounded registry/transport/version counters, and broker stdout is owned exclusively by rmcp stdio.

The blocking review concerns below supersede this initial self-review conclusion and are now covered by explicit RED contracts.

## Blocking review RED follow-up

Design and performance review found lifecycle and bounded-work defects in the initial GREEN implementation. The follow-up is back in strict RED: tests only, with no production changes, commands, or follow-up commit.

Added failing contracts for:

- broker startup from a genuinely fresh HOME;
- typed definitive stale-connect failures, with local resource exhaustion and generic `instance_unavailable` explicitly forbidden from authorizing deletion;
- cancellation racing cleanup blocked on the registry mutation lock, so detached work cannot mutate later;
- MCP-disabled proxy support on every platform and explicit rejection of MCP-enabled proxy startup on non-Unix;
- run-ID prefiltering and endpoint-generation conflict detection before private probes;
- bounded per-call probe waves that allow a later targeted call into the fair shared 16-probe pool;
- separate admission retained by cancelled blocking scans until their blocking work exits;
- stale-identity deduplication and one durable registry sync per cleanup batch;
- no result reserialization for estimated byte telemetry and a borrowed descriptor probe contract;
- a bounded registry index that ignores untracked directory junk, deterministic rejection of a 257th descriptor, and indexed enumeration instrumentation.

Exact focused RED commands for Main:

```bash
cargo test mcp --all-features
cargo test control_rpc::client --all-features
cargo test instance_registry --all-features
```

## Blocking review implementation

Implemented the review-fix GREEN candidate without running commands or committing:

- Fresh broker startup now securely initializes the owner-only registry hierarchy and bounded index before opening the scanner.
- The private client attaches a non-serialized typed local transport cause only to `NotFound`/`ConnectionRefused` connect failures. Local file-table exhaustion is `service_unavailable`; generic private RPC availability failures cannot authorize deletion.
- Registry discovery reads a fixed owner-only 64 KiB index of at most 256 tracked descriptors rather than enumerating the instances directory. Publication atomically maintains the index under the mutation lock and deterministically rejects descriptor 257.
- Blocking registry scans and cleanup share a separate 32-permit admission pool whose owned permit remains inside the blocking closure after caller cancellation/deadline.
- Run-only selection filters candidates before liveness work, and endpoint generation mismatches return before probing.
- Each call keeps only one 16-probe wave in flight. The next candidate is enqueued only after a result is drained, allowing later targeted calls to enter the fair process-wide semaphore.
- Probe descriptors are borrowed for private RPC and returned from the owning probe future without a deep descriptor clone.
- Only typed definitive stale-connect failures are collected. Identities are sorted/deduplicated and cleaned in one blocking batch with one registry-directory durability sync.
- Cleanup receives the original cancellation token/deadline and rechecks both under the mutation lock immediately before each unlink, preventing detached destructive work.
- Discovery no longer serializes `ControlResult` a second time to estimate relayed bytes.
- Effective MCP enablement is rejected on non-Unix before proxy listeners/services start, while MCP-disabled proxy startup remains supported.

Exact focused GREEN commands remain:

```bash
cargo test mcp --all-features
cargo test control_rpc::client --all-features
cargo test instance_registry --all-features
```

## Review-fix RED/GREEN evidence

Main confirmed the review-fix RED failed on the intended missing contracts: typed connect classification, the platform guard, bounded registry index and batched cleanup APIs, the borrowed probe contract, and their associated behavior.

After implementation and `cargo fmt`, Main confirmed:

- `cargo test mcp --all-features`: 42 passed.
- `cargo test control_rpc::client --all-features`: 8 passed.
- `cargo test instance_registry --all-features`: 25 passed.

All review findings were evaluated and adopted, including both P2 optimizations: discovery no longer reserializes private results for estimated bytes, and the private probe contract borrows descriptors rather than deep-cloning them. All Critical/Important lifecycle, stale-cleanup, platform, fairness, admission, cancellation/deadline, batching, and bounded-enumeration findings are covered by the GREEN contracts above.

Strict Clippy reaches only expected downstream staged dead-code plus one runtime collapsible-if that later tasks consume; Main explicitly deferred those staged findings.

## Post-commit lifecycle correction

GREEN. A final lifecycle review found and corrected two source-level gaps after `ac8366c`: startup still treated every generic `instance_unavailable` probe result as definitive stale evidence, and the bounded index had no migration/crash reconciliation path.

The correction:

- restricts startup replacement deletion to `is_definitive_stale_connect()`, with a regression proving generic unavailability preserves the current descriptor;
- reconciles valid legacy/crash-orphan descriptors into the bounded index under the registry mutation lock during initialization;
- lets exact endpoint lookup inspect only its deterministic descriptor pathname even when the index does not yet contain it;
- rejects reconciliation deterministically after descriptor 257 and bounds total directory work;
- opens the registry parent directory handle before atomic index rename and syncs that same handle afterward;
- adds legacy migration, exact crash-orphan recovery, over-cap rejection, bounded-work, and held-directory-handle tests.

Exact focused GREEN commands:

```bash
cargo test runtime::startup_tests --all-features
cargo test instance_registry --all-features
cargo test mcp --all-features
```

Main confirmed the final correction:

- `cargo test runtime::startup_tests --all-features`: 14 passed.
- `cargo test instance_registry --all-features`: 30 passed.
- `cargo test mcp --all-features`: 42 passed.
- `cargo check --all-features`: passed.
- `cargo test --test mcp_broker_child --all-features`: passed.
- Final design review: PASS, no blockers.
- Final performance review: PASS, no blockers.