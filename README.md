# Fluxcope

A terminal HTTP and HTTPS debugging proxy with request recording, searchable request trees, body inspection, and local/remote response mapping.

The first release is based on the existing mainline TUI and proxy. MCP integration is planned separately and is not included in v0.1.0.

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
