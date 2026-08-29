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
    runtime::run(cli::ProxyStartup::default()).await
}
