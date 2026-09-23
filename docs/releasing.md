# Releasing Fluxcope

The first release uses the existing `master` TUI and proxy; it excludes the unmerged MCP feature. Archives support macOS 15 on Apple Silicon and Intel, and Linux with glibc 2.28 on ARM64 and x86-64. Standard GitHub-hosted runners replace the originally proposed paid macOS 13 environments.

## Daily commands

```sh
just                    # Discover commands
just doctor             # Check local tools, active authentication and workflows
just pr                 # Push committed feature changes; create/reuse a PR
just ship               # Review/merge the feature, then review/merge its release
just release            # Review/merge an already prepared remote release PR
just release-status     # Resume watching remote progress
```

From a feature branch, commit the changes you want included and run `just ship`. You do not need to push or create the PR first. The command displays the full diff and exact head, waits for required checks, and requests separate confirmations for the feature merge and the release merge. It uses squash merges with an expected-head check. It does not stage files, create feature commits, force-push or use an administrator bypass. Uncommitted files stay local.

If the work is already on `master`, use `just release`. This command reads the remote PR; your local branch and working tree do not determine the published version. Both commands wait for every required check, including jobs such as `Release preview` that appear only after archive builds finish. They print missing/running check names and allow GitHub time to refresh mergeability after checks pass. Failed checks, conflicts, required reviews and an outdated base produce specific next steps. Waiting is bounded to 90 minutes; stopping it does not cancel remote checks.

Ctrl-C stops local waiting; remote work continues. Before the release PR is merged, resume with `just release`, or rerun `just ship` on the same feature branch. After the release PR is merged, use `just release-status` to follow publication. The helper prints the appropriate resume command when a check wait is interrupted.

For a specific version, use `just release-prepare 0.2.0` or `just release-prepare 0.2.0-rc.1`, then `just release`. Preparation uses native release-plz version/changelog logic in a read-only job. A separate App-authenticated job writes only Cargo metadata, changelog and release intent. Explicit versions survive automatic refreshes; retries reuse the same PR and preserve an unchanged head. RC releases publish only to GitHub, without crates.io or Homebrew. Stable promotion builds a new stable version from its reviewed source.

Normal releases need only version/diff review and merge confirmation. When TUI behavior changes, inspect the relevant behavior using the PR's native archive before merging. For the first release, inspect startup, request navigation, detail/body views, settings, logs and certificate download. Automated HTTP/HTTPS, mapping, recording, permissions and shutdown checks remain mandatory.

## Temporary branch previews

For an experiment that may never merge, commit its changes and run `just preview`.
The helper pushes committed changes, creates/reuses a draft development PR, and
asks once to publish its exact head. It neither stages local edits nor merges the
PR. The branch must belong to this repository, target master, and already contain
the Fluxcope manifest and these helper recipes. Incorporate current master into
an older feature branch first; merging that branch back is never required.

```sh
just preview                 # Apple Silicon macOS 15+, by default
just preview all             # macOS ARM64/Intel and Linux ARM64/x86-64
just preview linux-x64       # Or macos-arm64, macos-intel, linux-arm64
just preview-status          # Resume/select previews of the current PR
just preview-retry RUN_ID    # Retry original failed jobs, retaining source/version
just preview-run RUN_ID      # Verify provenance, download and launch isolated
just preview all --new       # Explicit new identity for the same source/profile
```

Versions are automatic: `0.0.0-preview.RUN_ID`. A protected tag identifies a
deterministic child snapshot containing only the version and packaging policy
changes; no version commit is pushed back to your feature branch. The application
comes from the approved feature commit, while the release workflow comes from
protected master. Downloads are GitHub prereleases, never Latest, crates.io or
Homebrew. Publishing requires maintain/admin permission. You may also dispatch
**Branch preview** on master in Actions with the PR number, full head SHA and profile.

Each new preview Release contains a version-specific shell installer. Share the
Release URL and copy its install command; the recipient needs `curl` and `tar`,
but no checkout, `just`, Python installation, or GitHub login. For example, replace
`RUN_ID` with the exact published ID:

