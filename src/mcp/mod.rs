mod body;
#[cfg(unix)]
pub(crate) mod broker;
#[cfg(unix)]
mod capture;
mod schema;
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

    let wirelens_home = crate::instance::wirelens_home_dir()
        .context("failed to resolve Wirelens home directory")?;
    let broker = broker::Broker::new(&wirelens_home)
        .context("failed to open the Wirelens instance registry")?;
    let service = broker
        .serve(rmcp::transport::stdio())
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
