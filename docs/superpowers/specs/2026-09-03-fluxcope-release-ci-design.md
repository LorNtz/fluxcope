# Fluxcope v0.1.0 Release and CI Design

## Status

Originally drafted on 2026-09-03; architecture and SOP revised on 2026-09-08 following the decision to adopt release-plz plus cargo-dist. A reviewed release-PR merge now authorizes an ordinary stable release. The crate publishes before the binary distribution completes; personal tag signing is no longer a routine release prerequisite. This remains a design, not implemented automation or authorization to publish anything now.

## Problem

The application is featureful enough for a public v0.1.0 release, but the repository does not yet have a stable public identity or a reproducible publication pipeline. The current Cargo package and executable are named `wirelens`; release metadata, public documentation, licenses, CI workflows, package policy, artifact generation, and rollback rules are absent.

A release is unusually security-sensitive because Fluxcope is a man-in-the-middle proxy that generates a local certificate authority, can expose the CA certificate over the LAN, listens on all IPv4 interfaces by default, and exposes optional MCP control. Publication must make those properties explicit and must not accidentally include local CA material, logs, runtime state, or private planning files.

## Goals

- Publish the first public release as Fluxcope v0.1.0.
- Make the existing repository public as `LorNtz/fluxcope`.
- Use one exact machine identifier, `fluxcope`, across the crate, executable, paths, protocol identifiers, artifacts, package managers, and documentation.
- Distribute native macOS and Linux binaries through GitHub Releases, a shell installer, and Homebrew.
- Publish installable source through crates.io.
- Make every release traceable to one immutable version tag, the exact source commit, and its reviewed release PR.
- Gate changes with reproducible locked CI on macOS and Linux.
- Generate a committed changelog from Conventional Commit squash history.
- Minimize persistent secrets by using crates.io Trusted Publishing and GitHub artifact attestations.
- Document the MITM threat model, unsigned macOS status, current network exposure, and safe CA removal.

## Non-goals

- Windows artifacts or Windows support in v0.1.0.
- Debian, RPM, AUR, Nix, Snap, or container packages.
- macOS code signing or notarization for v0.1.0.
- Automatic application self-update.
- A package-manager-independent update checker.
- A proxy ACL, authentication layer, or changed bind default as part of release work.
- Compatibility aliases for the `wirelens` executable, paths, or MCP resource URIs.
- Automatic migration of pre-release `.wirelens` state.
- Publication from an arbitrary ordinary commit on the default branch; only an eligible release-PR merge authorizes the normal stable path.
- Personal signing-key provisioning in CI or mandatory maintainer-signed tags for routine releases.
- A mandatory coverage percentage or second-person approval gate.

## Naming Decision and Preliminary Screening

The public name is **Fluxcope**, with lowercase machine identifier `fluxcope`.

Preliminary knockout screening found no exact crates.io package, Homebrew formula, GitHub repository, prominent software product/company, or indexed federal trademark for `Fluxcope`. The nearest relevant software names found were `FlowScope` and `Fluxscape`; neither is exact. This screening is not a legal trademark-clearance opinion. A comprehensive professional search remains appropriate before significant commercial investment.

The release will use:

- product: `Fluxcope`;
- crate and library package: `fluxcope`;
- executable and CLI command: `fluxcope`;
- repository: `LorNtz/fluxcope`;
- Homebrew formula: `fluxcope` in `LorNtz/homebrew-tap`;
- default persistent directory: `~/.fluxcope`;
- MCP resource scheme: `fluxcope://`;
- artifact prefix: `fluxcope`.

## Architecture Decision

### Selected: release-plz plus cargo-dist

The maintainer's routine is **review the bot's release PR, finish checks, merge, and verify publication**. There is no local preparation command or manual tag push for an ordinary stable release.

| Concern | Single owner |
| --- | --- |
| Propose version, update Cargo manifests/lockfile, and maintain committed changelog | release-plz |
| Authorize the proposed stable version and exact reviewed changes | Maintainer merging the release PR under branch rules |
| Publish the source crate and create its version tag | release-plz |
| Build native archives, shell installer, checksums, and attestations | cargo-dist |
| Create/populate/publish the GitHub Release and generate/update Homebrew metadata | cargo-dist |
| Source quality, package validation, and merge policy | CI and GitHub rules |

`.github/workflows/release-plz.yml` maintains the release PR and, only for an eligible stable release-PR merge, publishes through release-plz. `.github/workflows/release.yml` remains cargo-dist's generated tag-triggered artifact workflow. Configure release-plz not to create GitHub Releases; cargo-dist does not independently change versions, publish crates, or create a competing stable tag.

The release-plz steps use short-lived installation tokens from a repository-scoped GitHub App. Its PR/tag events can trigger the required CI and cargo-dist workflows. Do not substitute the default `GITHUB_TOKEN` and assume downstream events will behave identically. Do not add a `release: published` crates workflow: source publication already belongs to release-plz.

### Accepted tradeoffs

- A release-PR merge is the publication boundary, rather than a later personally signed tag push. The approval record is the protected PR/merge plus workflow provenance, not a claim that a bot tag carries the maintainer's signature.
- Source publication can succeed while a platform build fails. crates.io may therefore contain a version before GitHub binaries or Homebrew are ready. This is an accepted partial-publication state, not an all-channel atomic transaction.
- Version proposals need human review. CLI flags, configuration semantics, MCP contracts, and network defaults can break users without a detectable Rust library API break.
- The bot requires a GitHub App key and scoped permissions. That is additional one-time automation setup in exchange for fewer routine manual steps.
- First-crate bootstrap and GitHub-only release candidates remain explicit exceptions. They do not turn ordinary stable releases back into a manual-tag workflow.

### Superseded: manual-tag, binary-first publication

The earlier design used a local preparation command, a personally signed tag, and a later standalone crate publisher. It offered a separate signing-key authorization boundary and delayed source publication until binary success, but required more maintainer operations. Remove that preparation/publisher design rather than keeping two supported stable release paths.

### Rejected: fully custom artifact workflows

Custom artifact matrices would reimplement target selection, archive layout, checksums, installer generation, uploading, and Homebrew metadata. release-plz does not build binaries; cargo-dist remains the conventional owner of that work. Combining them is not inherently conflicting when each concern above has one owner.

## Public Platform Contract

v0.1.0 publishes these targets:

| Target | Minimum runtime | Delivery |
| --- | --- | --- |
| `aarch64-apple-darwin` | macOS 13 | GitHub, shell installer, Homebrew |
| `x86_64-apple-darwin` | macOS 13 | GitHub, shell installer, Homebrew |
| `aarch64-unknown-linux-gnu` | glibc 2.28 | GitHub, shell installer, Homebrew |
| `x86_64-unknown-linux-gnu` | glibc 2.28 | GitHub, shell installer, Homebrew |

Artifacts are architecture-specific; v0.1.0 does not create a macOS universal binary. Linux artifacts are dynamically linked GNU binaries built against pinned glibc 2.28 sysroots or container images identified by digest; release verification rejects any required `GLIBC_*` symbol newer than `GLIBC_2.28` and executes each target in a glibc 2.28 userspace. macOS builds set a deployment target of 13 rather than inheriting an implicit runner default. Binary publication is blocked until both architecture-specific binaries have also executed on native macOS 13 Intel and Apple Silicon environments; declaring a deployment target alone is insufficient. This gate can fail after the source crate has already published.

macOS artifacts are unsigned and unnotarized in v0.1.0. The README and each stable release note must state this adjacent to installation instructions. They must explain checksum and attestation verification and the narrow Gatekeeper override needed for a verified downloaded binary. They must not suggest globally disabling Gatekeeper.

## Clean Rename Contract

The first public release performs one clean cutover. There is no compatibility period because no public version exists to support.

The rename covers:

