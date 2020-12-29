use std::time::{Duration, Instant};

use serenity::{client::Context, model::prelude::Activity};

use super::HandlerWrapper;

/// Handles periodic updates and saving
pub struct Updates {
    /// Time since last save
    last_save: Instant,
    /// Time since last status change
    last_status_change: Instant,
    /// Index of the status message function.
    /// Is None if there hasn't been a status message yet.
    status_idx: Option<usize>,
}

impl Updates {
    /// Make a new one!
    pub fn new() -> Self {
        Self {
            last_save: Instant::now(),
            last_status_change: Instant::now(),
            status_idx: None,
        }
    }

    /// Save every half hour
    const SAVE_EVERY: Duration = Duration::from_secs(60 * 30);
    /// Update status every hour
    const UPDATE_EVERY: Duration = Duration::from_secs(60 * 60);
}

impl HandlerWrapper {
    /// Check if anything needs to be updated
    pub async fn check_updates(&self, ctx: &Context) -> Result<(), anyhow::Error> {
        let mut updates = self.updates.lock().await;
        let now = Instant::now();

        if now.duration_since(updates.last_save) >= Updates::SAVE_EVERY {
            // gotta save!
            log::debug!("saving at {:?}", &now);
            updates.last_save = now;
            let lock = self.handlers.lock().await;
            HandlerWrapper::save_all(&self.save_dir_path, &*lock).await?;
        }
        if now.duration_since(updates.last_status_change) >= Updates::UPDATE_EVERY
            || updates.status_idx.is_none()
        {
            log::debug!("updating status at {:?}", &now);
            updates.last_status_change = now;
            let idx = updates.status_idx.unwrap_or(0);

            // number of status messages there are
            const STATUSES_COUNT: usize = 4;

            let activity = match idx {
                0 => {
                    // Get number of potatoes awarded everywhere
                    let handlers = self.handlers.lock().await;
                    let potatoes: u64 = handlers
                        .iter()
                        .map(|(_, handler)| {
                            handler
                                .taters_given
                                .iter()
                                .map(|(_, &count)| count)
                                .sum::<u64>()
                        })
                        .sum();
                    Activity::playing(format!("with the {} potatoes given", potatoes).as_str())
                }
                1 => {
                    // Get number of servers its in
                    let handlers = self.handlers.lock().await;
                    Activity::playing(format!("in {} servers", handlers.len()).as_str())
                }
                2 => {
                    // Get number of messages listened to
                    let handlers = self.handlers.lock().await;
                    let messages: u64 = handlers
                        .iter()
                        .map(|(_, handler)| handler.tatered_messages.len() as u64)
                        .sum();
                    Activity::listening(format!("to {} potatoed messages", messages).as_str())
                }
                3 => {
                    // Get maximum potato count
                    let handlers = self.handlers.lock().await;
                    let max: u64 = handlers
                        .iter()
                        .map(|(_, handler)| {
                            handler
                                .tatered_messages
                                .iter()
                                .map(|(_id, msg)| msg.count)
                                .max()
                                .unwrap_or(0)
                        })
                        .max()
                        .unwrap_or(0);
                    Activity::competing(
                        format!("the record {} potatoes on one message", max).as_str(),
                    )
                }
                oh_no => {
                    // oh no
                    Activity::playing(
                        format!("someone file an issue on Github, the index is {} when it should be less than {}", oh_no, STATUSES_COUNT
                    ).as_str())
                }
            };
            ctx.set_activity(activity).await;

            let mut idx = idx + 1;
            if idx >= STATUSES_COUNT {
                idx = 0;
            }
            updates.status_idx = Some(idx);
        }

        Ok(())
    }
}
