mod app;
mod ca;
mod logging;
mod mapping;
mod proxy_handler;
mod recording;
mod runtime;
mod select;
mod select_widget;
mod settings;
mod ui;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    runtime::run().await
}