- Cargo package name and Rust crate references;
- binary name and Clap command identity;
- application title, logs, errors, help, version, and user-agent/server identity where present;
- default settings, certificate, logging, runtime registry, descriptor, lock, and control-socket paths;
- process discovery and ownership markers;
- MCP broker examples, prompts, resource templates, and `fluxcope://` URIs;
- tests, fixtures, configuration examples, benchmark imports, and command snippets;
- GitHub repository links and workflow/package identifiers;
- archive, installer, formula, and release names.

Existing `.wirelens` data remains untouched. Fluxcope does not read, move, copy, delete, or merge it. Pre-release users must configure Fluxcope again and may manually remove old development state after confirming it is no longer needed. No alias executable, fallback directory, legacy URI parser, re-exported crate alias, or deprecated path is shipped.

The active-identifier scan covers the checked-out tracked tree, generated package contents, workflows, and release assets, but excludes preserved `.git` history. It must find no active external identifier containing `wirelens`. A historical mention is permitted only in a changelog migration note that explicitly identifies the pre-release name; source identifiers, commands, paths, URI schemes, package fields, workflows, and installation examples may not use it. Git history is audited separately for secrets and prohibited private material.

## Cargo Package Contract

`Cargo.toml` will declare:

- `name = "fluxcope"`;
- `version = "0.1.0"` for the first release;
- `edition = "2024"`;
- `rust-version = "1.98"`, matching the pinned 1.98.0 release toolchain selected on 2026-09-03;
- `license = "MIT OR Apache-2.0"`;
- a concise description;
- `repository` and `homepage` pointing to `https://github.com/LorNtz/fluxcope`;
- `readme = "README.md"`;
- relevant keywords and categories;
- an explicit package `include` allowlist.

The source allowlist includes only files needed to build, understand, and license the package: Cargo manifests/lockfile, Rust sources, README, changelog, and both license notices. Cargo-generated package members such as normalized `Cargo.toml`, `Cargo.toml.orig`, and `.cargo_vcs_info.json` are expected outputs rather than source allowlist entries; CI validates their contents separately. The package excludes certificates and keys, logs, runtime/config state, task files, local-only requirement/work records, benchmark output, design/planning documents, and build output.

`cargo package --locked` is a CI gate. CI extracts `target/package/fluxcope-<version>.crate` into a clean directory, runs `cargo install --locked --path <extracted-directory> --root <temporary-root>`, and executes the installed `fluxcope --version` and `fluxcope --help`. The packaged source, not the working directory, must build successfully.

The release profile may enable stripping and thin LTO after measuring build behavior. It will retain panic unwinding because terminal restoration and supervised service shutdown are correctness requirements. Release work will not set `panic = "abort"` merely to reduce binary size.

## CLI Contract

Clap provides noninteractive identity endpoints:

- `fluxcope --version` prints `fluxcope 0.1.0` for the first release;
- `fluxcope --help` succeeds without entering the terminal UI;
- `fluxcope mcp` remains the stdio broker command.

The CLI version is sourced from Cargo package metadata rather than duplicated in Rust source. Package and archive smoke tests assert the exact tag version.

## Continuous Integration

### Toolchain

Commit `rust-toolchain.toml` pinned to Rust 1.98.0 with `rustfmt` and `clippy`. CI uses the committed pin; it does not silently follow `stable`. `Cargo.toml` declares `rust-version = "1.98"` so the package contract and runner contract agree.

Dependabot opens grouped weekly updates for Cargo dependencies and GitHub Actions. Updating the Rust pin is an explicit reviewed change with full CI.

Pin exact versions of release-plz, cargo-dist, cargo-deny, and the full-history secret scanner in committed automation. Changelog generation uses the git-cliff functionality integrated into the pinned release-plz version; do not install a second independently versioned changelog generator. Install tools from checksum-verified registry sources with exact versions and locked dependencies, or verify checksums of downloaded binaries. Tool-pin updates are ordinary reviewed changes.

### Pull-request workflow

`.github/workflows/ci.yml` runs on pull requests and pushes to `master`. Default permissions are `contents: read`; jobs receive no release credentials.

Required jobs have stable names suitable for repository rules:

1. **Source quality** on Ubuntu:
   - `cargo fmt --all -- --check`;
   - `cargo clippy --all-targets --all-features --locked -- -D warnings`;
   - `cargo deny check` using committed `deny.toml` policy.
2. **Tests (Linux)** on Ubuntu:
   - `cargo test --all-targets --all-features --locked`.
3. **Tests (macOS)** on macOS:
   - the same locked full test command.
4. **Package** on Ubuntu:
   - verify Cargo metadata and the package include set;
   - run `cargo package --locked`;
   - extract the generated `.crate`, install it with `cargo install --locked --path` into a temporary root, and execute that installed binary;
   - execute exact help/version smoke checks;
   - validate the cargo-dist plan and ensure the committed generated workflow matches configuration.
   - validate `release-plz.toml`, exact Cargo/lockfile/changelog agreement for a release PR, accepted version syntax, and the stable/RC publication gates.
5. **PR title** for pull requests:
   - require a Conventional Commit title compatible with squash merging.

CI caching may cache registry/git data and target outputs by lockfile, toolchain, target, and profile. A cache miss must only affect speed. No correctness step may be skipped because of a cache hit.

### Dependency policy

Commit `deny.toml` to define:

- security advisory handling and documented temporary exceptions with expiry/context;
- allowed licenses compatible with this distribution;
- approved registries and Git sources;
- duplicate-version policy focused on actionable expensive or security-sensitive duplicates rather than blanket failure.

`cargo-deny` replaces a redundant separate `cargo audit` job unless a future audit requirement adds distinct value.

### Action integrity and permissions

Every GitHub Action `uses:` reference, including GitHub-owned actions, is pinned to a full commit SHA with a version comment. Pin the release-plz executable version as well as its action wrapper. Checkouts use full history, an explicit event commit, and `persist-credentials: false`; credential-bearing steps receive tokens only when needed.

- A private GitHub App installed only on `LorNtz/fluxcope` supplies short-lived installation tokens. Its permissions are Contents read/write and Pull requests read/write; use modern tag rules with a creation-only exception for the App, not general repository-administration permission or a `master` bypass.
- Store its public App ID in repository variable `RELEASE_APP_ID`. Store `RELEASE_APP_PRIVATE_KEY` separately in the `release-pr` and `crates-release` environments, rotating both copies together. The PR job needs no crates.io or tap credential; the publishing job needs no tap credential. Where supported, mint the publisher's installation token with Pull requests read-only.
- The job running `release-plz release` uses environment `crates-release` and job-level `id-token: write`. Use release-plz's native crates.io Trusted Publishing support. Do not configure `CARGO_REGISTRY_TOKEN` or run `rust-lang/crates-io-auth-action` in that job.
- Only GitHub asset publication receives `contents: write` through the default workflow token; PR/tag writes use the scoped App token. OIDC is limited to the crates publisher and attestation jobs; `attestations: write` is limited to attestation jobs.
- Store the tap-only fine-grained token as `HOMEBREW_TAP_TOKEN` in `homebrew-release`; only the tap-update job receives it.

PR code runs with read-only permissions and no release credentials. Bot PR creation runs trusted code from `master`, not code checked out from a proposed release branch. Do not use `pull_request_target` to run unreviewed PR code with these secrets.

## Repository Policy

After the public-history safety gate, rename the existing repository to `LorNtz/fluxcope` and make it public. Configure:

- `master` as default; PRs and the five stable CI checks above are required;
- release PRs must be current with `master` before merge; a changed head requires fresh checks and renewed smoke sign-off;
- squash merge only, with the PR title as the default squash subject and the accepted Conventional Commit subject pattern enforced;
- no mandatory second-person approval and no bot auto-merge of release PRs;
- block direct/force pushes and deletion of `master`; the release App has no bypass for this branch;
- reserve `release-plz-` branch names for actual stable release preparation, including documented maintainer corrections, not unrelated feature work;
- restrict creation of `v*` tags to the release App and the maintainer's documented bootstrap/RC/missing-tag recovery exceptions;
- use separate tag rules to block updates/deletions for everyone, including the App; a creation exception must not allow retagging;
- enable GitHub immutable releases and private vulnerability reporting/secret scanning.

`release-pr` and `crates-release` permit deployments from `master` only, not tag refs or feature branches. The crates.io Trusted Publisher is bound to repository `LorNtz/fluxcope`, workflow **`release-plz.yml`**, environment **`crates-release`**. These differ from the former tag-triggered publisher. Neither environment contains a permanent registry token.

`homebrew-release` permits selected `v*` tags only and denies branches. Its job independently rejects prerelease versions. No routine environment approval is required beyond the maintainer's PR merge; if an organization imposes one, inspect the exact commit before approving it.

Repository variable `CRATES_BOOTSTRAPPED` starts as `false`. It gates automatic source publication and stable tag creation, but not creation of the initial release PR. Set it to `true` only after the first-release bootstrap below succeeds. Missing or malformed values fail closed.

## Changelog and Release Preparation

release-plz owns generation of the committed `CHANGELOG.md`. Keep its configuration in `release-plz.toml` using `[changelog]`; the standalone `cliff.toml`/local release-preparation command from the earlier design is not part of this architecture. Upstream currently deprecates the separate `changelog_config` path.

The baseline configuration contract is:

```toml
[workspace]
release_always = false
git_release_enable = false
git_tag_enable = true
git_tag_name = "v{{ version }}"
pr_branch_prefix = "release-plz-"
pr_name = "chore(release): prepare release"
pr_labels = ["release"]
dependencies_update = false
publish = true
publish_allow_dirty = false
publish_no_verify = false

[changelog]
protect_breaking_commits = true
tag_pattern = "^v[0-9]+\\.[0-9]+\\.[0-9]+$"
```

Validate this configuration against the exact selected release-plz version before implementation acceptance. `release_always = false` is essential: its upstream default otherwise attempts publication on ordinary main-branch commits. Branch-prefix recognition is not an authorization mechanism by itself; the wrapper validates merged-PR identity and commit association.

Normal `master` changes run `release-plz release-pr` to create or refresh the release PR. `workflow_dispatch` on `master` offers a **preparation-only** refresh when needed; it must not invoke the publisher. Serialize PR updates. Do not continuously prepare a next-version PR while a prior version is only partially published or the current manifest is an RC.

The bot proposes a version but the maintainer owns CLI/config/MCP compatibility judgments. Use `release-plz set-version X.Y.Z` on the preparation branch for an explicit correction, refresh the workspace lockfile, and review all three metadata files. Human changes may cause the bot to replace/close its old PR during regeneration; do not assume a remembered PR number or reviewed head remains current.

v0.1.0 includes all relevant reachable history and links to its tag rather than a nonexistent earlier release. Later stable entries compare against the preceding stable tag; GitHub-only RC tags do not become the stable comparison base. RC preparation uses the same generator followed by `set-version`, and stable promotion consolidates that version's RC sections into a reviewed stable section. A stale changelog or unintended lockfile dependency refresh blocks merging.

## Release Trigger and Validation

### Stable orchestration

`.github/workflows/release-plz.yml` runs on pushes to `master`. Its preparation and publication jobs have separate credentials and concurrency controls. Publication uses a non-cancelling concurrency group; never cancel an active upload because another commit arrived.

Before any OIDC exchange or call to `release-plz release`, the publisher must:

1. require `github.repository == "LorNtz/fluxcope"` and the `master` push event, not a PR event or preparation-only dispatch;
2. require `CRATES_BOOTSTRAPPED == "true"` and an exact stable `X.Y.Z` Cargo version; an RC never enters this job;
3. verify the event SHA is the squash merge commit of a merged same-repository release PR targeting `master`, with the reserved branch prefix and expected release label, authored by the configured App or authorized maintainer;
4. check out that exact event SHA, not the latest remote branch tip; require matching Cargo, lockfile, and changelog versions and no uncommitted changes;
5. wait for the required push CI at that exact SHA to succeed and run locked package preflight; absent, failed, or mismatched checks block publication;
6. record the source SHA, PR identity, intended tag, and expected packaged `.crate` digest in the run evidence before publishing;
7. use release-plz to publish and create the stable tag with the App token, native OIDC, and GitHub Release creation disabled;
8. require the resulting tag to resolve to that source SHA and the published registry checksum to match the expected package.

With squash merging, release-plz's source-selection behavior needs an explicit regression scenario: a later ordinary `master` merge must not change which release commit is packaged or tagged. Do not silently fetch and release a newer branch tip. The bot may not merge its own release PR.

### Artifact authorization

The App-created tag triggers `.github/workflows/release.yml` through a real tag push. Accept only stable `vX.Y.Z` and RC `vX.Y.Z-rc.N` refs. The workflow must:

- resolve the tag to its exact commit, require reachability from `master`, and check Cargo/lockfile/changelog agreement;
- validate the exact merged same-repository PR and source commit: stable/bootstrap tags require a labelled release PR under the reserved prefix, while RC tags require a maintainer's merged `rc/X.Y.Z-rc.N` PR with that exact prerelease version; stable releases additionally require the matching registry checksum;
- use the immutable tag/source digest in artifact attestations;
- reject tag updates and never infer success from a tag merely existing.

Normal stable tags need not be personally signed or annotated; use the tag's peeled commit identity regardless of tag-object representation. No maintainer private signing key is uploaded to CI. Bootstrap, RCs, and verified recovery of a missing tag are the only documented manual tag-creation exceptions.

## Artifact Workflow

For a valid authorized tag, cargo-dist builds all four target archives. Each archive includes the target executable and public read/license material. Stable source publication belongs to release-plz and may already have succeeded; a later binary failure does not roll it back. The artifact workflow:

1. builds with locked dependencies, the pinned toolchain, macOS 13 deployment targeting, and Linux glibc 2.28 sysroots or container images pinned by digest;
   - for a stable tag, verify that the exact intended crate exists with the matching packaged-source checksum; a missing or mismatched registry result blocks GitHub binary publication, not permission to publish the crate from cargo-dist;
2. extracts every finished archive into a clean temporary directory and runs the packaged binary's exact `--version` and `--help` checks;
3. rejects Linux binaries whose required symbol table contains a `GLIBC_*` version newer than `GLIBC_2.28`, then executes each architecture in a glibc 2.28 userspace;
4. executes both macOS architectures on native macOS 13 environments and inspects each Mach-O minimum-OS load command;
5. generates the POSIX shell installer;
6. generates one SHA-256 manifest covering the four target archives and the installer; the manifest does not cover itself, GitHub-generated source archive links, or attestation records;
7. creates GitHub build-provenance attestations tied to the tagged commit and every covered artifact;
8. deterministically builds release notes from the matching changelog section plus committed fixed text covering unsigned/unnotarized macOS binaries, checksum verification, and attestation verification;
9. creates a draft GitHub Release and uploads the complete artifact, checksum, attestation, and release-note set;
10. publishes the GitHub Release only after every pre-publication build, package, platform, integrity, and smoke gate succeeds;
11. starts stable-only Homebrew publication and installation tests against the published immutable assets.

The generated shell installer verifies downloaded release integrity using cargo-dist's checksum mechanism. Documentation presents Homebrew and Cargo as preferred managed channels while retaining the installer and manual archives for environments without them.

While a GitHub Release is still a draft, a rerun may replace draft assets, but it must regenerate and re-upload the complete checksum and attestation set. Once published, GitHub immutable releases are enabled and assets may not be deleted, replaced, or reused with different bytes.

