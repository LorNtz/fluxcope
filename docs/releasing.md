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

If the work is already on `master`, use `just release`. This command reads the remote PR; your local branch and working tree do not determine the published version. Ctrl-C stops local waiting; remote work continues. Resume with `just release-status`, or rerun `just ship` on the same feature branch to discover which stage completed.

For a specific version, use `just release-prepare 0.2.0` or `just release-prepare 0.2.0-rc.1`, then `just release`. Preparation uses native release-plz version/changelog logic in a read-only job. A separate App-authenticated job writes only Cargo metadata, changelog and release intent. Explicit versions survive automatic refreshes; retries reuse the same PR and preserve an unchanged head. RC releases publish only to GitHub, without crates.io or Homebrew. Stable promotion builds a new stable version from its reviewed source.

Normal releases need only version/diff review and merge confirmation. When TUI behavior changes, inspect the relevant behavior using the PR's native archive before merging. For the first release, inspect startup, request navigation, detail/body views, settings, logs and certificate download. Automated HTTP/HTTPS, mapping, recording, permissions and shutdown checks remain mandatory.

## One-time account and repository setup

See [the first-release account guide](first-release-setup.md) for the exact GitHub App registration and private-key import steps. Do not send private keys or registry tokens through chat.

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

If fixing source or a tagged workflow requires different bytes, publish a new version. `just release-replace VERSION` records a public reason and explicitly permits a reviewed replacement; it never marks the old release successful or yanks a crate. A checksum disagreement is an integrity conflict and must be resolved before ordinary recovery continues.

### 已发布版本的安装检查恢复

如果 GitHub Release 和 crates.io 已发布，但 Homebrew 或 registry 安装检查失败，修复验证工具后合入 master，再执行 `just release-recover VERSION`。当个人 tap 已包含与不可变 Release 完全相同的 formula 时，恢复计划选择 `verify-installations`：在 master 运行只读 `Verify installations` workflow，重新核验原始 tag、源码、crate 摘要、公开资产和 provenance，再检查四个平台的 Homebrew 安装及 registry 安装。tap 尚未写入时仍选择原有 `stable-installations` 发布流程。

Homebrew CI 先获取并锁定记录的 tap commit，确认 formula 与 Release 快照逐字节一致，然后执行 `brew trust --formula LorNtz/tap/fluxcope` 并安装；不信任整个 tap，也不关闭 Homebrew 的信任检查。普通用户仍使用 README 的 `brew install LorNtz/tap/fluxcope`；Homebrew 会为这个完整名称授予单项信任，参见 [Tap Trust](https://docs.brew.sh/Tap-Trust)。

验证工具的 master 提交与已发布应用的 source SHA 分别记录；安装报告必须仍绑定原始版本和 source SHA。恢复不移动 tag、不覆盖资产、不重复上传 crate。所有检查通过后，状态观察器签署并归档统一结果。