```sh
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/LorNtz/fluxcope/releases/download/v0.0.0-preview.RUN_ID/fluxcope-preview-installer.sh | sh
~/.local/bin/fluxcope-preview-RUN_ID
~/.local/bin/fluxcope-preview-RUN_ID --port 9010
```

Installation prints the absolute launch command and does not start the app or edit
PATH. If `~/.local/bin` is already on PATH, `fluxcope-preview-RUN_ID` also works.
The script selects the native platform included in that release: macOS 15+ or
Linux glibc 2.28+, ARM64 or x86-64. Unsupported platforms fail before installation.
The exact immutable HTTPS script is the initial trust boundary. It checks an
embedded SHA-256 digest before executing its private Python runtime and verifier;
the client then verifies the signed manifest, source/controller identity, archive,
and executable. The bundled verifier uses captured Sigstore trust roots offline.
It does not install Python or `gh` globally.

The launcher and `just preview-run RUN_ID` share
`~/.cache/fluxcope/previews/RUN_ID/TARGET/`, with a private HOME and separate
certificates/settings. The first launch selects and prints an available proxy port;
subsequent launches reuse settings. `--port` changes only that preview's saved port,
preserving other settings. Its port edit preserves YAML comments, but the app's
normal settings save can reformat the file and remove comments. Installation and launch are locked
against concurrent changes. Reinstalling the same ID preserves settings and repairs
its executable; it never silently selects a newer preview. Stable binaries and
state remain untouched. This state separation is not an OS sandbox, and previews
still listen on all IPv4 interfaces. System proxy/trust settings are not changed.

```sh
# Remove the launcher/runtime, retaining settings and certificates:
~/.local/bin/fluxcope-preview-RUN_ID --uninstall

# Alternatively, remove everything (asks you to type the ID):
~/.local/bin/fluxcope-preview-RUN_ID --uninstall --purge
```

After uninstall the command is gone; its output identifies the retained state
directory. Reinstall that exact ID while its downloads remain available to reuse
settings. Symlinked state paths are refused during installation and removal.

Downloads expire 30 days after publication; daily cleanup can take another 24 hours.
The protected tag and source stay in Git history. Installed copies continue to run
offline after expiry, verifying saved signed evidence and local files each time.
A new installation or repair needs an unexpired release. Older immutable previews
keep their archive-only assets and remain usable with `just preview-run`.
Closing a PR cancels an unpublished preview when observed by the final publication
checks; published previews keep their expiry.

Installer format 2 adds a controller-generated shell script and native client
bundles to the signed manifest. CI builds clients from the controller commit,
assembles and attests the staged files, then qualifies the exact shell/client on
fresh native runners with no publication credentials. An HTTPS replay supplies
staged metadata/files while normal signature and checksum verification remains
active. Each qualification report names the signed manifest digest. The separate
publisher checks those reports before obtaining/using release permissions;
installation tests do not execute in the signing or publishing jobs. Reports are
CI evidence outside the signed payload to avoid a circular manifest hash. A final
public download check is required for completion. New client dependencies are
version/hash pinned; the initial rollout qualifies all four native targets.

Closing a terminal leaves CI running. Use `preview-status` to resume; inspect a
failed run before `preview-retry`. Reruns retain the original source and workflow.
Fixing source or controller code requires a new preview, and published bytes are
never replaced. Expired evidence or downloads require a new ID. No scheduled builds
or automatic promotion to stable occur.

The local helper respects `HTTP_PROXY`/`HTTPS_PROXY` (including lowercase forms)
and uses an HTTP(S) `ALL_PROXY` as a fallback when a protocol-specific setting is absent.
This applies only to the command and its children; it does not change shell or system settings.

One-time preview setup: create the `preview-release` environment with a custom
**branch** policy allowing only `master`; import the existing source App ID/key
as `RELEASE_APP_ID` / `RELEASE_APP_PRIVATE_KEY`. Retain existing immutable release
and `v*` tag rules. Build/verification jobs enforce `cache-mode: read` and probe
the cache service for an explicit write denial before executing candidate code;
if the hosted service does not support this policy, publication remains blocked.