## Homebrew Publication

Create a separate public repository, `LorNtz/homebrew-tap`. cargo-dist generates the `fluxcope` formula using immutable GitHub Release URLs and checksums.

The formula supports both selected macOS architectures and both selected Linux architectures. After the GitHub Release is published, automation runs `brew install` and `brew test` against the formula on x86_64 macOS 13, Apple Silicon macOS 13, x86_64 glibc 2.28 Linux, and aarch64 glibc 2.28 Linux. Prereleases never overwrite the stable formula. A post-publication Homebrew failure does not revert the GitHub Release to draft; it is repaired under the rollback policy.

The cross-repository credential is a fine-grained token limited to repository contents for `LorNtz/homebrew-tap`. It is stored in a `homebrew-release` environment that accepts protected release tags only, denies branch deployments, and is referenced only by the tap-update job.

## crates.io Publication

### Stable releases after bootstrap

The `release-plz release` job in **`release-plz.yml`**, on the exact eligible merged commit, owns source publication. It obtains a short-lived registry token through release-plz's native OIDC exchange with `id-token: write`; it stores no `CARGO_REGISTRY_TOKEN`. The Trusted Publisher environment is **`crates-release`**, whose permitted ref is now **`master`**.

Run locked Cargo package verification before release-plz, retain the expected `.crate` digest, and reject a changed lockfile or different uploaded digest. Do not assume a successful/no-op release-plz exit proves that a missing tag, artifact workflow, or registry checksum is correct.

For an existing registry version, compare its checksum with the package from the exact authorized commit before treating the upload as idempotently complete. If a previous attempt uploaded the crate but failed before creating a tag, release-plz can report no new packages; use the narrowly defined missing-tag recovery procedure, not a blind version bump or second upload.

The intended stable order is source publication and version tagging by release-plz, then verified GitHub binary publication by cargo-dist, then Homebrew. If tag delivery overlaps registry visibility, the artifact gate waits for/verifies the intended registry entry before publishing binaries. Crate-first partial publication is accepted; all-channel completion still requires every artifact and installation gate.

### v0.1.0 bootstrap

Trusted Publishing cannot create a new crate. Keep `CRATES_BOOTSTRAPPED=false` while preparing and merging the initial 0.1.0 release PR. The PR and CI run normally, but the automated publisher must explicitly skip with a bootstrap-required explanation and create no stable tag.

The maintainer then:

1. confirms the crate name remains available and checks out the exact merged 0.1.0 commit;
2. verifies locked tests/package checks and the manual application checklist;
3. publishes locally once with a short-lived new-crate token and revokes it after the attempt, including failure/interruption;
4. compares the registry checksum with the expected package and configures owner `LorNtz`, repository `fluxcope`, workflow `release-plz.yml`, environment `crates-release` as Trusted Publisher;
5. creates the first annotated `v0.1.0` tag at that exact commit and pushes it, without requiring a personal signature;
6. waits for cargo-dist, GitHub asset verification, and Homebrew checks to succeed;
7. sets `CRATES_BOOTSTRAPPED=true` and enables ordinary release-plz publishing for future release-PR merges.

Do not expect release-plz to manufacture the missing bootstrap tag after discovering that 0.1.0 is already in the registry. If publication succeeds but later setup fails, revoke the token, retain the exact source/package evidence, repair the setup, and create/push only the intended first tag. Never change an already published tag.

## Prerelease Policy

Keep GitHub-only `vX.Y.Z-rc.N` releases. Do not change this to crates.io RC publication merely to fit a default release-plz workflow.

RC preparation is an explicit maintainer exception:

- prepare version/changelog changes on `rc/X.Y.Z-rc.N`, not a `release-plz-` branch, and merge through normal CI;
- use the shared release-plz changelog generator and `set-version`; do not introduce a second generator;
- the stable publisher rejects every prerelease Cargo version before credentials or `release-plz release` are used;
- after verifying the exact merged commit, the maintainer creates and pushes the annotated RC tag;
- cargo-dist builds/verifies the same four archives, installer, checksums, and attestations, marks the release prerelease/not-latest, and skips Homebrew;
- no crates.io upload or registry-existence gate runs for an RC.

While `master` contains an RC version, automatic stable-PR refresh is paused. To promote, create a reviewed `release-plz-promote-X.Y.Z` PR using the documented version override, with a stable manifest and consolidated changelog. The maintainer-created promotion PR satisfies the same identity/CI gates as an App-created release PR; merging it invokes the ordinary stable publisher and creates a new stable tag. Never rename/reuse an RC tag.

## Public Documentation

### README

`README.md` must contain:

- a concise description of Fluxcope as a terminal HTTP/HTTPS MITM debugging proxy with optional MCP control;
- supported platforms and minimum versions;
- screenshots or a terminal recording that does not expose private traffic;
- installation through Homebrew, crates.io, shell installer, and manual archives;
- checksum and GitHub attestation verification;
- unsigned/unnotarized macOS guidance without global Gatekeeper disablement;
- CLI quick start and `fluxcope mcp` setup;
- CA generation, trust installation, download, and removal instructions;
- recording, mapping, request inspection/search, configuration, and relevant key bindings;
- a prominent network warning that v0.1.0 listens on all IPv4 interfaces without proxy authentication or an ACL, so it must be used only on trusted networks or restricted with the host firewall;
- links to security policy, contributing guide, changelog, licenses, and release artifacts.

### Security policy

`SECURITY.md` directs reports to GitHub private vulnerability reporting, lists supported versions, requests reproduction detail without real captured secrets, and documents the MITM-specific threat model:

- the CA private key grants interception authority wherever the CA is trusted;
- users must protect `~/.fluxcope` and remove trust when finished;
- the proxy may be LAN-reachable and unauthenticated;
- captured bodies and headers may contain credentials or personal data;
- MCP control should remain owner-local and its output must not be treated as trusted instructions.

### Contributing guide

`CONTRIBUTING.md` documents the pinned Rust toolchain, local CI commands, test expectations, Conventional Commit PR titles, squash merges, security-reporting boundary, and rule against committing captures, certificates, keys, logs, or personal data.

### Maintainer runbook

`docs/releasing.md` is the operational source of truth for:

- required GitHub repository/environment/tag settings;
- Homebrew tap setup and credential scope;
- first crates.io publication and Trusted Publisher setup;
- finding, reviewing, correcting, and merging the bot's release PR;
- automated stable tags and the limited bootstrap/RC/missing-tag exceptions;
- stable and prerelease publication;
- artifact/checksum/attestation/Homebrew/crate verification;
- failure classification, rerun rules, yanking, and patch-release policy;
- credential revocation and rotation.
- the pinned full-history scanner command, reviewed allowlist policy, report-retention location, and maintainer sign-off;
- deterministic stable release-note generation;
- the versioned application smoke checklist, exact fixtures/observations, evidence, and mandatory CA/state cleanup.

## Public-Repository Safety Gate

Before changing visibility, inspect the complete Git history and current tree for:

- API tokens, passwords, cookies, private keys, and signing material;
- generated CA keys/certificates and capture exports;
- logs and runtime descriptors;
- private hosts, endpoints, usernames, email addresses, and personal data;
- local-only `requirements.md` and `what_i_just_did.md` content;
- large generated artifacts.

A current-tree ignore rule is insufficient if sensitive or prohibited content exists in history. Pin a history-capable scanner such as gitleaks to an exact version, commit the exact full-history command and reviewed allowlist policy to the runbook, assign the maintainer as gate owner, and retain its report plus the manual MITM-artifact checklist as private release evidence. Revoke exposed credentials before rewriting history. Purge any generated secrets and the designated private planning/work files—including `requirements.md` and `what_i_just_did.md`—before publication whether or not they contain credentials. Enable GitHub secret scanning when the repository becomes public.

