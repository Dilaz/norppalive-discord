use actix::prelude::*;
use base64::Engine;
use serde_json::json;
use serenity::all::{ChannelId, CreateAttachment, Http, MessageFlags};
use std::sync::Arc;

use crate::kafka_producer::BotKafkaProducer;
use crate::models::DetectionMessage;
use crate::settings::SettingsCache;

/// Maximum base64-encoded image size (10 MB decoded ≈ 13.3 MB base64)
const MAX_IMAGE_BASE64_LEN: usize = 14 * 1024 * 1024;

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
        Self {
            http_client,
            settings,
            kafka_producer,
        }
    }
}

fn classify_discord_error_code(code: isize) -> &'static str {
    match code {
        50001 | 50013 => "missing_permissions",
        10003 => "channel_not_found",
        _ => "unknown",
    }
}

fn classify_discord_error(error: &serenity::Error) -> String {
    if let serenity::Error::Http(serenity::http::HttpError::UnsuccessfulRequest(resp)) = error {
        classify_discord_error_code(resp.error.code).to_string()
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
        tracing::debug!(
            "DiscordActor received SendDetection: {}",
            msg.detection.message
        );

        if msg.detection.image.len() > MAX_IMAGE_BASE64_LEN {
            let err = format!(
                "Image payload too large ({} bytes), dropping",
                msg.detection.image.len()
            );
            tracing::error!("{}", err);
            return Err(err);
        }

        let image_bytes =
            match base64::engine::general_purpose::STANDARD.decode(&msg.detection.image) {
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
        let text = msg.detection.message;
        let text_detection_type = msg.detection.detection_type;

        let fut = async move {
            let guilds: Vec<_> = {
                let map = settings.read().await;
                map.values().filter(|g| g.bot_enabled).cloned().collect()
            };

            if guilds.is_empty() {
                tracing::warn!("No enabled guilds in settings cache, dropping detection.");
                return;
            }

            let is_rock = text_detection_type.as_deref() == Some("rock");

            for guild in guilds {
                // Determine target channel and role based on detection type
                let (target_channel, target_role) = if is_rock {
                    if !guild.rock_detection_enabled {
                        continue;
                    }
                    match guild.rock_channel_id {
                        Some(ch) => (ch, guild.rock_role_id),
                        None => continue,
                    }
                } else {
                    (guild.channel_id, guild.role_id)
                };

                let mut content = text.clone();
                if let Some(role_id) = target_role {
                    content.push_str(&format!("\n\n<@&{role_id}>"));
                }

                let attachment = CreateAttachment::bytes(image_bytes.clone(), "image.jpg");
                let payload = json!({
                    "content": content,
                    "flags": MessageFlags::SUPPRESS_EMBEDS.bits()
                });

                match http_client
                    .send_message(ChannelId::from(target_channel), vec![attachment], &payload)
                    .await
                {
                    Ok(_) => tracing::info!(
                        "Sent detection to guild {} channel {} (type: {})",
                        guild.guild_id,
                        target_channel,
                        if is_rock { "rock" } else { "seal" }
                    ),
                    Err(e) => {
                        let error_type = classify_discord_error(&e);
                        let error_str = format!("{e:?}");
                        tracing::error!(
                            "Failed to send to guild {} channel {}: {error_str}",
                            guild.guild_id,
                            target_channel
                        );

                        let error_payload = crate::kafka_producer::BotErrorPayload {
                            guild_id: guild.guild_id.clone(),
                            channel_id: target_channel.to_string(),
                            error_type,
                            error_message: error_str,
                            timestamp: chrono::Utc::now().to_rfc3339(),
                        };
                        if let Err(ke) = kafka_producer_clone
                            .publish_error_event(&error_payload)
                            .await
                        {
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
    fn classify_missing_access() {
        assert_eq!(classify_discord_error_code(50001), "missing_permissions");
    }

    #[test]
    fn classify_missing_permissions() {
        assert_eq!(classify_discord_error_code(50013), "missing_permissions");
    }

    #[test]
    fn classify_unknown_channel() {
        assert_eq!(classify_discord_error_code(10003), "channel_not_found");
    }

    #[test]
    fn classify_other_discord_error() {
        assert_eq!(classify_discord_error_code(50035), "unknown");
    }

    #[test]
    fn classify_non_http_error() {
        let err = serenity::Error::Other("something");
        assert_eq!(classify_discord_error(&err), "unknown");
    }
}