## One-time account and repository setup


1. Install `gh`, `just` and Python 3.11+. Authenticate with `gh auth login --hostname github.com --git-protocol ssh --web`; verify the active account with `gh api user` and repository access with `gh repo view LorNtz/fluxcope --json nameWithOwner,viewerPermission`.
2. Review all history and branches before making `LorNtz/fluxcope` public. The personal tap is `LorNtz/homebrew-tap`, not `homebrew/core`. Verify both origin URLs point to `git@github.com:LorNtz/fluxcope.git`.
3. Enable immutable releases, private vulnerability reporting and applicable secret scanning. Set `IMMUTABLE_RELEASES_ENABLED=true` only after verifying the actual repository setting. Initially set `CRATES_BOOTSTRAPPED=false`.
4. Require PRs and up-to-date branches on `master`, squash-only merging, and checks **Source quality**, **Tests (Linux)**, **Tests (macOS)**, **Package**, and **Release preview**. The preview aggregate runs on every PR; its four native builds run on release branches. Neither the source App nor scripts bypass `master` rules.
5. Register a source App with Contents/PR read-write, installed only on `fluxcope`, and a separate tap App with Contents read-write, installed only on `homebrew-tap`. Set the actual source `[bot]` login in `.github/release-config.json`. `RELEASE_APP_ID` / `RELEASE_APP_PRIVATE_KEY` and `TAP_APP_ID` / `TAP_APP_PRIVATE_KEY` must match those installations.
6. Configure environments using this table. Restrict deployment branches/tags and omit routine manual approval gates:

| Environment | Allowed ref | Credentials / authority |
| --- | --- | --- |
| `release-prepare` | `master` | Source App ID/key; PR metadata writer |
| `crates-release` | `master` | Source App ID/key; native crates.io OIDC publisher |
| `release-source` | `master` | Source App ID/key; authorized tag recovery and RC tags |
| `github-release` | `v*` tags | Workflow token; artifact signing and GitHub publication |
| `homebrew-release` | `v*` tags | Separate tap App ID/key |
| `release-result` | `master` | Workflow token; result signing/checks/notes and explicit replacement classification |

7. Separate tag rules: permit the source App to create `v*` tags, while another rule prohibits updating or deleting them. Protect the tap from force pushes and deletion; serialized publication verifies the expected old formula. Rotate App keys by importing a new key into its listed environments before revoking the old one.

Public repositories use free standard runners, including `macos-15`, `macos-15-intel`, `ubuntu-24.04` and `ubuntu-24.04-arm`. Private repositories consume account minutes and may incur charges. Do not substitute paid `-large`/`-xlarge` runners. [GitHub runner reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners).

## First crates.io upload

Trusted Publishing cannot create a new crate. After the reviewed `0.1.0` release PR is merged, exact-source CI produces the clean `.crate` and its SHA-256. The source workflow reports `bootstrap-required`; it does not upload or create a tag.

The maintainer receives the real PR/source/CI links before running:

```sh
just release-bootstrap 0.1.0 PR_NUMBER
```

The helper verifies the merged release identity and CI package, creates a disposable clean checkout of that source, runs locked package verification and installs the unpacked crate, then compares its digest with CI. Only after these checks does it request confirmation of the exact version and a hidden one-time crates.io token. Create a short-lived token in crates.io with permission to create/publish `fluxcope`. Do not use `cargo login` or put the token in a command, file or GitHub secret. Packages built with `--allow-dirty` are never publishable.

The helper reminds you to revoke the token after success, failure or interruption. If an upload response is lost, retrying first checks registry facts; an existing matching version is not uploaded again. A mismatching digest stops recovery.

After uploading, configure the crate's Trusted Publisher as owner `LorNtz`, repository `fluxcope`, workflow `release-plz.yml`, environment `crates-release`. Run `just release-recover 0.1.0`: it verifies the registry checksum, creates the missing exact-source tag and starts distribution. Follow `just release-status 0.1.0`. Set `CRATES_BOOTSTRAPPED=true` only after the signed unified result confirms all stable channels. No permanent registry token is used for later releases.

