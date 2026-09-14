fn main() -> anyhow::Result<()> {
    clap::Command::new(env!("CARGO_PKG_NAME"))
        .version(env!("CARGO_PKG_VERSION"))
        .about(env!("CARGO_PKG_DESCRIPTION"))
        .after_help("Settings: ~/.fluxcope/config.yml\nThe proxy listens on all IPv4 interfaces. Use only on a trusted network or behind a host firewall.")
        .get_matches();

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(fluxcope::run())
}