## Failure and Rollback Policy

- Never move, delete, or recreate a pushed release tag. Never overwrite a crates.io version or replace published release assets.
- A failed source preflight blocks upload. A failed binary gate keeps GitHub draft/unpublished but does not undo an already published crate.
- After upload, retain the package digest, source SHA, and release-PR evidence even if tag creation or binary publication fails.
- Rerun transient failures against the original event SHA/tag, not a newer `master` checkout. Keep upload jobs non-cancelling.
- Draft-only assets may be replaced with a fully regenerated matching checksum/attestation set. Published assets remain immutable.
- A Homebrew failure leaves GitHub and crates.io published; repair formula-only metadata against the existing assets or rerun the failed tap job.
- A missing stable tag after a successful upload is recoverable only after exact registry/package equality, reviewed source-commit identity, and tag absence are proved. The maintainer may create that one missing tag; an existing tag is never replaced.
- A source/workflow defect requiring different committed bytes needs a new version. Use a patch where semantically appropriate and describe the failed/partial earlier release.
- Yank only when continued installation is harmful and explicitly authorized; a yank does not erase the published version or free its number.
- Do not auto-merge a new release PR while the previous version is partially published; finish or explicitly classify recovery first.

## Acceptance Criteria

The implementation is ready for v0.1.0 only when all of the following are demonstrated:

### Source and CI

- A clean clone uses Rust 1.98.0 and passes format, strict Clippy, full locked tests, cargo-deny policy, package validation, release-plz configuration validation, and cargo-dist plan validation with exact reviewed tool pins.
- Required checks run on pull requests with read-only permissions and no release secrets.
- The packaged crate builds independently from the working tree after extraction and path installation.
- `cargo package --list` contains only the approved source files plus the explicitly validated Cargo-generated metadata files.
- Conventional PR title validation, squash-title configuration, commit-subject restriction, and squash-only repository policy produce changelog-compatible default-branch history.
- The GitHub App's release PR automatically receives ordinary read-only CI, and ordinary `master` merges do not publish a crate or create a stable tag.
- The publisher uses the exact release-PR squash commit even if a later ordinary PR merges; changed PR heads require fresh checks and smoke sign-off.

### Identity

- `cargo install --locked fluxcope` installs `fluxcope`.
- `fluxcope --version` reports exactly `fluxcope 0.1.0`.
- `fluxcope --help` exits successfully without entering the TUI.
- The checked-out tracked tree, generated package, workflows, and release assets contain no active command, package field, path, protocol URI, workflow, artifact, formula, or installation instruction exposing `wirelens`; preserved `.git` history is outside this identity assertion and passes the separate safety audit.
- Default settings and runtime state are created only below the new Fluxcope namespaces; old `.wirelens` state is neither consumed nor modified.

### Artifacts and platforms

- All four target archives are present and their packaged binaries pass exact help/version smoke tests.
- Both Linux architectures have no required symbol newer than `GLIBC_2.28` and execute in glibc 2.28 userspaces built from pinned images.
- Both macOS binaries declare a minimum OS of 13 and execute on native macOS 13 hardware or runners for their respective architectures.
- One SHA-256 manifest verifies the four target archives and installer; it explicitly excludes itself, GitHub-generated source links, and attestation records.
- GitHub attestations resolve to the exact tagged commit and covered asset digests.
- The shell installer verifies integrity and installs the expected version.
- Homebrew install and formula tests pass on all four supported OS/architecture combinations after GitHub publication.

### Application smoke tests

A versioned clean-environment checklist in `docs/releasing.md` assigns the maintainer as owner and specifies exact OS images/hardware, commands, fixture requests/responses, expected observations, retained evidence, and cleanup. It verifies:

- first startup creates state directories with mode `0700` and CA private keys plus other secret-bearing files with mode `0600` on Unix;
- proxy bind/startup and clean shutdown use the documented address and leave no listener;
- CA download presentation never exposes the private key;
- deterministic local HTTP and HTTPS fixtures produce the expected captures;
- recording toggle, map-remote, and map-local produce the stated fixture outcomes;
- settings persist under `~/.fluxcope` with the documented ownership and permissions;
- request navigation/search and body viewing match the fixture data;
- terminal state is restored after normal exit and startup failure;
- test CA trust, temporary state, fixtures, and listeners are removed before sign-off.

An automated child-process MCP smoke test verifies that `fluxcope mcp`:

- starts with stdout reserved for protocol traffic;
- completes MCP initialization;
- lists tools, prompts, and resources;
- exercises one bounded owner-local discovery/status path;
- exits cleanly.

### Publication paths

- A normal stable release requires only review/checks, merge of the release PR, and publication verification; no local preparation recipe or personal tag-signing step.
- An eligible stable release-PR merge publishes through release-plz's native OIDC, creates the exact immutable version tag, and triggers cargo-dist through the App's tag event.
- release-plz never creates a competing GitHub Release; cargo-dist never uploads the crate.
- A crate-success/binary-failure scenario retains the registry version and blocks GitHub binary completion; the runbook correctly identifies this partial publication.
- Recovery verifies exact existing package checksums and handles a missing tag without assuming a release-plz no-op repaired it.
- Bootstrap remains gated until the manual first upload, unconditional token revocation, Trusted Publisher setup, first tag, and binary/Homebrew checks complete.
- An RC PR/tag never invokes registry publication or changes Homebrew/latest; stable promotion creates a new stable release through the normal merged-PR path.
- The pinned full-history scan and manual prohibited-artifact checklist pass before visibility changes; designated private planning/work files are removed from public history with explicit authorization.

## Implementation Boundaries

Release implementation should be divided into reviewable groups:

1. clean Fluxcope identity cutover;
2. Cargo metadata, licenses, and package allowlist;
3. public README/security/contributing/releasing/changelog documents;
4. pinned toolchain, cargo-deny, release-plz/changelog configuration, and cargo-dist;
5. CI, App/token boundaries, stable release-PR publishing, first-crate bootstrap, RC exceptions, dependency updates, and repository-setting instructions;
6. end-to-end package, artifact, platform, TUI, and MCP verification.

The implementation must not change proxy behavior, add network controls, refactor unrelated application code, or expand platform scope. Any behavior defect discovered while exercising the release contract is fixed at its source in a separately identifiable change and reviewed before release.

## Manual Release SOP

### Scope and execution status

For an ordinary stable version, the human procedure is **review → check → merge → verify**. The bot prepares version/changelog changes and handles source publication/tagging; cargo-dist handles binaries and Homebrew. The longer setup, bootstrap, RC, and recovery subsections are exceptions, not work repeated for every stable version.

This SOP is a contract for the release implementation, not a claim that these workflows/tools are already installed. Adding or approving the document does not itself authorize uploads, tag pushes, GitHub App creation, permission changes, or repository visibility changes.

| Interface | Required operator behavior |
| --- | --- |
| `release-plz.toml` | Own version/changelog policy; require `release_always=false`; disable release-plz GitHub Releases. |
| `.github/workflows/release-plz.yml` | Refresh release PRs; publish only eligible stable release-PR merges; a manual dispatch refreshes preparation only. |
| `.github/workflows/ci.yml` | Check ordinary and bot PRs plus exact `master` commits. |
| `.github/workflows/release.yml` | Build/verify GitHub artifacts from authorized tags, then update/test stable Homebrew. |
| `SHA256SUMS` | Name the four archives and installer by relative filename. |
| `docs/releasing.md` | Contain this SOP, tool pins, exact application fixtures, runner details, and the private evidence location. |

Use Bash for command examples, execute one block at a time, and stop on errors or unexpected results. Do not bypass checks with `--admin`, `--allow-dirty`, `--no-verify`, force tags, or forced `master` updates.

