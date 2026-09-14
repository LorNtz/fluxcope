mod body;
#[cfg(unix)]
pub(crate) mod broker;
#[cfg(unix)]
mod capture;
pub(crate) mod client;
#[cfg(unix)]
mod mapping;
#[cfg(unix)]
mod prompts;
mod schema;
#[cfg(unix)]
mod stdio;
mod telemetry;

pub(crate) fn ensure_supported_platform(enabled: bool) -> anyhow::Result<()> {
    if enabled && !cfg!(unix) {
        anyhow::bail!("unsupported_platform: embedded MCP requires Unix");
    }
    Ok(())
}

#[cfg(unix)]
pub(crate) async fn run_stdio() -> anyhow::Result<()> {
    use anyhow::Context as _;
    use rmcp::ServiceExt as _;
    use tokio_util::sync::CancellationToken;

    let wirelens_home = crate::instance::wirelens_home_dir()
        .context("failed to resolve Wirelens home directory")?;
    let broker = broker::Broker::new(&wirelens_home)
        .context("failed to open the Wirelens instance registry")?;
    let cancelled = CancellationToken::new();
    let (broker, transport) = stdio::session(
        broker,
        tokio::io::stdin(),
        tokio::io::stdout(),
        cancelled.clone(),
    );
    let service = broker
        .serve_with_ct(transport, cancelled)
        .await
        .context("failed to start MCP stdio transport")?;
    service
        .waiting()
        .await
        .context("MCP stdio transport failed")?;
    Ok(())
}

#[cfg(not(unix))]
pub(crate) async fn run_stdio() -> anyhow::Result<()> {
    anyhow::bail!("unsupported_platform: the MCP broker requires Unix")
}
