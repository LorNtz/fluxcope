mod app;
#[doc(hidden)]
pub mod benchmark_support;
mod ca;
pub mod capture;
mod cli;
#[cfg(unix)]
mod control;
#[cfg(unix)]
mod control_rpc;
mod instance;
#[cfg(unix)]
mod instance_registry;
mod logging;
mod mapping;
mod mcp;
mod private_fs;
mod proxy_handler;
mod recording;
mod request_policy;
mod request_search;
mod runtime;
mod select;
mod settings;
mod ui;
pub async fn run() -> anyhow::Result<()> {
    run_from(std::env::args_os()).await
}

pub async fn run_from<I, T>(args: I) -> anyhow::Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    match cli::parse_from(args)? {
        cli::ProcessMode::Proxy(startup) => runtime::run(startup).await,
        cli::ProcessMode::Broker => mcp::run_stdio().await,
    }
}

#[cfg(test)]
mod mcp_platform_tests {
    #[test]
    fn disabled_proxy_mcp_is_supported_on_every_target() {
        super::mcp::ensure_supported_platform(false).expect("MCP-disabled proxy");
    }

    #[test]
    fn enabled_proxy_mcp_matches_target_support() {
        let result = super::mcp::ensure_supported_platform(true);
        #[cfg(unix)]
        assert!(result.is_ok());
        #[cfg(not(unix))]
        assert!(
            result
                .expect_err("MCP-enabled proxy must be rejected")
                .to_string()
                .contains("unsupported_platform")
        );
    }
}
