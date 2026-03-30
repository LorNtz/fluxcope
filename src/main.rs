mod app;
mod ca;
mod logging;
mod proxy_handler;
mod runtime;
mod ui;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    runtime::run().await
}