## Verification and retained evidence

Versioned GitHub assets include four archives with native `.sha256` sidecars, a shell installer, formula, manifest, smoke reports, source evidence and `sha256.sum`. The installer checks archive digests even on macOS using `shasum`; a corrupted-download test must fail before installation. Archives and the installer have provenance bound to the exact tag and source.

For independent verification, compare the file's SHA-256 with its entry in `sha256.sum`, then verify provenance:

```sh
gh attestation verify /absolute/path/to/fluxcope-ARCHIVE.tar.xz --repo LorNtz/fluxcope --signer-workflow LorNtz/fluxcope/.github/workflows/release.yml --source-ref refs/tags/vVERSION --source-digest SOURCE_SHA --deny-self-hosted-runners
```

`Release VERSION` on the authorized source commit is the unified result. It becomes successful only after public asset/provenance verification, four exact-tap-commit Homebrew installations and an exact-version crates.io installation; RC reports those stable-only channels as not applicable.

Temporary native build/preview artifacts last 7 days, source `.crate` artifacts last 30 days, and installation/result reports last 90 days. Immutable `source-evidence.json` preserves source/package identity after the source artifact expires. Complete results are stored as canonical JSON signed by the trusted `master` workflow `release-status.yml`, with a display copy in the editable Release notes. The observer verifies that signature and current immutable asset digests before reusing old installation results. Notes alone cannot claim successful checks; expired raw logs are not advertised as downloadable. Do not delete historical CI run metadata needed to identify its source checks.

## Recovery

Run `just release-recover VERSION` to inspect the current facts and confirm the specific remaining action. It can retry the original source publisher, create a missing exact-source tag, finish an unpublished draft, verify existing public assets, rerun installations, or refresh/archive the result. It never moves an existing tag or replaces published immutable assets. Temporary builds can be regenerated on a failed-job retry; the official published files remain fixed.

A GitHub publication followed by failed installation or result signing remains partial. If an archive job is cancelled before its script starts, the check stays actionable and local waiting ends when that observer run ends. Retry through recovery. A damaged notes report can be re-signed only while independent live evidence still proves every required channel. With expired evidence, use verification/install recovery rather than editing success fields. An older Homebrew release uses its matching historical formula without downgrading a newer tap. If result boundary markers themselves were deleted or duplicated, the tool refuses to guess which human notes to remove: edit the GitHub notes, remove only the damaged `fluxcope-release-result` block while preserving the changelog, then retry recovery.

If fixing the application source, package, formula or distributed assets requires different bytes, publish a new version. Installation-only tooling fixes can use the reviewed master recovery workflow below. `just release-replace VERSION` records a public reason and explicitly permits a reviewed replacement; it never marks the old release successful or yanks a crate. A checksum disagreement is an integrity conflict and must be resolved before ordinary recovery continues.

### Recovering installation checks for published versions

When GitHub Release and crates.io are already published but Homebrew or registry installation checks failed, merge repaired verification tooling into master and run `just release-recover VERSION`. If the personal tap contains the exact formula from the immutable Release, recovery selects `verify-installations`. The read-only `Verify installations` workflow on master verifies the original tag, source, crate checksum, public assets and provenance, then runs all four Homebrew installations and the registry installation. It also handles expired public verification reports directly. An unpublished formula still uses the existing tap publication recovery path.

Homebrew CI pins the recorded tap commit and compares its formula byte-for-byte with the Release snapshot before running `brew trust --formula LorNtz/tap/fluxcope` and installing. It trusts only that formula and keeps Homebrew's trust checks enabled. Users keep the README command `brew install LorNtz/tap/fluxcope`, which grants item-specific trust for that fully qualified name; see [Tap Trust](https://docs.brew.sh/Tap-Trust).

The master tooling commit and original release source SHA remain separate. Installation reports must identify the original version and source. Recovery never moves a tag, replaces assets or repeats the crate upload; the observer signs and archives the unified result after every required check passes.
