mod commands;
mod updates;

use std::{
    collections::{hash_map, HashMap, HashSet},
    fs::File,
    path::Path,
    path::PathBuf,
    sync::Arc,
};

use anyhow::{anyhow, bail, Context as AnyhowContext};
use itertools::Itertools;
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
        interactions::{Interaction, InteractionType},
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

    bot_user_id: Arc<RwLock<Option<UserId>>>,
}

impl HandlerWrapper {
    /// Try to load from the given file, or just create default if it can't
    pub fn new(save_path: PathBuf) -> Result<Self, anyhow::Error> {
        let map = match save_path.read_dir() {
            Ok(dir) => dir
                // Map to only the file stems
                .filter_map(|entry| {
                    let entry = entry.ok()?;
                    let path = entry.path();
                    let stem = path.file_stem()?;
                    let stem = stem.to_string_lossy();
                    let stem = stem.into_owned();
                    // Now get out the ID part first
                    let id_end_idx = stem.find(|c: char| !c.is_numeric())?;
                    let id = &stem[..id_end_idx];
                    let id = id.parse::<u64>().ok()?;
                    Some(id)
                })
                // Unique-ify the stems
                // this is because `xyz_taters.json` and `xyz_config.json` will both be loaded
                // otherwise we would try to open those twice.
                // We are collecting to a hashmap, not a vec, so technically we don't *need*
                // to do this but it probably saves some time
                .unique()
                // Now open up the files
                .filter_map(|id| {
                    let tater_filepath = format!("{}_taters.json", id);
                    let tater_file = File::open(save_path.join(tater_filepath)).ok()?;
                    let config_filepath = format!("{}_config.json", id);
                    let config_file = File::open(save_path.join(config_filepath)).ok()?;

                    let taters: HandlerButOnlyTaters = serde_json::from_reader(tater_file).ok()?;
                    let config: Config = serde_json::from_reader(config_file).ok()?;
                    log::info!("Loaded taters and config for guild {}", id);
                    Some((
                        GuildId(id),
                        Handler {
                            config,
                            tatered_messages: taters.tatered_messages,
                            taters_given: taters.taters_given,
                            taters_got: taters.taters_got,
                        },
                    ))
                })
                .collect(),
            Err(e) => {
                bail!(e);
            }
        };
        Ok(Self {
            handlers: Arc::new(Mutex::new(map)),
            save_dir_path: save_path,
            updates: Arc::new(Mutex::new(Updates::new())),
            bot_user_id: Arc::new(RwLock::new(None)),
        })
    }

    /// Save the taters only to json
    async fn save_taters<P: AsRef<Path>>(
        path: P,
        handlers: &HashMap<GuildId, Handler>,
    ) -> Result<(), anyhow::Error> {
        for (&id, _) in handlers.iter() {
            HandlerWrapper::save_server_taters(&path, &handlers, id).await?;
        }

        Ok(())
    }

    /// Save one server's taters to json
    async fn save_server_taters<P: AsRef<Path>>(
        path: P,
        handlers: &HashMap<GuildId, Handler>,
        guild: GuildId,
    ) -> Result<(), anyhow::Error> {
        let handler = handlers
            .get(&guild)
            .ok_or_else(|| anyhow!("Guild id {} didn't exist somehow", guild.0))?;
        let file = File::create(path.as_ref().join(format!("{}_taters.json", guild)))?;

        // Make the wrapper struct
        let hbot = HandlerButOnlyTatersRef {
            tatered_messages: &handler.tatered_messages,
            taters_given: &handler.taters_given,
            taters_got: &handler.taters_got,
        };
        serde_json::to_writer(file, &hbot)?;
        log::debug!("Saved taters for guild {:?}", guild);
        Ok(())
    }

    /// Save the configuration only to json
    async fn save_config<P: AsRef<Path>>(
        path: P,
        handlers: &HashMap<GuildId, Handler>,
    ) -> Result<(), anyhow::Error> {
        for (&id, _) in handlers.iter() {
            HandlerWrapper::save_server_config(&path, &handlers, id).await?
        }
        Ok(())
    }

    /// Save one server's config to json
    async fn save_server_config<P: AsRef<Path>>(
        path: P,
        handlers: &HashMap<GuildId, Handler>,
        guild: GuildId,
    ) -> Result<(), anyhow::Error> {
        let handler = handlers
            .get(&guild)
            .ok_or_else(|| anyhow!("Guild id {} didn't exist somehow", guild.0))?;
        let file = File::create(path.as_ref().join(format!("{}_config.json", guild)))?;
        serde_json::to_writer(file, &handler.config)?;
        log::debug!("Saved config for guild {:?}", guild);
        Ok(())
    }

