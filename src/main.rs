#[tokio::main]
async fn main() -> anyhow::Result<()> {
    wirelens::run().await
}
