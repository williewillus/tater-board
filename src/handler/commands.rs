//! Handles the commands

use std::convert::TryFrom;

use serenity::{
    client::Context,
    model::{channel::Message, channel::ReactionType, id::ChannelId, Permissions},
    prelude::*,
    Error,
};

use super::{Handler, HandlerWrapper};

pub async fn handle_commands(
    wrapper: &HandlerWrapper,
    ctx: &Context,
    message: &Message,
) -> Result<(), Error> {
    let guild_id = match message.guild_id {
        Some(it) => it,
        None => return Ok(()),
    };
    let mut handlers = wrapper.handlers.lock().await;
    let this = handlers.entry(guild_id).or_insert_with(Handler::new);

    if message.author.id == ctx.http.get_current_user().await?.id
        || !message.content.starts_with("potato")
    {
        return Ok(());
    }

    // Check if they are an admin
    let guild = match message.guild(&ctx.cache).await {
        Some(it) => it,
        None => return Ok(()),
    };
    let is_admin = match guild
        .member(&ctx.http, message.author.id)
        .await?
        .roles(&ctx.cache)
        .await
    {
        Some(roles) => roles
            .iter()
            .any(|r| r.has_permission(Permissions::ADMINISTRATOR)),
        None => return Ok(()),
    };
    // i'm also an "admin"
    let is_admin = is_admin || message.author.id == 273636822565912578;

    let split = message.content.split_whitespace().collect::<Vec<_>>();
    if split.len() < 2 {
        return Ok(());
    }
    let cmd = split[1];
    let args = &split[2..];

    match cmd {
        "help" => {
            const HELP: &str = r" === PotatoBoard Help ===
- `help`: Get this message.
- `receivers <page_number>`: See the most protatolific receivers of potatoes. `page_number` is optional.
- `givers <page_number>`: See the most protatolific givers of potatoes. `page_number` is optional.";
            const ADMIN_HELP: &str = r"You're an admin! Here's the admin commands:
- `set_pin_channel <channel_id>`: Set the channel that pinned messages to go, and adds it to the potato blacklist.
- `set_potato <emoji>`: Set the given emoji to be the operative one.
- `set_threshold <number>`: Set how many potatoes have to be on a message before it is pinned.
- `blacklist <channel_id>`: Make the channel no longer eligible for pinning messages, regardless of potato count.
- `unblacklist <channel_id>`: Unblacklist this channel so messages from it can be pinned again.
- `save`: Save this server's information to the server the bot is running on in case it goes down.";
            message.channel_id.say(&ctx.http, HELP).await?;
            if is_admin {
                message.channel_id.say(&ctx.http, ADMIN_HELP).await?;
            }
        }
        leaderboard @ "receivers" | leaderboard @ "givers" => {
            const PAGE_SIZE: usize = 10;
            let res: Result<(), String> = try {
                let page_num = args.get(0).and_then(|page| page.parse().ok()).unwrap_or(0);

                let map = if leaderboard == "receivers" {
                    &this.taters_got
                } else {
                    &this.taters_given
                };
                // high score at the front
                let mut scores: Vec<_> = map.iter().map(|(id, count)| (*id, *count)).collect();
                scores.sort_by_key(|(_id, count)| *count);
                scores.reverse();
                // de-mut it
                let scores = scores;

                let to_display = scores
                    .iter()
                    .enumerate()
                    .skip(PAGE_SIZE * page_num)
                    .take(PAGE_SIZE);

                let mut board = String::with_capacity(20 * PAGE_SIZE);
                let verb = if leaderboard == "receivers" {
                    "received"
                } else {
                    "given"
                };
                for (idx, (user_id, count)) in to_display {
                    let medal = match idx + 1 {
                        1 => "🏅",
                        2 => "🥈",
                        3 => "🥉",
                        _ => "🎖️",
                    };
                    let user = ctx
                        .http
                        .get_user(user_id.0)
                        .await
                        .map_err(|e| e.to_string())?;

                    board.push_str(&format!(
                        "{} {}: {} has {} {}x {}\n",
                        medal,
                        idx + 1,
                        user.mention(),
                        verb,
                        count,
                        this.config.tater_emoji.to_string()
                    ));
                }

                let asker_place = scores
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, (id, score))| {
                        if *id == message.author.id {
                            Some((idx + 1, score))
                        } else {
                            None
                        }
                    })
                    .next();
                let (place, score) = match asker_place {
                    Some((p, s)) => (p.to_string(), s.to_string()),
                    None => ("?".to_string(), "?".to_string()),
                };
                let total_pages = map.len() / PAGE_SIZE + 1;
                let footer = format!(
                    "Your place: #{}/{} with {}x {} | Page {}/{}",
                    place,
                    map.len(),
                    score,
                    this.config.tater_emoji.to_string(),
                    page_num + 1,
                    total_pages
                );

                message
                    .channel_id
                    .send_message(&ctx.http, |m| {
                        m.embed(|e| {
                            e.title(format!("Leaderboard - Taters {}", verb))
                                .description(&board)
                                .footer(|f| f.text(footer))
                        })
                    })
                    .await
                    .map_err(|e| e.to_string())?;
            };
            if let Err(oh_no) = res {
                message
                    .channel_id
                    .say(&ctx.http, format!("An error occured: \n{}", oh_no))
                    .await?;
            };
        }
        "set_pin_channel" if is_admin => {
            let msg: Result<String, String> = try {
                let channel_id = args
                    .get(0)
                    .ok_or_else(|| String::from("Not enough arguments (1 expected)"))?;
                let channel_id = ChannelId(channel_id.parse::<u64>().map_err(|e| e.to_string())?);
                this.config.pin_channel = channel_id;

                let existed = !this.config.blacklisted_channels.insert(channel_id);
                if !existed {
                    format!(
                        "Set pins channel to `{}` and added it to the blacklist",
                        channel_id
                    )
                } else {
                    format!(
                        "Set pins channel to `{}`, and it was already blacklisted",
                        channel_id
                    )
                }
            };
            match msg {
                Ok(msg) => message.channel_id.say(&ctx.http, msg).await?,
                Err(oh_no) => {
                    message
                        .channel_id
                        .say(&ctx.http, format!("An error occured: \n{}", oh_no))
                        .await?
                }
            };
        }
        "set_threshold" if is_admin => {
            let msg: Result<String, String> = try {
                let threshold = args
                    .get(0)
                    .ok_or_else(|| String::from("Not enough arguments (1 expected)"))?;
                let threshold = threshold.parse::<u64>().map_err(|e| e.to_string())?;
                this.config.threshold = threshold;
                format!("Threshold changed to {}", threshold)
            };
            match msg {
                Ok(msg) => message.channel_id.say(&ctx.http, msg).await?,
                Err(oh_no) => {
                    message
                        .channel_id
                        .say(&ctx.http, format!("An error occured: \n{}", oh_no))
                        .await?
                }
            };
        }
        "blacklist" if is_admin => {
            let msg: Result<String, String> = try {
                let channel_id = args
                    .get(0)
                    .ok_or_else(|| String::from("Not enough arguments (1 expected)"))?;
                let channel_id = ChannelId(channel_id.parse::<u64>().map_err(|e| e.to_string())?);
                let existed = !this.config.blacklisted_channels.insert(channel_id);
                if !existed {
                    format!("Blacklisted `{}`", channel_id)
                } else {
                    format!("`{}` was already blacklisted", channel_id)
                }
            };
            match msg {
                Ok(msg) => message.channel_id.say(&ctx.http, msg).await?,
                Err(oh_no) => {
                    message
                        .channel_id
                        .say(&ctx.http, format!("An error occured: \n{}", oh_no))
                        .await?
                }
            };
        }
        "unblacklist" if is_admin => {
            let msg: Result<String, String> = try {
                let channel_id = args
                    .get(0)
                    .ok_or_else(|| String::from("Not enough arguments (1 expected)"))?;
                let channel_id = ChannelId(channel_id.parse::<u64>().map_err(|e| e.to_string())?);
                let existed = this.config.blacklisted_channels.remove(&channel_id);
                if existed {
                    format!("Unblacklisted `{}`", channel_id)
                } else {
                    format!("`{}` was not blacklisted", channel_id)
                }
            };
            match msg {
                Ok(msg) => message.channel_id.say(&ctx.http, msg).await?,
                Err(oh_no) => {
                    message
                        .channel_id
                        .say(&ctx.http, format!("An error occured: \n{}", oh_no))
                        .await?
                }
            };
        }
        "set_potato" if is_admin => {
            let msg: Result<String, String> = try {
                let emoji = args
                    .get(0)
                    .ok_or_else(|| String::from("Not enough arguments (1 expected)"))?;
                let potato_react = ReactionType::try_from(*emoji).map_err(|e| e.to_string())?;
                let old_react = this.config.tater_emoji.to_string();
                this.config.tater_emoji = potato_react;
                format!("Set potato emoji to {} (from {})", emoji, old_react)
            };
            match msg {
                Ok(msg) => message.channel_id.say(&ctx.http, msg).await?,
                Err(oh_no) => {
                    message
                        .channel_id
                        .say(&ctx.http, format!("An error occured: \n{}", oh_no))
                        .await?
                }
            };
        }
        "save" if is_admin => {
            let msg: Result<String, String> =
                HandlerWrapper::save(&wrapper.save_dir_path, &*handlers)
                    .await
                    .map(|_| "Saved successfully!".to_string());
            match msg {
                Ok(msg) => message.channel_id.say(&ctx.http, msg).await?,
                Err(oh_no) => {
                    message
                        .channel_id
                        .say(&ctx.http, format!("An error occured: \n{}", oh_no))
                        .await?
                }
            };
        }

        // ignore other stuff
        _ => {}
    }

    Ok(())
}
