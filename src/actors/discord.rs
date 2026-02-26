use actix::prelude::*;
use serenity::all::{ChannelId, CreateAttachment, Http, MessageFlags};
use std::sync::Arc;
use base64::Engine;
use serde_json::json;

use crate::models::DetectionMessage;
use crate::settings::SettingsCache;

// Message for the DiscordActor to send a detection
#[derive(Message, Clone)]
#[rtype(result = "Result<(), String>")]
pub struct SendDetection {
    pub detection: DetectionMessage,
}

pub struct DiscordActor {
    http_client: Arc<Http>,
    settings: SettingsCache,
}

impl DiscordActor {
    pub fn new(http_client: Arc<Http>, settings: SettingsCache) -> Self {
        Self { http_client, settings }
    }
}

impl Actor for DiscordActor {
    type Context = Context<Self>;

    fn started(&mut self, _ctx: &mut Context<Self>) {
        tracing::info!("DiscordActor started.");
    }

    fn stopped(&mut self, _ctx: &mut Context<Self>) {
        tracing::info!("DiscordActor stopped.");
    }
}

impl Handler<SendDetection> for DiscordActor {
    type Result = Result<(), String>;

    fn handle(&mut self, msg: SendDetection, ctx: &mut Context<Self>) -> Self::Result {
        tracing::debug!("DiscordActor received SendDetection: {}", msg.detection.message);

        let image_bytes = match base64::engine::general_purpose::STANDARD.decode(&msg.detection.image) {
            Ok(b) => b,
            Err(e) => {
                let err = format!("Failed to decode image: {e}");
                tracing::error!("{}", err);
                return Err(err);
            }
        };

        let http_client = self.http_client.clone();
        let settings = self.settings.clone();
        let text = msg.detection.message.clone();

        let fut = async move {
            let guilds: Vec<_> = {
                let map = settings.read().await;
                map.values()
                    // TODO: also enforce min_confidence threshold and active_hours_start/end time window
                    .filter(|g| g.bot_enabled)
                    .cloned()
                    .collect()
            };

            if guilds.is_empty() {
                tracing::warn!("No enabled guilds in settings cache, dropping detection.");
                return;
            }

            for guild in guilds {
                let mut content = text.clone();
                if let Some(role_id) = guild.role_id {
                    content.push_str(&format!("\n\n<@&{role_id}>"));
                }

                let attachment = CreateAttachment::bytes(image_bytes.clone(), "image.jpg");
                let payload = json!({
                    "content": content,
                    "flags": MessageFlags::SUPPRESS_EMBEDS.bits()
                });

                match http_client
                    .send_message(ChannelId::from(guild.channel_id), vec![attachment], &payload)
                    .await
                {
                    Ok(_) => tracing::info!("Sent detection to guild {} channel {}", guild.guild_id, guild.channel_id),
                    Err(e) => tracing::error!("Failed to send to guild {} channel {}: {:?}", guild.guild_id, guild.channel_id, e),
                }
            }
        };

        ctx.spawn(fut.into_actor(self));
        Ok(())
    }
}
