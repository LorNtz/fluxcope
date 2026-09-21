# Fluxcope

A terminal HTTP and HTTPS debugging proxy with request recording, searchable request trees, body inspection, and local/remote response mapping.

The published v0.1.0 release contains the mainline TUI and proxy. Source builds on this branch also include the optional multi-instance MCP control plane described below.

## Installation

Once a version is available in [Releases](https://github.com/LorNtz/fluxcope/releases), install it through Homebrew or crates.io:

```sh
brew install LorNtz/tap/fluxcope
# Or build the published crate (Rust 1.98 or newer):
cargo install fluxcope --locked
```

Release archives target macOS 15+ (Apple Silicon and Intel) and Linux with glibc 2.28+ (ARM64 and x86-64). Homebrew installation is tested separately on systems supported by Homebrew. macOS 13 and 14 are outside the supported range.

macOS archives are unsigned and are not notarized. Verify the downloaded archive against the release checksums and its GitHub build attestation before running it. See [release verification](docs/releasing.md). If Gatekeeper blocks a verified executable, remove quarantine only from that file with `xattr -d com.apple.quarantine /absolute/path/to/fluxcope`; do not disable Gatekeeper globally.

## Run

```sh
fluxcope --help
fluxcope --version
fluxcope
```

Point your client's HTTP/HTTPS proxy at this machine and the port shown in the status bar (default 8989). The proxy listens on **all IPv4 interfaces**, with no client authentication. Run it only on a trusted network or restrict access with your host firewall. Recording can contain credentials, cookies and personal data.

Settings live in `~/.fluxcope/config.yml`. Fluxcope does not read or migrate state from earlier development builds under another application name. Proxy settings include optional named presets, map-remote, map-local and recording URL filters; edit them through the settings UI.

### MCP control plane

MCP is disabled by default and currently requires Unix. Start a proxy with MCP enabled, then let your MCP client launch `fluxcope mcp` as its stdio server:

```sh
fluxcope --mcp
# Or use an isolated loopback proxy with ephemeral settings:
fluxcope --host 127.0.0.1 --port 8899 --mcp
```

The broker discovers same-user instances under `~/.fluxcope/run/instances`; it does not start proxies. Pin both the returned endpoint and run ID when inspecting or changing an instance. Any process running as the same OS user can access MCP-enabled instances, including captured credentials and mapping controls.

When native MCP tools are unavailable, use the managed CLI client:

```sh
fluxcope mcp tools
fluxcope mcp tools preview_mapping_mutation
fluxcope mcp call list_instances --arguments '{}'
fluxcope mcp call get_mapping_settings --arguments @arguments.json
fluxcope mcp read '<returned fluxcope:// resource URI>'
```

The tool set supports bounded capture/body inspection, recording control, and revision-safe mapping changes. Scope settings reads by preset/table, preview a typed mutation, and commit using the evaluated revision. Preview performs neither file reads nor network requests; conflicting revisions and unfinished TUI drafts reject writes. The client manages initialization, pipes, deadlines, and output normalization without automatically retrying mutations. For a five-minute capture wait, use `--timeout-secs 360`.

MCP schemas use `fluxcope_version`, resource URIs use `fluxcope://`, and resource pagination metadata lives under `_meta.fluxcope`. Development-era Wirelens names and state directories are not compatibility aliases; rebuild the client and restart the proxy together.

The portable [Fluxcope MCP agent skill](skills/fluxcope-skill/SKILL.md) guides agents through setup, capture inspection, and mapping changes. Skill 1.0.2 targets the MCP interface in Fluxcope 0.2.0. Install the pinned skill release for Codex and Claude Code:

```sh
npx skills@1.7.0 add \
  https://github.com/LorNtz/fluxcope/tree/skill-v1.0.2/skills/fluxcope-skill \
  --agent codex claude-code --global
```

Omit `--global` for project scope, or choose only the agent you use. Skill installation is separate from the application and MCP client registration; the [setup guide](skills/fluxcope-skill/references/setup.md) covers that separation and preview-build limitations. Pinned installs stay on their chosen tag; install a newer tag explicitly to upgrade. See the [skill changelog](skills/CHANGELOG.md) and [skill release procedure](docs/skill-releases.md).

To follow the default branch instead, use `npx skills@1.7.0 add LorNtz/fluxcope --skill fluxcope-skill --agent codex claude-code --global`. A direct GitHub folder URL must include `/tree/<branch-or-tag>/skills/fluxcope-skill`; the CLI does not use `/skills/fluxcope-skill` alone as a folder selector.

Version 1.0.1 renames the installed skill from `fluxcope-mcp` to `fluxcope-skill`. After installing the new name, remove the old entry with `npx skills@1.7.0 remove fluxcope-mcp --agent codex claude-code --global`, using the same agents and scope as the old installation. Releases 1.0.0 and 1.0.1 were withdrawn to correct repository attribution; their tag names are retired. Use 1.0.2 or later.

## HTTPS and the local CA

Fluxcope creates a local certificate authority in `~/.fluxcope/certificate/`. For HTTPS inspection, explicitly trust **only the public certificate** `fluxcope-ca.pem` in the client you are testing. Press `c` to show the temporary certificate download URL and QR code. The download server is reachable from the local network while the app is running; it serves the public certificate only.

Never share `ca_key.der` or your state directory. Never disable client certificate verification as an alternative to trusting the test CA. When debugging is finished, disable the client's proxy and remove the Fluxcope CA from every client or trust store where you installed it. Deleting the local CA files does not remove trust from those clients. Stop Fluxcope before removing its local state.

## Keyboard controls

| Key | Action |
| --- | --- |
| `Tab` / `Shift+Tab` | Move panel focus |
| `Ctrl+h/j/k/l` | Directional focus |
| Arrow keys / `h/j/k/l` | Navigate the focused view |
| `r` | Toggle request recording |
| `/` | Search the request tree or entered body editor |
| `Enter` / `Esc` | Enter or leave a read-only body editor |
| `d` / `D` | Delete the selected request/subtree or all captures |
| `c` | Show the CA certificate download popup |
| `m` | Open settings |
| `@` | Toggle the log view |
| `q` | Quit the application |

Bodies support Vim-style navigation, selection/copy and visible-text jumps. Mouse navigation and scrolling are supported. The log view overlays the workspace while preserving the status bar.

## Development and security

See [CONTRIBUTING.md](CONTRIBUTING.md), [SECURITY.md](SECURITY.md), and the [release runbook](docs/releasing.md). Licensed under either [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
