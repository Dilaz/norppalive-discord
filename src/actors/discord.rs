use actix::prelude::*;
use serenity::all::{ChannelId, CreateAttachment, Http, MessageFlags};
use std::sync::Arc;
use base64::Engine;
use serde_json::json;

use crate::kafka_producer::BotKafkaProducer;
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
    kafka_producer: Arc<BotKafkaProducer>,
}

impl DiscordActor {
    pub fn new(
        http_client: Arc<Http>,
        settings: SettingsCache,
        kafka_producer: Arc<BotKafkaProducer>,
    ) -> Self {
        Self { http_client, settings, kafka_producer }
    }
}

fn classify_discord_error(error: &str) -> String {
    let lower = error.to_lowercase();
    if lower.contains("missing access") || lower.contains("missing permissions") {
        "missing_permissions".to_string()
    } else if lower.contains("unknown channel") {
        "channel_not_found".to_string()
    } else {
        "unknown".to_string()
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
        let kafka_producer_clone = self.kafka_producer.clone();
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
                    Err(e) => {
                        let error_str = format!("{e:?}");
                        let error_type = classify_discord_error(&error_str);
                        tracing::error!("Failed to send to guild {} channel {}: {error_str}", guild.guild_id, guild.channel_id);

                        let error_payload = crate::kafka_producer::BotErrorPayload {
                            guild_id: guild.guild_id.clone(),
                            channel_id: guild.channel_id.to_string(),
                            error_type,
                            error_message: error_str,
                            timestamp: chrono::Utc::now().to_rfc3339(),
                        };
                        if let Err(ke) = kafka_producer_clone.publish_error_event(&error_payload).await {
                            tracing::error!("Failed to publish error event: {ke}");
                        }
                    }
                }
            }
        };

        ctx.spawn(fut.into_actor(self));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_missing_permissions() {
        assert_eq!(classify_discord_error("Missing Access"), "missing_permissions");
        assert_eq!(classify_discord_error("Missing Permissions"), "missing_permissions");
    }

    #[test]
    fn classify_unknown_channel() {
        assert_eq!(classify_discord_error("Unknown Channel"), "channel_not_found");
    }

    #[test]
    fn classify_unknown_error() {
        assert_eq!(classify_discord_error("some random error"), "unknown");
    }
}
