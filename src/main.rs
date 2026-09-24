mod app;
mod player;
mod storage;
mod ui;

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
  app::run().await
}