### One-time setup

1. Complete the private full-history/current-tree safety audit. Obtain explicit approval before any rewrite or visibility change, then confirm the public repositories are `LorNtz/fluxcope` and `LorNtz/homebrew-tap`.
2. Create the repository's `release` label. Configure required checks, up-to-date release PRs, squash-only merging, immutable releases, and separate tag-creation versus tag-update/deletion rules. Do not grant the bot a `master` bypass or auto-merge permission.
3. Install GitHub CLI, rustup, Python 3.11+, and the exact pinned release-plz/cargo-dist/cargo-deny tools from `docs/releasing.md`. `gh auth login` and `gh auth status` must identify the maintainer account. Routine releases require no local signing-key setup.
4. Create a private GitHub App with webhooks disabled, Contents read/write and Pull requests read/write, installed only on `LorNtz/fluxcope`. Record `RELEASE_APP_ID`; store its private key as `RELEASE_APP_PRIVATE_KEY` in both `release-pr` and `crates-release`. The workflow mints short-lived App tokens; never store an installation token permanently. Keep both environments limited to `master`.
5. Put `HOMEBREW_TAP_TOKEN` in tag-only `homebrew-release`, scoped only to tap contents. Keep tap credentials out of the other environments. Do not store `CARGO_REGISTRY_TOKEN` in GitHub.
6. Initialize repository variable `CRATES_BOOTSTRAPPED=false`. After the manual first upload, configure crates.io Trusted Publishing for owner **LorNtz**, repository **fluxcope**, workflow **release-plz.yml**, environment **crates-release**. In release-plz's workflow only the publishing job receives `id-token: write`; cargo-dist's attestation jobs receive their own separately scoped OIDC permission.
7. Provision native Intel/Apple Silicon macOS 13 and both glibc 2.28 Linux test environments. Confirm the bot PR gets CI without manual close/reopen and the App's tag event starts cargo-dist.
8. Complete the first-release bootstrap below before enabling routine automatic publication. Keep the App key/tap token rotation procedure and recovery evidence private.

### Ordinary stable release: four steps

#### Step 1 — Open the current release PR and review it

In GitHub, open the PR labelled `release`, targeting `master`, from the configured App's `release-plz-` branch. Use the current PR, not a stale link: regeneration can replace it.

Optional CLI navigation:

```bash
set -euo pipefail
export REPO="LorNtz/fluxcope"
gh auth status
gh pr list --repo "$REPO" --state open --base master --label release
printf 'Release PR number: '
read -r PR_NUMBER
export PR_NUMBER
gh pr view "$PR_NUMBER" --repo "$REPO" \
  --json url,title,author,baseRefName,headRefName,headRefOid
gh pr view "$PR_NUMBER" --repo "$REPO" --web
```

Review the proposed Cargo version, lockfile, changelog, and included changes. Check CLI flags, YAML semantics, MCP contracts, and network defaults manually for breaking changes. The PR should not contain unrelated dependency refreshes.

If no release PR exists, inspect the latest preparation run. A no-op is normal when no packaged files changed. A preparation-only refresh is:

```bash
gh workflow run release-plz.yml --repo "$REPO" --ref master
```

That dispatch does **not** publish. If the proposed version is wrong, use the version-correction subsection before proceeding.

#### Step 2 — Wait for checks and complete smoke verification

```bash
export PR_HEAD="$(gh pr view "$PR_NUMBER" --repo "$REPO" \
  --json headRefOid --jq .headRefOid)"
gh pr checks "$PR_NUMBER" --repo "$REPO" --watch
test "$(gh pr view "$PR_NUMBER" --repo "$REPO" --json headRefOid --jq .headRefOid)" = "$PR_HEAD"
```

Expected: all required checks pass, the release PR is current with `master`, and `PR_HEAD` is the head you reviewed. A bot update, rebase, manual correction, or newly incorporated commit invalidates the earlier review/checklist; repeat this step for the new head.

Run the versioned application checklist from `docs/releasing.md` with synthetic local traffic and isolated state. Record the tested head and OS/architecture. Required observations: HTTP/HTTPS capture, recording toggle, both mapping types, settings persistence, search/body viewing, CA download without the key, MCP initialization/discovery, and terminal restoration. Remove test CA trust, listeners, and temporary state before sign-off.

For local smoke testing, use a dedicated clean checkout, not an active working directory:

```bash
test -z "$(git status --porcelain)"
gh pr checkout "$PR_NUMBER" --repo "$REPO"
test "$(git rev-parse HEAD)" = "$PR_HEAD"
cargo build --release --locked
./target/release/fluxcope --version
./target/release/fluxcope --help
```

The displayed version must equal the release PR. Review the exact fixture checklist rather than using `--help` as a substitute for application testing.

#### Step 3 — Merge when ready to publish

**This merge authorizes registry publication and automatic tagging.** There is no later manual tag push to delay the release. Confirm the repository, reviewed version, and PR head now; do not merge merely to save the preparation changes.

Squash-merge in GitHub after the final review, or use the head-locked command:

```bash
gh pr merge "$PR_NUMBER" --repo "$REPO" --squash \
  --match-head-commit "$PR_HEAD" \
  --subject "chore(release): prepare release"
```

Expected: the reviewed PR merges under normal rules. If the head changed or checks are stale, stop and repeat review; do not add `--admin` or remove the head check. While release-plz is publishing, avoid merging another release PR.

The normal pipeline now does the work: validate the exact squash commit and its CI, publish the crate, create the version tag, build/verify binary artifacts, publish GitHub, then update/test Homebrew. Some of these are separate runs, not one atomic transaction.

#### Step 4 — Monitor and verify completion

Record the actual merged commit, not a newer `master` tip:

```bash
test "$(gh pr view "$PR_NUMBER" --repo "$REPO" --json state --jq .state)" = "MERGED"
export RELEASE_COMMIT="$(gh pr view "$PR_NUMBER" --repo "$REPO" \
  --json mergeCommit --jq .mergeCommit.oid)"
export PLZ_RUN_ID="$(gh run list --repo "$REPO" --workflow release-plz.yml \
  --event push --commit "$RELEASE_COMMIT" --limit 1 \
  --json databaseId --jq '.[0].databaseId // empty')"
test -n "$PLZ_RUN_ID"
gh run watch "$PLZ_RUN_ID" --repo "$REPO" --exit-status
gh run view "$PLZ_RUN_ID" --repo "$REPO" --web
```

If no run is listed yet, wait and repeat the lookup. A skipped publisher is expected on an ordinary merge, but is not success for this authorized stable release. Inspect the recorded version/tag/digest; a release-plz no-op alone is not proof of completed publication.

Take `VERSION` from the reviewed PR and successful publisher output, without a leading `v`:

```bash
printf 'Published version, for example 0.2.0: '
read -r VERSION
export VERSION TAG="v$VERSION"
export RELEASE_RUN_ID="$(gh run list --repo "$REPO" --workflow release.yml \
  --event push --commit "$RELEASE_COMMIT" --branch "$TAG" --limit 1 \
  --json databaseId --jq '.[0].databaseId // empty')"
test -n "$RELEASE_RUN_ID"
gh run watch "$RELEASE_RUN_ID" --repo "$REPO" --exit-status
gh release view "$TAG" --repo "$REPO"
```

Expected: the tag and registry package correspond to `RELEASE_COMMIT`; the GitHub release is stable, published, and complete; all four platform gates and Homebrew tests pass. Inspect checksum/attestation and exact-version installation evidence as described below.

Record the PR, source SHA, version/tag, both workflow URLs, package digest, artifact/installation results, and manual smoke sign-off privately. Only announce all-channel availability after all gates pass. Crate success with a failing binary or tap job is **partial publication**, not a completed release.

### Correcting the proposed version