    /// Save EVERYTHING
    async fn save_all<P: AsRef<Path>>(
        path: P,
        handlers: &HashMap<GuildId, Handler>,
    ) -> Result<(), anyhow::Error> {
        HandlerWrapper::save_config(&path, handlers).await?;
        HandlerWrapper::save_taters(&path, handlers).await?;
        Ok(())
    }

    async fn bot_uid(&self) -> UserId {
        let g = self.bot_user_id.read().await;
        g.expect("Asking for bot UID before Ready event has been received")
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

/// Wrapper struct that only stores info about the taters so `Handler`
/// can be deserialized piecewise
#[derive(Deserialize)]
struct HandlerButOnlyTaters {
    tatered_messages: HashMap<MessageId, TateredMessage>,
    taters_got: HashMap<UserId, u64>,
    taters_given: HashMap<UserId, u64>,
}

/// Wrapper struct that only stores info about the taters so `Handler`
/// can be serialized piecewise.
/// this time with references
#[derive(Serialize)]
struct HandlerButOnlyTatersRef<'a> {
    tatered_messages: &'a HashMap<MessageId, TateredMessage>,
    taters_got: &'a HashMap<UserId, u64>,
    taters_given: &'a HashMap<UserId, u64>,
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

    async fn do_add_tater(
        &mut self,
        ctx: &Context,
        reaction: &Reaction,
        bot_uid: UserId,
    ) -> Result<(), anyhow::Error> {
        // ok this is a tater!
        let giver = reaction
            .user(&ctx.http)
            .await
            .context("Getting user for reaction")?;

        // Update taters received and taters on this message via the cache
        let tatered_message = {
            let tatered_message = self.tatered_messages.entry(reaction.message_id);
            let tatered_message = match tatered_message {
                hash_map::Entry::Occupied(o) => o.into_mut(),
                hash_map::Entry::Vacant(v) => {
                    // this is empty, so we need to fill the cache
                    let message = reaction
                        .message(&ctx.http)
                        .await
                        .with_context(|| "Getting message for reaction")?;
                    if message.author.id == bot_uid {
                        return Ok(());
                    }
                    v.insert(TateredMessage::new(message.author.id, 0, None))
                }
            };
            if Some(tatered_message.sender) == reaction.user_id {
                // hey you can't do your own message!
                return Ok(());
            }
            // one more potato on this message
            tatered_message.count += 1;
            // smuggle out the message to avoid borrow errors
            tatered_message.clone()
        };

        // the giver gave one more potato
        *self.taters_given.entry(giver.id).or_insert(0) += 1;
        // this person got one more potato
        *self.taters_got.entry(tatered_message.sender).or_insert(0) += 1;

        let new_pin_id = update_pin_message(self, &tatered_message, &reaction, &ctx)
            .await
            .context("Update pin message")?;
        if let Some(tm) = self.tatered_messages.get_mut(&reaction.message_id) {
            tm.pin_id = new_pin_id
        }
        Ok(())
    }

    async fn do_remove_tater(
        &mut self,
        ctx: &Context,
        reaction: &Reaction,
    ) -> Result<(), anyhow::Error> {
        if self.config.tater_emoji != reaction.emoji {
            return Ok(());
        }
        if self
            .config
            .blacklisted_channels
            .contains(&reaction.channel_id)
        {
            return Ok(());
        }
        // ok this is a tater!
        let ungiver = reaction.user(&ctx.http).await?;
        log::trace!(
            "tater removed by {:?} from message {:?}",
            reaction.user_id,
            reaction.message_id
        );

        // Update taters received and taters on this message via the cache
        let tatered_message = {
            let tatered_message = self.tatered_messages.entry(reaction.message_id);
            let tatered_message = match tatered_message {
                hash_map::Entry::Occupied(o) => o.into_mut(),
                hash_map::Entry::Vacant(..) => {
                    // this should never be an empty entry
                    log::error!("`reaction_remove`: there was an empty entry in `tatered_messages`. This probably means someone un-reacted to a message this bot did not know about, from before the bot was introduced.");
                    return Ok(());
                }
            };
            if Some(tatered_message.sender) == reaction.user_id {
                // hey you can't do your own message!
                return Ok(());
            }
            // one fewer potato on this message
            tatered_message.count -= 1;
            // smuggle out the message to avoid borrow errors
            tatered_message.clone()
        };

        // the ungiver reduces potato
        *self.taters_given.entry(ungiver.id).or_insert(0) -= 1;
        // this person lost a potato
        *self.taters_got.entry(tatered_message.sender).or_insert(0) -= 1;

        let new_pin_id = update_pin_message(self, &tatered_message, &reaction, &ctx)
            .await
            .context("update_pin_message")?;
        if let Some(tm) = self.tatered_messages.get_mut(&reaction.message_id) {
            tm.pin_id = new_pin_id
        }
        Ok(())
    }
}

#[async_trait]
impl EventHandler for HandlerWrapper {
    async fn ready(&self, _: Context, ready: Ready) {
        log::info!(
            "{}#{} is connected!",
            ready.user.name,
            ready.user.discriminator
        );
        let mut g = self.bot_user_id.write().await;
        *g = Some(ready.user.id);
    }

