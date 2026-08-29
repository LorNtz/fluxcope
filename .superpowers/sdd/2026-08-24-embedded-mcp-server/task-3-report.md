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