This is exceptional; ordinary releases need no local version command. Stop bot refreshes/other merges while applying a correction, then re-review any regenerated PR rather than assuming edits survive.

On the current release PR in a clean checkout:

```bash
gh pr checkout "$PR_NUMBER" --repo "$REPO"
release-plz set-version 0.2.0
cargo update --workspace
git diff -- Cargo.toml Cargo.lock CHANGELOG.md
git add -- Cargo.toml Cargo.lock CHANGELOG.md
git commit -m "chore(release): correct proposed version"
git push
```

Replace `0.2.0` with the intended exact stable version. Check the complete diff for unexpected dependency changes; don't commit those as incidental release preparation. Re-run CI and the affected smoke checks. Record the new PR head before the head-locked merge.

### First-release bootstrap: v0.1.0 only

1. Keep `CRATES_BOOTSTRAPPED=false`. Review/check/merge the initial 0.1.0 PR through ordinary Steps 1–3. If no initial metadata diff exists for the bot to propose, create a reviewed, `release`-labelled `release-plz-bootstrap-0.1.0` PR with the initial changelog and metadata rather than assuming a bot PR must appear.
2. Confirm the publisher explicitly skipped because bootstrap is disabled and created no stable tag. Recheck exact `fluxcope` name availability on crates.io; network errors do not establish availability.
3. Set `RELEASE_COMMIT` to that PR's exact squash commit, set `VERSION=0.1.0`, and in a clean checkout verify locked package/CI/application checks:

```bash
export VERSION="0.1.0" TAG="v0.1.0"
test "$(gh pr view "$PR_NUMBER" --repo "$REPO" --json state --jq .state)" = "MERGED"
export RELEASE_COMMIT="$(gh pr view "$PR_NUMBER" --repo "$REPO" \
  --json mergeCommit --jq .mergeCommit.oid)"
export CI_RUN_ID="$(gh run list --repo "$REPO" --workflow ci.yml \
  --event push --commit "$RELEASE_COMMIT" --limit 1 \
  --json databaseId --jq '.[0].databaseId // empty')"
test -n "$CI_RUN_ID"
gh run watch "$CI_RUN_ID" --repo "$REPO" --exit-status
git fetch origin --tags
test -z "$(git status --porcelain)"
git switch --detach "$RELEASE_COMMIT"
git merge-base --is-ancestor "$RELEASE_COMMIT" origin/master
cargo publish --locked --dry-run
shasum -a 256 "target/package/fluxcope-$VERSION.crate"
```

4. Save the digest privately. On crates.io, create the narrowest, shortest-lived token that can publish this new crate. Confirm the intended name/version/source and owner. This upload is permanent. Do not use `cargo login` or store the token in GitHub, files, or shell history:

```bash
set +x
read -r -s -p 'Short-lived crates.io token: ' BOOTSTRAP_TOKEN
printf '\n'
if CARGO_REGISTRY_TOKEN="$BOOTSTRAP_TOKEN" cargo publish --locked; then
  PUBLISH_STATUS=0
else
  PUBLISH_STATUS=$?
fi
unset BOOTSTRAP_TOKEN
printf 'Publish exit status: %s. Revoke the token on crates.io now.\n' "$PUBLISH_STATUS"
```

5. Revoke the token on crates.io after success, failure, or interruption. Clearing the shell variable is not revocation. If the upload response was lost, inspect the registry before retrying.
6. Configure the crate's Trusted Publisher for `LorNtz/fluxcope`, **`release-plz.yml`**, **`crates-release`**. No separate auth action or permanent registry token is configured.
7. Compare the registry checksum to the local package using the verification subsection. A mismatch stops the procedure. If it matches and the first tag is absent, create the one bootstrap tag:

```bash
git tag -a "$TAG" "$RELEASE_COMMIT" -m "Release Fluxcope $TAG"
test "$(git rev-parse "$TAG^{commit}")" = "$RELEASE_COMMIT"
printf 'Bootstrap tag: %s\nSource: %s\n' "$TAG" "$RELEASE_COMMIT"
git push origin "refs/tags/$TAG"
```

8. Wait for `release.yml` at that exact tag/commit, verify GitHub artifacts and Homebrew, then enable future automatic publication:

```bash
gh variable set CRATES_BOOTSTRAPPED --repo "$REPO" --body true
```

This is a one-time account/repository setup change, not a per-release step. Do not re-upload 0.1.0 or expect a later release-plz no-op to create its missing tag.

### GitHub-only release candidates and promotion

The stable path is automatic; RCs remain manual to preserve the no-crates.io/no-Homebrew policy.

1. Close or leave unmerged the current stable release PR. From clean, current `master`, prepare an RC on a non-release-plz branch:

```bash
export VERSION="0.2.0-rc.1"
export TAG="v$VERSION"
git switch master
git pull --ff-only origin master
git switch -c "rc/$VERSION"
release-plz update
release-plz set-version "$VERSION"
cargo update --workspace
git diff -- Cargo.toml Cargo.lock CHANGELOG.md
git add -- Cargo.toml Cargo.lock CHANGELOG.md
git commit -m "chore(release): prepare $TAG"
git push --set-upstream origin "rc/$VERSION"
gh pr create --repo "$REPO" --base master --head "rc/$VERSION" \
  --title "chore(release): prepare $TAG" --body "GitHub-only release candidate; no registry or Homebrew publication."
export PR_NUMBER="$(gh pr view "rc/$VERSION" --repo "$REPO" --json number --jq .number)"
```

2. Review, run CI and the versioned smoke checklist, and squash-merge the RC PR. Record its exact merge SHA as `RELEASE_COMMIT`. Confirm the stable publisher skipped before requesting credentials because the manifest is a prerelease.
3. Fetch and verify that exact merge commit, then create only the intended RC tag:

```bash
git fetch origin --tags
test "$(gh pr view "$PR_NUMBER" --repo "$REPO" --json state --jq .state)" = "MERGED"
export RELEASE_COMMIT="$(gh pr view "$PR_NUMBER" --repo "$REPO" \
  --json mergeCommit --jq .mergeCommit.oid)"
git merge-base --is-ancestor "$RELEASE_COMMIT" origin/master
git show "$RELEASE_COMMIT:Cargo.toml"
git tag -a "$TAG" "$RELEASE_COMMIT" -m "Release Fluxcope $TAG"
git push origin "refs/tags/$TAG"
```

4. Monitor cargo-dist and verify all GitHub artifact/integrity/platform gates. The release must be prerelease/not-latest; registry publication and Homebrew updates must not run. Skip stable-channel install checks.
5. While `master` has an RC version, the bot does not refresh stable preparation automatically. When ready for stable, create a promotion PR:

```bash
export VERSION="0.2.0"
git switch master
git pull --ff-only origin master
git switch -c "release-plz-promote-$VERSION"
release-plz update
release-plz set-version "$VERSION"
cargo update --workspace
git diff -- Cargo.toml Cargo.lock CHANGELOG.md
git add -- Cargo.toml Cargo.lock CHANGELOG.md
git commit -m "chore(release): promote $VERSION"
git push --set-upstream origin "release-plz-promote-$VERSION"
gh pr create --repo "$REPO" --base master --head "release-plz-promote-$VERSION" \
  --label release --title "chore(release): promote $VERSION" \
  --body "Promote the reviewed RC changes to a stable release. Merging authorizes crates.io publication and automatic tagging."
```

Review the consolidated changelog against the preceding stable release; remove redundant RC sections only as part of this reviewed preparation. Continue ordinary Steps 1–4 using the promotion PR. Do not manually create the stable tag or reuse an RC tag.

### Verification commands and evidence

For a normal stable release, the jobs produce this evidence automatically and the maintainer reviews it. The commands below allow independent verification and are required when recovering partial publication; they are not extra preparation/tagging steps.

