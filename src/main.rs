fn main() -> anyhow::Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(fluxcope::run())
        .inspect_err(|error| {
            if let Some(clap_error) = error.downcast_ref::<clap::Error>() {
                clap_error.exit();
            }
        })
}
