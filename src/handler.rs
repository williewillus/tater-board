use std::{
    collections::{hash_map, HashMap, HashSet},
    convert::TryFrom,
    fs,
    path::Path,
    path::PathBuf,
    sync::Arc,
};

use fs::File;
use serde::{Deserialize, Serialize};
use serenity::{
    async_trait,
    model::{
        channel::Message,
        channel::Reaction,
        channel::ReactionType,
        gateway::Ready,
        id::ChannelId,
        id::GuildId,
        id::MessageId,
        id::{EmojiId, UserId},
        Permissions,
    },
    prelude::*,
};

/// Has an arc-muxed wrapper to the true handlers
pub struct HandlerWrapper {
    handlers: Arc<Mutex<HashMap<GuildId, Handler>>>,
    save_dir_path: PathBuf,
}

impl HandlerWrapper {
    /// Try to load from the given file, or just create default if it can't
    pub fn new(save_path: PathBuf) -> Self {
        let map = match save_path.read_dir() {
            Ok(dir) => dir
                .filter_map(|entry| {
                    // let let let
                    let entry = entry.ok()?;
                    let file = File::open(entry.path()).ok()?;
                    let handler = serde_json::from_reader(file).ok()?;
                    let file_path = PathBuf::from(entry.file_name());
                    let guild_id = file_path.file_stem()?;
                    let guild_id = GuildId(guild_id.to_string_lossy().parse().ok()?);
                    println!("- loaded {}", &entry.file_name().to_string_lossy());
                    Some((guild_id, handler))
                })
                .collect(),
            Err(..) => HashMap::new(),
        };
        Self {
            handlers: Arc::new(Mutex::new(map)),
            save_dir_path: save_path,
        }
    }

    /// save this to json
    async fn save<P: AsRef<Path>>(
        path: P,
        handlers: &HashMap<GuildId, Handler>,
    ) -> Result<(), String> {
        handlers
            .iter()
            .map(|(id, handler)| {
                let file = File::create(path.as_ref().join(format!("{}.json", id)))
                    .map_err(|e| e.to_string())?;
                serde_json::to_writer_pretty(file, handler).map_err(|e| e.to_string())
            })
            .collect::<Result<Vec<_>, String>>()
            .map(|_| ())
    }
}

#[derive(Serialize, Deserialize)]
struct Handler {
    /// Configuration
    config: Config,

    /// Cache of messages with any taters on them, mapping IDs to number of taters gotten
    /// and the ID of the person who sent it
    tatered_messages: HashMap<MessageId, TateredMessage>,
    /// How many taters each user has accumulated
    taters_got: HashMap<UserId, u64>,
    /// How many taters each user has posted
    taters_given: HashMap<UserId, u64>,
}

impl Handler {
    fn new() -> Self {
        Self {
            config: Config::new(),
            tatered_messages: HashMap::new(),
            taters_got: HashMap::new(),
            taters_given: HashMap::new(),
        }
    }
}

#[async_trait]
impl EventHandler for HandlerWrapper {
    async fn ready(&self, _: Context, ready: Ready) {
        println!(
            "{}#{} is connected!",
            ready.user.name, ready.user.discriminator
        );
    }

    async fn reaction_add(&self, ctx: Context, reaction: Reaction) {
        let guild_id = match reaction.guild_id {
            Some(it) => it,
            None => return,
        };
        let mut handlers = self.handlers.lock().await;
        let this = handlers.entry(guild_id).or_insert_with(Handler::new);

        let res: Result<(), SerenityError> = try {
            if this.config.tater_emoji != reaction.emoji {
                return;
            }
            // ok this is a tater!
            let giver = reaction.user(&ctx.http).await?;

            // Update taters received and taters on this message via the cache
            let tatered_message = {
                let tatered_message = this.tatered_messages.entry(reaction.message_id);
                let tatered_message = match tatered_message {
                    hash_map::Entry::Occupied(o) => o.into_mut(),
                    hash_map::Entry::Vacant(v) => {
                        // this is empty, so we need to fill the cache
                        let message = reaction.message(&ctx.http).await?;
                        v.insert(TateredMessage::new(message.author.id, 0, None))
                    }
                };
                if Some(tatered_message.sender) == reaction.user_id {
                    // hey you can't do your own message!
                    return;
                }
                // one more potato on this message
                tatered_message.count += 1;
                // smuggle out the message to avoid borrow errors
                tatered_message.clone()
            };

            // the giver gave one more potato
            *this.taters_given.entry(giver.id).or_insert(0) += 1;
            // this person got one more potato
            *this.taters_got.entry(tatered_message.sender).or_insert(0) += 1;

            let new_pin_id = update_pin_message(this, &tatered_message, &reaction, &ctx).await?;
            if let Some(tm) = this.tatered_messages.get_mut(&reaction.message_id) {
                tm.pin_id = new_pin_id
            }
        };
        if let Err(oh_no) = res {
            eprintln!("`reaction_add`: {}", oh_no);
        }
    }

