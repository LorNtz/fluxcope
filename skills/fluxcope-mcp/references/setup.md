# MCP setup and connection diagnosis

## Establish what is installed

Skill 1.0.0 targets the MCP interface shipped in Fluxcope 0.2.0. Other builds must expose the capabilities needed for the requested workflow.

Check the selected executable's path, `--version`, `--help`, and `mcp --help`. Use those capabilities rather than assuming a published release supports MCP. Keep the proxy and broker on the same build where possible; inspect reported `fluxcope_version` and `rpc_version` when a connection fails after an upgrade.

There are three independent pieces:

1. This skill supplies instructions to the agent.
2. An MCP-enabled Fluxcope proxy captures traffic and exposes a local control socket.
3. The agent host starts `fluxcope mcp` as a stdio broker, which discovers running proxies.

The broker does not start a proxy. The HTTP proxy port is not an HTTP MCP endpoint.

## Start and register a normal build

For an explicit setup request, inspect the existing registration first. Reuse a correct entry; repair the requested entry without replacing unrelated client settings. Follow the user's requested configuration scope. If scope is unspecified, keep an existing entry's scope; for a new entry, choose the host's project/local scope where available and state the choice.

Start a proxy in an interactive terminal with MCP enabled:

```sh
/absolute/path/to/fluxcope --mcp
```

For a separate proxy with ephemeral settings, choose a free port and bind to loopback:

```sh
/absolute/path/to/fluxcope --host 127.0.0.1 --port 8899 --mcp
```

The default launch uses the user's persistent settings. Keep the process running. Use `--config PATH` only when a read-only settings file is intended; it cannot be combined with `--host`/`--port`. Enabling MCP exposes inspection and mutation capabilities to processes running as the same OS user.

Register the executable as a **stdio** server with arguments `["mcp"]`. Use an absolute path so a GUI host can find it. Existing instances and the broker must run as the same OS user and resolve the same Fluxcope state directory through `HOME`.

For Codex, project configuration in a trusted project's `.codex/config.toml` can contain:

```toml
[mcp_servers.fluxcope]
command = "/absolute/path/to/fluxcope"
args = ["mcp"]
```

For a user-wide Codex registration, use `codex mcp add fluxcope -- /absolute/path/to/fluxcope mcp`. For Claude Code, `claude mcp add --scope local fluxcope -- /absolute/path/to/fluxcope mcp` registers it for the current project; use `--scope user` when user-wide registration is requested. Check the installed client's help if its syntax differs. Other hosts need the same stdio command and arguments in their MCP settings.

Reload the host's MCP connection, or restart the session if it cannot reload. Then verify native `list_instances` and `get_status` for the intended run. A saved registration or a successful CLI helper call verifies only that layer, not the host's native connection. If session reload is required, report the completed configuration and the remaining verification step.

Client references: [Codex MCP configuration](https://developers.openai.com/codex/mcp), [Claude Code local MCP servers](https://code.claude.com/docs/en/mcp-quickstart#add-a-local-server).

## Diagnose missing connections

- **No native tools:** inspect the host registration, absolute executable path, startup errors, and whether the current session loaded the configuration. Use the managed CLI fallback for the current task when possible; an ordinary debugging request does not require persistent host configuration changes.
- **No instances:** check that the intended proxy is still running with MCP enabled and shares the broker's OS user and `HOME`. Inspect discovery diagnostics and `get_broker_status`. An empty registry is not a reason to modify unrelated state directories.
- **Instance found but unavailable:** compare the returned run identity with the requested run, inspect the error and versions, and check for a stopped or replaced process. Preserve the original target when diagnosing the failure.
- **Connected but no captures:** follow the recording and traffic checks in [captures](captures.md). A successful MCP connection does not configure the tested application's proxy or CA trust.

## Preview builds

Inspect the preview launcher's `--help` and the project's preview documentation before registering it. A compatible launcher must support both proxy startup with MCP enabled and a concurrent stdio broker, use the same selected build and private state for both, and keep launcher output off the broker's stdout.

The repository's current preview launcher exposes install/launch operations and a port option, but does not forward `--mcp` or offer a broker mode. Do not register that launcher as `COMMAND mcp` or assume the proxy port accepts MCP. Report this launcher limitation when it applies.

Preview state can use a private `HOME`, so the normal broker may discover a different set of instances. Long preview state paths can also exceed Unix socket path limits; inspect the actual startup error rather than diagnosing every failure as a version mismatch. Changing `HOME` alone does not establish that a preview launcher supports MCP.

When the launcher lacks support, offer a source build from the desired revision, launched directly with MCP enabled, or a build already known to support it. Keep any preview broker under a distinct client entry such as `fluxcope-preview` when stable and preview installations coexist. Do not silently replace the stable registration or extract a cached preview binary to bypass its launcher lifecycle and verification.

Preview setup is complete only when the intended preview proxy starts, the host's broker discovers that run, and native `get_status` succeeds for it. If the launcher cannot do this, report setup as blocked at that layer; skill installation cannot supply the missing runtime capability.
