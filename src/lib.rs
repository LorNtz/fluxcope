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