    async fn reaction_remove(&self, ctx: Context, reaction: Reaction) {
        let guild_id = match reaction.guild_id {
            Some(it) => it,
            None => return,
        };
        let mut handlers = self.handlers.lock().await;
        let this = handlers.entry(guild_id).or_insert_with(Handler::new);

        let res: Result<(), SerenityError> = try {
            if this.config.tater_emoji != reaction.emoji {
                return;
            }
            // ok this is a tater!
            let ungiver = reaction.user(&ctx.http).await?;

            // Update taters received and taters on this message via the cache
            let tatered_message = {
                let tatered_message = this.tatered_messages.entry(reaction.message_id);
                let tatered_message = match tatered_message {
                    hash_map::Entry::Occupied(o) => o.into_mut(),
                    hash_map::Entry::Vacant(..) => {
                        // this should never be an empty entry
                        eprintln!("`reaction_remove`: there was an empty entry in `tatered_messages`. This probably means someone un-reacted to a message this bot did not know about, from before the bot was introduced.");
                        return;
                    }
                };
                // one fewer potato on this message
                tatered_message.count -= 1;
                // smuggle out the message to avoid borrow errors
                tatered_message.clone()
            };

            // the ungiver reduces potato
            *this.taters_given.entry(ungiver.id).or_insert(0) -= 1;
            // this person lost a potato
            *this.taters_got.entry(tatered_message.sender).or_insert(0) -= 1;

            let new_pin_id = update_pin_message(this, &tatered_message, &reaction, &ctx).await?;
            if let Some(tm) = this.tatered_messages.get_mut(&reaction.message_id) {
                tm.pin_id = new_pin_id
            }
        };
        if let Err(oh_no) = res {
            eprintln!("`reaction_remove`: {}", oh_no);
        }
    }

