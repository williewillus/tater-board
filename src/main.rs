mod handler;

use std::{env, error::Error, path::PathBuf};

use handler::HandlerWrapper;
use serenity::{client::bridge::gateway::GatewayIntents, Client};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    env_logger::init_from_env(env_logger::Env::default());
    log::info!("taterboard v{} initializing", env!("CARGO_PKG_VERSION"));

    let token = env::var("TATERBOARD_TOKEN").expect("expected bot token at env `TATERBOARD_TOKEN`");
    let path_to_save = env::args()
        .nth(1)
        .expect("must provide path to directory to save json");
    let path_to_save = PathBuf::from(path_to_save);

    log::debug!("Saving data to {:?}", path_to_save);
    tokio::fs::create_dir_all(&path_to_save).await?;

    let mut client = Client::builder(&token)
        .intents(
            GatewayIntents::GUILDS
                | GatewayIntents::GUILD_EMOJIS
                | GatewayIntents::GUILD_MESSAGES
                | GatewayIntents::GUILD_MESSAGE_REACTIONS,
        )
        .event_handler(HandlerWrapper::new(path_to_save)?)
        .await?;

    client.start().await?;
    Ok(())
}
