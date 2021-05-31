mod handler;

use std::{env, error::Error, path::PathBuf};

use handler::HandlerWrapper;
use serenity::model::interactions::{ApplicationCommand, ApplicationCommandOptionType};
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

    ApplicationCommand::create_global_application_commands(
        &client.cache_and_http.http,
        configure_commands,
    )
    .await?;

    client.start().await?;
    Ok(())
}

fn configure_commands(
    builder: &mut serenity::builder::CreateApplicationCommands,
) -> &mut serenity::builder::CreateApplicationCommands {
    builder
        .create_application_command(|a| {
            a.name("receivers")
                .description("Show taterboard receiver leaderboard")
                .create_option(|o| {
                    o.name("page")
                        .description("Which page of the leaderboard to show")
                        .kind(ApplicationCommandOptionType::Integer)
                        .required(false)
                })
        })
        .create_application_command(|a| {
            a.name("givers")
                .description("Show taterboard giver leaderboard")
                .create_option(|o| {
                    o.name("page")
                        .description("Which page of the leaderboard to show")
                        .kind(ApplicationCommandOptionType::Integer)
                        .required(false)
                })
        })
}
