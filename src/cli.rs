use std::{ffi::OsString, net::IpAddr, path::PathBuf};

use clap::{CommandFactory, Parser, Subcommand, error::ErrorKind};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ProcessMode {
    Proxy(ProxyStartup),
    Broker,
    McpClient {
        command: McpClientCommand,
        timeout_secs: u64,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ConfigSelection {
    DefaultOwned,
    ReadOnlyFile(PathBuf),
    Temporary { host: IpAddr, port: u16 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum McpOverride {
    Inherit,
    Enabled,
    Disabled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProxyStartup {
    pub config: ConfigSelection,
    pub mcp: McpOverride,
}

impl Default for ProxyStartup {
    fn default() -> Self {
        Self {
            config: ConfigSelection::DefaultOwned,
            mcp: McpOverride::Inherit,
        }
    }
}

#[derive(Parser)]
#[command(
    name = "fluxcope",
    version,
    about,
    after_help = "Settings: ~/.fluxcope/config.yml\nThe proxy listens on all IPv4 interfaces. Use only on a trusted network or behind a host firewall."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
    #[arg(long, value_name = "IP")]
    host: Option<IpAddr>,
    #[arg(long, value_name = "PORT")]
    port: Option<u16>,
    #[arg(long, conflicts_with = "no_mcp")]
    mcp: bool,
    #[arg(long)]
    no_mcp: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Run the stdio broker, or execute a command through a managed broker child.
    Mcp {
        #[command(subcommand)]
        command: Option<McpClientCommand>,
        /// Whole-command deadline (default 60 seconds). Use 360 for a five-minute capture wait.
        #[arg(long, global = true, value_parser = clap::value_parser!(u64).range(1..=360))]
        timeout_secs: Option<u64>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Subcommand)]
pub(crate) enum McpClientCommand {
    /// List tool names/descriptions, or the selected tool's input schema and annotations.
    Tools { name: Option<String> },
    /// Call a tool once; arguments must be a JSON object (maximum 1 MiB).
    Call {
        tool: String,
        /// JSON, @PATH, or - to read standard input.
        #[arg(long, value_name = "JSON|@PATH|-")]
        arguments: String,
    },
    /// Read an MCP resource URI.
    Read { uri: String },
}

pub(crate) fn parse_from<I, T>(args: I) -> Result<ProcessMode, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    Cli::try_parse_from(args)?.into_process_mode()
}

impl Cli {
    fn into_process_mode(self) -> Result<ProcessMode, clap::Error> {
        if let Some(Command::Mcp {
            command,
            timeout_secs,
        }) = self.command
        {
            if self.config.is_some()
                || self.host.is_some()
                || self.port.is_some()
                || self.mcp
                || self.no_mcp
            {
                return Err(validation_error(
                    ErrorKind::ArgumentConflict,
                    "proxy options cannot be used with the mcp subcommand",
                ));
            }

            return match command {
                Some(command) => Ok(ProcessMode::McpClient {
                    command,
                    timeout_secs: timeout_secs.unwrap_or(60),
                }),
                None if timeout_secs.is_some() => Err(validation_error(
                    ErrorKind::ArgumentConflict,
                    "--timeout-secs requires an mcp client command",
                )),
                None => Ok(ProcessMode::Broker),
            };
        }

        if self.config.is_some() && (self.host.is_some() || self.port.is_some()) {
            return Err(validation_error(
                ErrorKind::ArgumentConflict,
                "--config cannot be combined with --host or --port",
            ));
        }

        let config = match (self.config, self.host, self.port) {
            (Some(path), None, None) => ConfigSelection::ReadOnlyFile(path),
            (None, None, None) => ConfigSelection::DefaultOwned,
            (None, Some(host), Some(port)) => {
                if port == 0 {
                    return Err(validation_error(
                        ErrorKind::ValueValidation,
                        "--port must be greater than zero",
                    ));
                }
                ConfigSelection::Temporary { host, port }
            }
            (None, Some(_), None) | (None, None, Some(_)) => {
                return Err(validation_error(
                    ErrorKind::MissingRequiredArgument,
                    "temporary mode requires both --host and --port",
                ));
            }
            (Some(_), _, _) => unreachable!("config conflicts are validated above"),
        };

        let mcp = if self.mcp {
            McpOverride::Enabled
        } else if self.no_mcp {
            McpOverride::Disabled
        } else {
            McpOverride::Inherit
        };

        Ok(ProcessMode::Proxy(ProxyStartup { config, mcp }))
    }
}

fn validation_error(kind: ErrorKind, message: &'static str) -> clap::Error {
    Cli::command().error(kind, message)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn default_arguments_select_owned_proxy_with_inherited_mcp() {
        assert_eq!(
            parse_from(["fluxcope"]).expect("default args"),
            ProcessMode::Proxy(ProxyStartup {
                config: ConfigSelection::DefaultOwned,
                mcp: McpOverride::Inherit,
            })
        );
        assert_eq!(
            ProxyStartup::default(),
            ProxyStartup {
                config: ConfigSelection::DefaultOwned,
                mcp: McpOverride::Inherit,
            }
        );
    }

    #[test]
    fn config_selects_read_only_proxy() {
        assert_eq!(
            parse_from(["fluxcope", "--config", "x.yml"]).expect("config args"),
            ProcessMode::Proxy(ProxyStartup {
                config: ConfigSelection::ReadOnlyFile(PathBuf::from("x.yml")),
                mcp: McpOverride::Inherit,
            })
        );
    }

    #[test]
    fn proxy_mcp_overrides_are_explicit_and_mutually_exclusive() {
        assert_eq!(
            parse_from(["fluxcope", "--mcp"]).expect("enabled MCP override"),
            ProcessMode::Proxy(ProxyStartup {
                config: ConfigSelection::DefaultOwned,
                mcp: McpOverride::Enabled,
            })
        );
        assert_eq!(
            parse_from(["fluxcope", "--no-mcp"]).expect("disabled MCP override"),
            ProcessMode::Proxy(ProxyStartup {
                config: ConfigSelection::DefaultOwned,
                mcp: McpOverride::Disabled,
            })
        );
        assert!(parse_from(["fluxcope", "--mcp", "--no-mcp"]).is_err());
    }

    #[test]
    fn temporary_mode_requires_host_and_port() {
        assert!(parse_from(["fluxcope", "--host", "127.0.0.1"]).is_err());
        assert!(parse_from(["fluxcope", "--port", "9100"]).is_err());
        assert_eq!(
            parse_from(["fluxcope", "--host", "127.0.0.1", "--port", "9100"])
                .expect("temporary args"),
            ProcessMode::Proxy(ProxyStartup {
                config: ConfigSelection::Temporary {
                    host: "127.0.0.1".parse().expect("IP"),
                    port: 9100,
                },
                mcp: McpOverride::Inherit,
            })
        );
    }

    #[test]
    fn config_rejects_bind_overrides_and_temporary_port_zero() {
        assert!(
            parse_from([
                "fluxcope",
                "--config",
                "x.yml",
                "--host",
                "127.0.0.1",
                "--port",
                "9100",
            ])
            .is_err()
        );
        assert!(parse_from(["fluxcope", "--host", "127.0.0.1", "--port", "0"]).is_err());
    }

    #[test]
    fn mcp_subcommand_selects_broker() {
        assert_eq!(
            parse_from(["fluxcope", "mcp"]).expect("broker args"),
            ProcessMode::Broker
        );
    }

    #[test]
    fn broker_rejects_proxy_flags() {
        assert!(parse_from(["fluxcope", "mcp", "--mcp"]).is_err());
        assert!(parse_from(["fluxcope", "mcp", "--config", "x.yml"]).is_err());
    }
}