    async fn reaction_add(&self, ctx: Context, reaction: Reaction) {
        let guild_id = match reaction.guild_id {
            Some(it) => it,
            None => return,
        };
        let mut handlers = self.handlers.lock().await;
        let this = handlers.entry(guild_id).or_insert_with(Handler::new);

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

        log::trace!(
            "tater added by {:?} to message {:?}",
            reaction.user_id,
            reaction.message_id
        );
        let bot_uid = self.bot_uid().await;
        if let Err(oh_no) = this.do_add_tater(&ctx, &reaction, bot_uid).await {
            log::error!("`reaction_add`: {:?}", oh_no);
        }
    }

    async fn reaction_remove(&self, ctx: Context, reaction: Reaction) {
        let guild_id = match reaction.guild_id {
            Some(it) => it,
            None => return,
        };
        let mut handlers = self.handlers.lock().await;
        let this = handlers.entry(guild_id).or_insert_with(Handler::new);

        if let Err(oh_no) = this.do_remove_tater(&ctx, &reaction).await {
            log::error!("`reaction_remove`: {:?}", oh_no);
        }
    }

    async fn message(&self, ctx: Context, message: Message) {
        if message.author.bot {
            return;
        }

        // Try every time it sees a message.
        // I figure that's often enough
        if let Err(oh_no) = self.check_updates(&ctx).await {
            log::error!("`message`: {:?}", oh_no);
        }

        let uid = self.bot_uid().await;
        let res = commands::handle_commands(self, &ctx, uid, &message).await;
        if let Err(oh_no) = res {
            log::error!("`message`: {:?}", oh_no);
        }
    }

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        if interaction.kind == InteractionType::ApplicationCommand {
            if let Err(e) = commands::handle_slash_command(self, ctx, interaction).await {
                log::error!("Handling slash command: {}", e);
            }
        }
    }
}

/// Return what we need to update the pin message ID to
async fn update_pin_message(
    this: &mut Handler,
    tatered_message: &TateredMessage,
    reaction: &Reaction,
    ctx: &Context,
) -> Result<Option<MessageId>, anyhow::Error> {
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
            msg.delete(&ctx.http).await.context("deleting pin")?;
        }
        return Ok(None);
    };

    let content = format!("{} {}", medal, tatered_message.count);

    match tatered_message.pin_id {
        Some(mid) => {
            log::trace!("Editing existing pin message {}", mid);
            // we just need to edit the header
            let mut msg = ctx
                .http
                .get_message(this.config.pin_channel.0, mid.0)
                .await
                .with_context(|| {
                    format!(
                        "getting message {} from channel {}",
                        mid.0, this.config.pin_channel.0
                    )
                })?;
            msg.edit(&ctx.http, |m| m.content(content))
                .await
                .context("updating pin text")?;
            // Don't change anything
            Ok(tatered_message.pin_id)
        }
        None => {
            log::trace!("Creating new pin message");
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

            let previous_message_count = this
                .tatered_messages
                .iter()
                .filter_map(|(_id, msg)| {
                    if msg.sender == tatered_message.sender {
                        Some(())
                    } else {
                        None
                    }
                })
                .count()
                - 1;

            let image = original_message
                .attachments
                .get(0)
                .and_then(|att| att.dimensions().map(|_dims| &att.url));

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
                                    "This user has been pinned {} times before",
                                    previous_message_count,
                                ))
                            });
                        if let Some(image) = image {
                            e.image(image);
                        }
                        e
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
    /// The trigger word for bot administration commands
    pub trigger_word: String,

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
            trigger_word: "taterboard".to_owned(),
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
