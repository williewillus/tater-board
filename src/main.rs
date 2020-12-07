#![feature(try_blocks)]

mod handler;

use std::{env, error::Error, path::PathBuf};

use handler::HandlerWrapper;
use serenity::{Client, client::bridge::gateway::GatewayIntents};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    println!("taterboard v{} initializing", env!("CARGO_PKG_VERSION"));

    let token = env::var("TATERBOARD_TOKEN").expect("expected bot token at env `TATERBOARD_TOKEN`");
    let path_to_save = env::args()
        .nth(1)
        .expect("must provide path to directory to save json");
    let path_to_save = PathBuf::from(path_to_save);

    #[cfg(debug_assertions)]
    let path_to_save = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path_to_save);

    tokio::fs::create_dir_all(&path_to_save).await?;

    let mut client = Client::builder(&token)
        .intents(GatewayIntents::GUILD_MESSAGES | GatewayIntents::GUILD_MESSAGE_REACTIONS)
        .event_handler(HandlerWrapper::new(path_to_save)?)
        .await?;

    client.start().await?;
    Ok(())
}
