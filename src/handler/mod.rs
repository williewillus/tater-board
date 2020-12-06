mod commands;
mod updates;

use std::{
    collections::{hash_map, HashMap, HashSet},
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
    },
    prelude::*,
};
use updates::Updates;

/// Has an arc-muxed wrapper to the true handlers
pub struct HandlerWrapper {
    handlers: Arc<Mutex<HashMap<GuildId, Handler>>>,
    save_dir_path: PathBuf,

    /// Update info
    updates: Arc<Mutex<Updates>>,
}

impl HandlerWrapper {
    /// Try to load from the given file, or just create default if it can't
    pub fn new(save_path: PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
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
            Err(e) => {
                return Err(Box::new(e));
            }
        };
        Ok(Self {
            handlers: Arc::new(Mutex::new(map)),
            save_dir_path: save_path,
            updates: Arc::new(Mutex::new(Updates::new())),
        })
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
                serde_json::to_writer(file, handler).map_err(|e| e.to_string())
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
            if this
                .config
                .blacklisted_channels
                .contains(&reaction.channel_id)
            {
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
            if this
                .config
                .blacklisted_channels
                .contains(&reaction.channel_id)
            {
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
        // Try every time it sees a message.
        // I figure that's often enough
        if let Err(oh_no) = self.check_updates(&ctx).await {
            eprintln!("`message`: {}", oh_no);
        }

        let res = commands::handle_commands(self, &ctx, &message).await;
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

    /// people who can administrate the bot
    pub admins: HashSet<UserId>,
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
            admins: {
                let mut set = HashSet::new();
                // this is my user ID
                set.insert(UserId(273636822565912578));
                set
            },
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
