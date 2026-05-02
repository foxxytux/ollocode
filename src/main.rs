mod app;
mod config;
mod ollama;
mod terminal;
mod tools;
mod tui;

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let config = config::Config::load()?;
    let client = ollama::OllamaClient::new(config.ollama_host.clone());
    let tools = tools::ToolRunner::new(cwd.clone());
    let app = app::App::new(cwd, config, client, tools).await;
    terminal::run(app).await
}