    async fn message(&self, ctx: Context, message: Message) {
        let guild_id = match message.guild_id {
            Some(it) => it,
            None => return,
        };
        let mut handlers = self.handlers.lock().await;
        let this = handlers.entry(guild_id).or_insert_with(Handler::new);

        let res: Result<(), SerenityError> = try {
            if message.author.id == ctx.http.get_current_user().await?.id
                || !message.content.starts_with("potato")
            {
                return;
            }

            // Check if they are an admin
            let guild = match message.guild(&ctx.cache).await {
                Some(it) => it,
                None => return,
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
                None => return,
            };
            // i'm also an "admin"
            let is_admin = is_admin || message.author.id == 273636822565912578;

            let split = message.content.split_whitespace().collect::<Vec<_>>();
            if split.len() < 2 {
                return;
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
                        let mut scores: Vec<_> =
                            map.iter().map(|(id, count)| (*id, *count)).collect();
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
                        let channel_id =
                            ChannelId(channel_id.parse::<u64>().map_err(|e| e.to_string())?);
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
                        let channel_id =
                            ChannelId(channel_id.parse::<u64>().map_err(|e| e.to_string())?);
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
                        let channel_id =
                            ChannelId(channel_id.parse::<u64>().map_err(|e| e.to_string())?);
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
                        let potato_react =
                            ReactionType::try_from(*emoji).map_err(|e| e.to_string())?;
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
                        HandlerWrapper::save(&self.save_dir_path, &*handlers)
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
        };
        if let Err(oh_no) = res {
            eprintln!("`message`: {}", oh_no);
        }
    }
}

/// Return what we need to update the pin message ID to
async fn update_pin_message(
    this: &mut Handler,
    tatered_message: &TateredMessage,
    reaction: &Reaction,
    ctx: &Context,
) -> Result<Option<MessageId>, SerenityError> {
    let medal_idx = (tatered_message.count as f32 / this.config.threshold as f32)
        .log2()
        .floor();
    let medal: &str = if medal_idx >= 0.0 {
        // we made it, nice
        match this.config.medals.get(medal_idx as usize) {
            Some(it) => it.as_str(),
            None => this.config.medals.last().map(|s| s.as_str()).unwrap_or("?"),
        }
    } else {
        // oh no we gotta delete that now ;-;
        if let Some(mid) = tatered_message.pin_id {
            // delete the message
            let msg = ctx
                .http
                .get_message(this.config.pin_channel.0, mid.0)
                .await?;
            msg.delete(&ctx.http).await?;
        }
        return Ok(None);
    };

    let content = format!("{} {}", medal, tatered_message.count);

    match tatered_message.pin_id {
        Some(mid) => {
            // we just need to edit the header
            let mut msg = ctx
                .http
                .get_message(this.config.pin_channel.0, mid.0)
                .await?;
            msg.edit(&ctx.http, |m| m.content(content)).await?;
            // Don't change anything
            Ok(tatered_message.pin_id)
        }
        None => {
            // Must both create and edit message
            let original_message = reaction.message(&ctx.http).await?;
            let content_safe = original_message.content_safe(&ctx.cache).await;

            let author_name = original_message
                .author_nick(&ctx.http)
                .await
                .unwrap_or_else(|| original_message.author.name.clone());
            let author_url = original_message.author.face();

            let message_link = match reaction.guild_id {
                Some(guild_id) => format!(
                    "https://discord.com/channels/{}/{}/{}",
                    guild_id.0, original_message.channel_id.0, original_message.id.0
                ),
                None => format!(
                    "https://discord.com/channels/@me/{}/{}",
                    original_message.channel_id.0, original_message.id.0
                ),
            };

            let msg = this
                .config
                .pin_channel
                .send_message(&ctx.http, |m| {
                    m.content(content).embed(|e| {
                        e.author(|a| a.name(author_name).icon_url(author_url))
                            .description(content_safe)
                            // zero width space
                            .field(
                                "\u{200b}",
                                format!("[**Click to jump to message!**]({})", message_link),
                                false,
                            )
                            .footer(|f| {
                                f.text(format!(
                                    "This user has received {}x {}",
                                    tatered_message.count, this.config.tater_emoji
                                ))
                            })
                    })
                })
                .await?;
            Ok(Some(msg.id))
        }
    }
}

/// Configuration for the handler
#[derive(Serialize, Deserialize)]
pub struct Config {
    /// Taters required for the first level of potato.
    pub threshold: u64,
    /// Potatoes displayed on the pinned message.
    /// Each one is displayed at twice the previous one.
    /// So if the threshhold is 6, a new one will show at 6, 12, 24, and 48 potatoes.
    pub medals: Vec<String>,

    /// Emoji ID that counts as a potato
    pub tater_emoji: ReactionType,
    /// Blacklisted channel IDs to not listen to potatoes on
    pub blacklisted_channels: HashSet<ChannelId>,
    /// Channel ID to send pins to
    pub pin_channel: ChannelId,
}

impl Config {
    /// Make a new Config with default values
    fn new() -> Self {
        Self {
            threshold: 5,
            medals: vec![
                "🥔".to_owned(),
                "🍠".to_owned(),
                "<:tinypotato:735938441505931286>".to_owned(),
                "<:angerypotato:559818417654333461>".to_owned(),
                "<:concernedpotato:711936190080876584>".to_owned(),
                "<a:pattato:754104288078331955>".to_owned(),
            ],
            tater_emoji: ReactionType::Custom {
                animated: false,
                id: EmojiId(735938441505931286),
                name: Some("tinypotato".to_owned()),
            },
            blacklisted_channels: HashSet::new(),
            pin_channel: ChannelId(0),
        }
    }
}

/// Handle to a message with potatoes on it
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TateredMessage {
    /// ID of the sender
    sender: UserId,
    /// Number of taters on it
    count: u64,
    /// If this is pinned, has the ID of the pin message
    pin_id: Option<MessageId>,
}

impl TateredMessage {
    fn new(sender: UserId, tater_count: u64, pin: Option<MessageId>) -> Self {
        Self {
            sender,
            count: tater_count,
            pin_id: pin,
        }
    }
}
