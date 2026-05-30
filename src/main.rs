mod app;
mod ca;
mod logging;
mod mapping;
mod proxy_handler;
mod runtime;
mod settings;
mod ui;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    runtime::run().await
}
