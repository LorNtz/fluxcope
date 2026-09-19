# Task 7 Report

## RED intent

The Task 7 tests require deterministic wall/monotonic timing, total-duration terminal ordering, first-write-wins response metadata and timing, one-lock capture snapshots, immutable persistent 64 KiB chunk previews, decoder parity with contiguous input, one-event-per-public-revision multi-subscriber change delivery, explicit 4,096-ring gaps, distinguishable store removals, stored-`Arc` retrieval, persistent-node budget accounting, and byte-for-byte proxy forwarding with populated timing.

## RED evidence

Main confirmed the focused RED state failed exactly on the missing Task 7 APIs: `CaptureSnapshotMode`, atomic snapshot/timing/change-feed support, and chunked preview behavior. Per instruction, this implementation worker did not run formatter, build, lint, or test commands.
Main reported final post-format verification: focused capture tests passed 53/53, focused proxy-handler tests passed 11/11, and `cargo test --all-targets --all-features` passed 539/539. Design review and performance review both returned PASS after the adopted fixes. Strict Clippy reached only intentionally staged downstream APIs plus the known Task 5 runtime `collapsible_if`; Main reported no Task 7 non-staged lint remained.


## Implementation notes

- Added a cloneable capture change feed with monotonic epochs, a fixed 4,096-entry retained ring, independent subscribers, and explicit lag gaps.
- Bound records to their publisher feed at store insertion, publishing admission at the record's current revision so pre-binding mutations are represented. Store retention eviction, explicit deletion, and clear publish distinct removal kinds.
- Added UTC start plus monotonic response/terminal instants, deterministic internal `_at` lifecycle methods, first-response-wins semantics, and total duration derived from the later of two terminal sides.
- Added one-lock `CaptureRecord::snapshot` support for metadata-only and body-preview modes; `summary` now derives from the same locked snapshot state.
- Replaced contiguous body previews with persistent shared full 64 KiB nodes plus one immutable trailing snapshot, ordered chunk iteration, worker-side flattening, live-snapshot immutability, and fixed node-overhead budget charges.
- Updated decoder and non-decoder body display callers to flatten only at admitted worker/display boundaries. Request and response forwarding paths remain unchanged.
- Preserved `CaptureStore::get` as `Option<Arc<CaptureRecord>>` and wired change publication through existing publisher/store/proxy lifecycle paths.
- After Main's first GREEN compile, corrected the chunk buffer import to the direct `bytes` dependency, made `flatten()` return its explicit `Vec<u8>` allocation, and removed unused change-feed reexports while retaining the feed/error/kind API used by this task.
- Updated the pre-Task7 live-body display test to follow the revision-bearing `BodyViewerKey`: an append advances the key, invalidates the old cached entry, and stores refreshed progress under the new key.
- Kept live trailing buffers lazy (`BytesMut::new`) so captures with untouched bodies do not eagerly allocate two 64 KiB blocks.
- Made the one-lock snapshot proof deterministic with a narrow `#[cfg(test)]` hook that runs while `RecordState` is locked: the writer signals immediately before attempting append, the hook proves append cannot complete until snapshot exits, and before/after snapshots assert matching revision, status, and preview.
- Made ordered chunk iteration and flattening O(n) instead of repeatedly walking backward from the persistent tail, and changed decode to consume the first flattened `Vec`; only the rare decode-error fallback flattens the retained preview again to preserve the original raw fallback contract.
- Made the legacy one-side `body_preview(side)` accessor copy only the selected live trailing block rather than constructing both body previews.
- Preserved the retained-prefix invariant after total-memory truncation: later budget availability cannot resume capture with a noncontiguous suffix; observed bytes and revisions continue.
- Snapshot trailing copies deliberately share the record's existing lease: retained bytes are charged once while mutable/canonical, and every preview clone keeps that reservation alive after record eviction. Only persistent full-node control-block overhead adds a distinct fixed charge.
- Dedicated Criterion gates for append/live-snapshot/flatten/feed hot paths are deferred: Task 7's authoritative completion contract names the focused functional capture/proxy commands and prohibits this worker from running benchmark commands. The review nevertheless removed the identified eager allocation, O(n²) walk, unrelated-side copy, and common-path duplicate decode copy; no unmeasured benchmark claim is made here.

## Completion

Status: `DONE`

Final evidence reported by Main:

```text
cargo test capture --all-features                 # 53 passed
cargo test proxy_handler --all-features           # 11 passed
cargo test --all-targets --all-features            # 539 passed
Design review                                      # PASS
Performance review                                 # PASS
```