Download the exact release, not `latest`:

```bash
export VERIFY_DIR="$(mktemp -d)"
gh release download "$TAG" --repo "$REPO" --dir "$VERIFY_DIR"
(
  cd "$VERIFY_DIR"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum --check SHA256SUMS
  else
    shasum -a 256 --check SHA256SUMS
  fi
)
```

All four archives and the installer must report `OK`. Select each of those five subject filenames in turn and verify provenance:

```bash
printf 'Artifact filename from SHA256SUMS: '
read -r ARTIFACT
gh attestation verify "$VERIFY_DIR/$ARTIFACT" --repo "$REPO" \
  --signer-workflow "$REPO/.github/workflows/release.yml" \
  --source-ref "refs/tags/$TAG" --source-digest "$RELEASE_COMMIT"
```

Keep the expected signer path synchronized if implementation uses a reusable attestation workflow. Do not weaken verification flags on failure.

To verify an existing registry version for bootstrap or recovery, first create the expected package from the clean exact authorized commit using the pinned toolchain:

```bash
test -z "$(git status --porcelain)"
test "$(git rev-parse HEAD)" = "$RELEASE_COMMIT"
cargo package --locked
python3 - <<'PY'
import hashlib
import json
import os
import pathlib
import tomllib
import urllib.request

version = os.environ["VERSION"]
package = tomllib.loads(pathlib.Path("Cargo.toml").read_text())["package"]
assert package["name"] == "fluxcope" and package["version"] == version, package
archive = pathlib.Path(f"target/package/fluxcope-{version}.crate")
with archive.open("rb") as source:
    expected = hashlib.file_digest(source, "sha256").hexdigest()
request = urllib.request.Request(
    "https://index.crates.io/fl/ux/fluxcope",
    headers={"User-Agent": "fluxcope-release-verification"},
)
with urllib.request.urlopen(request, timeout=30) as response:
    entries = [json.loads(line) for line in response if line.strip()]
matches = [entry for entry in entries if entry["name"] == "fluxcope" and entry["vers"] == version]
assert len(matches) == 1, f"Expected one published fluxcope {version} entry"
assert matches[0]["cksum"] == expected, "Registry/package checksum mismatch"
assert not matches[0]["yanked"], "Version is yanked: investigate"
print(f"Verified published fluxcope {version}: {expected}")
PY
```

Registry propagation can lag. Retry a read-only lookup after a short wait; missing metadata, a network error, or different bytes must never trigger an unconditional success or blind re-upload.

For an independent stable Cargo installation:

```bash
export CARGO_VERIFY_ROOT="$(mktemp -d)"
cargo install fluxcope --version "=$VERSION" --locked --root "$CARGO_VERIFY_ROOT"
test "$("$CARGO_VERIFY_ROOT/bin/fluxcope" --version)" = "fluxcope $VERSION"
"$CARGO_VERIFY_ROOT/bin/fluxcope" --help
```

On a disposable Homebrew verification machine without Fluxcope installed:

```bash
brew update
brew install LorNtz/tap/fluxcope
brew test LorNtz/tap/fluxcope
test "$("$(brew --prefix)/bin/fluxcope" --version)" = "fluxcope $VERSION"
```

A successful local check is not a substitute for all four supported platform jobs. The tap tracks its current stable version; use the release-specific tap commit and recorded CI evidence when a newer release has superseded it. Check the workflow's isolated shell-installer test rather than running a remote installer into your personal environment.

Retain evidence privately and remove only the recorded temporary verification directories, test processes, and test CA trust. Never remove personal Fluxcope state or an unrelated CA as cleanup.

### Recovery

| Situation | Action |
| --- | --- |
| No release PR appears | Inspect preparation logs. No packaged-file change can legitimately mean no PR. A dispatch refreshes preparation only. |
| Bot changes/replaces the PR during review | Select the current PR/head and repeat review, CI, and affected smoke checks. Do not merge a stale reviewed SHA. |
| OIDC or source preflight fails before upload | Fix environment/configuration or source as appropriate. Rerun only against the original authorized commit; no tag is proof of success. |
| Crate publishes but binaries fail | Keep the immutable registry version. Classify transient infrastructure versus committed source defects. Retry infrastructure on the same tag; a committed defect needs a new version. |
| Crate publishes but its tag is missing | Verify the authorized merge SHA, registry checksum, and tag absence. Use the narrowly scoped missing-tag procedure below; a release-plz no-op may not repair this. |
| A tag exists but cargo-dist did not start | Verify tag identity and App event credentials. Repair the trigger, then use the committed workflow's validated existing-tag recovery entry; do not delete/re-push the tag. |
| GitHub publishes but Homebrew fails | Repair/rerun the tap job only. Do not replace GitHub assets or re-upload the crate. |
| Existing crate checksum differs | Stop as an integrity incident. Do not skip, overwrite, or attach binaries from different source under that version. |
| A published version is defective | Publish a new version through a reviewed release PR. Yank only if harmful and explicitly authorized. |

Inspect the exact failed run; use `PLZ_RUN_ID` for source/tagging failures and `RELEASE_RUN_ID` for artifact/tap failures:

```bash
printf 'Failed run ID from GitHub Actions: '
read -r FAILED_RUN_ID
gh run view "$FAILED_RUN_ID" --repo "$REPO" --log-failed
git ls-remote --tags origin "refs/tags/$TAG" "refs/tags/$TAG^{}"
```

After confirming a transient failure and unchanged source:

```bash
gh run rerun "$FAILED_RUN_ID" --repo "$REPO" --failed
gh run watch "$FAILED_RUN_ID" --repo "$REPO" --exit-status
```

Never rerun all successful build/upload jobs just to repair a post-publication channel. The artifact workflow's recovery dispatch accepts an **existing tag only**, resolves and validates its original source/PR/registry evidence, and skips already published immutable assets. It cannot create a new tag or publish a crate:

```bash
gh workflow run release.yml --repo "$REPO" --ref "$TAG" -f tag="$TAG"
```

This dispatch/input is an implementation requirement for recovery, not a claim the current generated workflow already supplies it. Its ref and input tag must match and it must pass the same authorization gates as a tag push.

For a genuinely missing stable tag after an upload, the maintainer first completes the registry/package verification above from the original merged commit and confirms the source publisher's retained digest agrees. Then confirm that the tag does not exist locally or remotely. Only this exact absent tag may be created:

```bash
REMOTE_TAG="$(git ls-remote --tags origin "refs/tags/$TAG")"
test -z "$REMOTE_TAG"
git tag -a "$TAG" "$RELEASE_COMMIT" -m "Recover release tag for Fluxcope $TAG"
git push origin "refs/tags/$TAG"
```

The evidence record must identify this as recovery, not a new release authorization. If any existing tag disagrees, stop; never use `-f`. For source/workflow changes, prepare a new version rather than claiming the old tag includes a fix.

### Operator references

- [Release-plz configuration](https://release-plz.dev/docs/config): `release_always`, GitHub Release ownership, tag names, changelog settings, and publication safeguards.
- [Release PR lifecycle](https://release-plz.dev/docs/usage/release-pr) and [version overrides](https://release-plz.dev/docs/usage/set-version).
- [Release behavior and commit selection](https://release-plz.dev/docs/usage/release), including squash-merge implications.
- [Native Trusted Publishing](https://release-plz.dev/docs/github/quickstart) and [GitHub App/event credentials](https://release-plz.dev/docs/github/token).
- [Separate binary distribution](https://release-plz.dev/docs/extra/releasing-binaries): release-plz does not build binaries; cargo-dist remains their owner.
- [GitHub CLI attestation verification](https://cli.github.com/manual/gh_attestation_verify) and [Cargo registry checksum format](https://doc.rust-lang.org/cargo/reference/registry-index.html).
