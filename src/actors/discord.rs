use actix::prelude::*;
use serenity::all::{ChannelId, CreateAttachment, Http, MessageFlags};
use std::sync::Arc;
use crate::models::DetectionMessage;
use base64::Engine;
use serde_json::json;
use std::env;

// Message for the DiscordActor to send a detection
#[derive(Message, Clone)]
#[rtype(result = "Result<(), String>")]
pub struct SendDetection {
    pub detection: DetectionMessage,
}

pub struct DiscordActor {
    http_client: Arc<Http>,
    channel_id: ChannelId,
}

impl DiscordActor {
    pub fn new(http_client: Arc<Http>, channel_id_val: u64) -> Self {
        Self {
            http_client,
            channel_id: ChannelId::from(channel_id_val),
        }
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
        tracing::debug!("DiscordActor received SendDetection for message: {}", msg.detection.message);
        let detection = msg.detection;
        
        let image_bytes = match base64::engine::general_purpose::STANDARD.decode(&detection.image) {
            Ok(bytes) => bytes,
            Err(e) => {
                let err_msg = format!("Failed to decode image: {}", e);
                tracing::error!(err_msg);
                return Err(err_msg);
            }
        };

        let channel_id = self.channel_id;
        let http_client = self.http_client.clone();
        let mut message_content = detection.message.clone(); // Clone for the async block

        // Append the role ping
        match env::var("PING_ROLE_ID") {
            Ok(role_id) => {
                message_content.push_str(&format!("\n\n<@&{}>", role_id));
            }
            Err(e) => {
                tracing::warn!("Failed to get PING_ROLE_ID: {}. Skipping role ping.", e);
            }
        }

        let fut = async move {
            let attachment = CreateAttachment::bytes(image_bytes, "image.jpg");
            let message_payload = json!({
                "content": message_content,
                "flags": MessageFlags::SUPPRESS_EMBEDS.bits()
            });

            match http_client.send_message(channel_id, vec![attachment], &message_payload).await {
                Ok(_) => tracing::info!("Message sent to Discord by DiscordActor."),
                Err(e) => tracing::error!("DiscordActor failed to send message: {:?}", e),
            }
        };

        ctx.spawn(fut.into_actor(self)); // Fire and forget

        Ok(())
    }
} 