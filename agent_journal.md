
### Task: Repair branch preview CI blockers
Date: 2026/09/19 12:36

- Updated trusted smoke verification to select exactly one log across stable and per-instance layouts, reuse it on restart, and reject missing or ambiguous logs with focused regression cases.

**Result**: Implementation prepared; validation and design/performance reviews pending.

### Task: Validate preview smoke compatibility
Date: 2026/09/19 12:38

- Passed 78 release-tooling tests and full native smoke verification of both stable and MCP feature binaries.
- Design review: no actionable findings.
- Performance review: no actionable findings.

**Result**: Both log layouts pass the trusted verifier; missing and ambiguous logs remain rejected.

### Preview installer: shared protocol
Date: 2026/09/20 10:30

- Began the approved standalone installer on `codex/preview-installer` from master.
- Extracted repository-independent identity, manifest policy, and bounded application archive extraction; retained legacy preview identities and added explicit installer format 2.
- Validation and two independent reviews will follow implementation.

### Preview installer: standalone client
Date: 2026/09/20 11:50

- Added anonymous bounded downloads, provenance checks, private atomic installs, offline launch, YAML-preserving port updates, and uninstall/purge.
- Runtime files are checksum-checked by per-ID launchers before execution. No stable app or settings paths are changed.

### Preview installer: bootstrap and native packaging
Date: 2026/09/20 11:52

- Added per-release shell bootstrap with OS/architecture checks and embedded runtime digests.
- Added native PyInstaller packaging and pinned GitHub CLI downloads; builds capture trusted roots for offline attestation verification.

### Preview installer: CI gates and maintainer entry point
Date: 2026/09/20 11:58

- Split attestation, native installation qualification, and publication into separate jobs. Publisher requires digest-bound installation reports.
- New previews use format 2 with controller-generated scripts and native clients; old manifests remain accepted.
- Updated `just preview-run` to share installation/state verification and release notes/status to show the exact installer command.

### Preview installer: exact-payload qualification
Date: 2026/09/20 12:01

- Added native lifecycle qualification using an HTTPS replay; shipped code keeps its normal signature and digest verification without test-only bypasses.
- Exercises bootstrap corruption, unsupported architecture, isolated/offline launch, settings-preserving reinstall/port changes, expiry, executable tampering, uninstall and confirmed purge.

### Preview installer: lifecycle tests and documentation
Date: 2026/09/20 12:07

- Added boundary/atomicity/YAML/removal tests and recipient/maintainer documentation; added four-platform client packaging checks on PRs.
- Applied initial reviews: reject unsupported architectures before libc probing, bound purge test cleanup, accept static Linux verifier binaries, and skip remote lookup for cached explicit-ID launches.

### Preview installer: review corrections
Date: 2026/09/20 12:11

- Fixed signal forwarding so the launcher reaps its proxy child before releasing the state lock; added a real child-process regression test.
- Separated installer-report validation before App-token creation, retained checks at publication, and simplified port-update branches.
- Adding portable HTTPS CA roots so frozen clients do not depend on build-machine Python certificate paths.
- Initial validation: 88 Python tests, seven cache-policy tests, metadata checks passed before these final review corrections.

### Preview installer: portable trust and release gate coverage
Date: 2026/09/20 12:14

- Pinned certifi HTTPS roots and embedded license notices; explicit SSL_CERT_FILE remains supported for enterprise/replay trust.
- Added identity/expiry/signature-failure and CI permission-order regression tests.
- Launcher termination now forwards signals and kills/reaps an unresponsive child after a bounded grace period.

### Preview installer: final reviewer follow-up
Date: 2026/09/20 12:16

- Both reviewers confirmed the trust/runtime corrections. Fixed their follow-up on normal PTY EOF/EIO during confirmed purge, retaining the deadline.
- Normalized private installation file permissions. Full-stage transfer optimization remains deferred within explicit size bounds.
- Validation: 93 Python tests and seven cache tests passed; actionlint reports only its existing lack of cache-mode schema support.

### Preview installer: native CI packaging corrections
Date: 2026/09/20 12:23

- Native CI identified static manylinux Python and the Intel bootloader’s legacy LC_VERSION_MIN_MACOSX command.
- Pinned standalone CPython 3.11.16 Linux distributions by SHA-256 for packaging; retained execution in glibc 2.28 containers and accepted both Mach-O minimum-version forms.
- Real Apple Silicon frozen client passed anonymous public install using bundled HTTPS roots, offline signed-evidence verification, isolated proxy forwarding and clean shutdown.

### Preview installer: real-terminal purge correction
Date: 2026/09/20 12:26

- Real frozen-client purge found that buffered r+ opening of /dev/tty requires a seekable stream. Split terminal input/output handles so confirmation works on an actual PTY.
- Removed imports made obsolete by the shared-client extraction. Both reviewers approved the native packaging correction.

### Preview installer: final provenance regression coverage
Date: 2026/09/20 12:33

- Pinned the macOS packaging interpreter to CPython 3.11.16, matching the explicit Linux runtime version.
- Added format-2 installer-tampering and mismatched qualification-digest/controller regression checks; removed a duplicate manifest read introduced during extraction.
- Both reviewers approved the real-terminal confirmation correction.

### Preview installer: consistent pinned runtime on all platforms
Date: 2026/09/20 12:38

- The final exact-version check exposed that setup-python’s macOS 3.11 selector supplies 3.11.9 and does not offer 3.11.16.
- Extended the existing pinned standalone-runtime mechanism to macOS ARM64/Intel, so all four frozen clients use checksum-pinned CPython 3.11.16. The workflow interpreter remains separate from the shipped runtime.

### Fix preview artifact selection after failed-job retries
Date: 2026/09/20 13:12

- Live installer qualification exposed GitHub carrying successful jobs into later attempt responses under new job IDs. Resolve evidence from the first appearance of the same completed execution timestamps while preserving the latest job result.
- Added regressions for carried jobs across three attempts and for actual reruns that succeed or fail, preventing fallback to stale successful evidence.

**Result**: Focused correction implemented; review and validation follow.

### Validate preview retry evidence correction
Date: 2026/09/20 13:14

- All 99 Python tooling tests passed, including inherited-job and real-rerun regressions. Checked against actual attempt 1/2 metadata from preview 35490029823.
- Design Review: no actionable findings; newer failures remain authoritative.
- Performance Review: no actionable findings; no extra API calls and constant extra work per job.

**Result**: Ready for CI and merge.

### Correct native installer settings-preservation assertion
Date: 2026/09/20 14:22

- Signed native installer qualification reached offline relaunch on all targets, then wrongly required YAML comments/quotes to survive the application’s existing startup normalization.
- Verify two real unrelated setting values across the explicit port change; retain the unit test for the port editor’s comment preservation. Clarified the application serialization boundary in releasing.md.

**Result**: Harness correction prepared for replay against the exact signed artifacts before CI.

### Validate exact signed installer lifecycle
Date: 2026/09/20 14:27

- Replayed the real signed format-2 payload from run 35492919316 locally on Apple Silicon with the unmodified shell/frozen client and full signature/checksum checks. Complete lifecycle passed, including corrupted downloads/executable rejection, offline expiry launch, settings-preserving port override, uninstall and TTY-confirmed purge.
- All 99 Python tests passed. Design and performance reviews found no blockers; the fixture uses real non-default settings.

**Result**: Harness correction verified against production artifacts and ready for CI.
