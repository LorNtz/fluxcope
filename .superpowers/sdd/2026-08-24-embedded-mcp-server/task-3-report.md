# Task 3 Report: Secure Multi-Instance Startup

## Result

Implemented secure instance identity, registry publication/scanning, shared CA initialization, per-endpoint log files, and bind-first proxy startup.

## RED evidence

Main ran `cargo test instance_registry --all-features` after the test-first change. Compilation failed on the intentionally missing `InstanceIdentity`, `RunId`, endpoint hashing, registry publisher/scanner and metadata validation, endpoint log-path derivation, and proxy listener binding interfaces.

## Implementation

- Added canonical 128-bit base64url run IDs, canonical local proxy URLs, timestamps, and stable SHA-256 endpoint names.
- Added owner-only registry directories, run-specific Unix sockets, atomically published descriptors, identity-safe cleanup, secure no-follow opened-handle validation, deterministic bounded scans, targeted endpoint reads, and bounded diagnostics.
- Added exclusive owner-only CA initialization locking and atomic owner-only authority outputs.
- Added endpoint-hashed log destinations with owner-only directories/files and independent rotation.
- Replaced check-then-bind proxy startup with one retained nonblocking `TcpListener` passed to Hudsucker through `with_listener` before descriptor publication.

## GREEN evidence

Main ran the focused suites after formatting:

- `cargo test instance_registry --all-features` — 17 passed.
- `cargo test ca --all-features` — 106 passed.
- `cargo test logging --all-features` — 8 passed.
- `cargo test runtime::tests --all-features` — 6 passed.

The Darwin listener regression uses bounded condition polling around nonblocking `accept`, preserving production bind-first behavior without production sleeps.

## Self-review

Reviewed Task 3 against the brief after GREEN. Descriptor reads use `O_NOFOLLOW | O_CLOEXEC` and validate the opened handle; matching entries are sorted before the 256-entry cap; publication uses same-directory `0600` temporaries, flush/sync, rename, and directory sync; cleanup checks endpoint plus run ID before removing published paths; CA initialization rechecks state under the exclusive lock; and runtime retains the actual prebound listener and registry publisher for their required lifetimes. No unresolved Task 3 correctness or security concerns were found. Temporary downstream-consumption warnings for staged registry/control APIs are expected and were accepted by Main.


## Fix round 1 RED intent

Added test-first regressions for the five review findings before production changes:

- registry mutation locking is exclusive, and cleanup observation must occur while the same lock spans identity reread through unlink;
- descriptor-name retention is instrumented through a bounded 256-name set while omitted entries are counted;
- per-user Wirelens home derivation is platform-independent and rejects a missing home instead of falling back to the current directory;
- publication refuses to replace an existing endpoint descriptor before Task 5 performs a liveness probe;
- a retained bound-listener lease continues owning the endpoint after the proxy-task listener exits and releases it only at instance shutdown.

Awaiting a focused RED run; no production changes are included in this pass.


## Fix round 1 implementation

Implemented the review corrections behind the RED contracts:

- publication and cleanup now share a persistent owner-only, no-follow registry mutation lock; cleanup securely rereads identity while holding the lock through unlink;
- publication uses atomic no-clobber linking and returns `AlreadyExists` for an existing endpoint descriptor so Task 5 owns replacement after liveness probing;
- `scan_all` streams matching paths through a max-256 lexicographic heap and computes omissions from the full observed count;
- Wirelens home resolution moved to the cross-platform instance layer and runtime uses it on every platform;
- runtime retains a bound-listener lease while Hudsucker receives a cloned listener, so endpoint ownership outlives proxy-task exit through instance shutdown.

Awaiting focused GREEN verification; no commands or commit were run in this fix pass.


## Fix round 1 GREEN evidence

Main formatted the fix and ran the focused suites successfully:

- registry: 21 passed;
- CA: 107 passed;
- logging: 8 passed;
- runtime: 6 passed.

The review findings are closed: registry mutations are serialized without lock-file replacement, scans retain bounded deterministic names, all platforms use per-user log roots, publication cannot clobber an existing endpoint descriptor, and listener ownership remains leased through instance shutdown.
