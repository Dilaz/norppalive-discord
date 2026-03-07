use actix::prelude::*;
use rdkafka::consumer::{Consumer, StreamConsumer};
use rdkafka::config::ClientConfig;
use rdkafka::message::Message;
use serde::{Deserialize, Serialize};
use serenity::all::{ChannelId, CreateAttachment, Http, MessageFlags};
use std::sync::Arc;
use serde_json::json;

use crate::kafka_producer::BotKafkaProducer;
use crate::settings::SettingsCache;

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum BotRequest {
    #[serde(rename = "is_in_guild")]
    IsInGuild {
        guild_id: String,
        correlation_id: String,
    },
    #[serde(rename = "send_test_message")]
    SendTestMessage {
        guild_id: String,
        channel_id: String,
        role_id: String,
        ping_role: bool,
        correlation_id: String,
    },
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum BotReply {
    #[serde(rename = "is_in_guild")]
    IsInGuild {
        correlation_id: String,
        present: bool,
    },
    #[serde(rename = "send_test_message")]
    SendTestMessage {
        correlation_id: String,
        success: bool,
        error_message: String,
    },
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct StartListening;

pub struct BotRequestActor {
    broker_addr: String,
    topic: String,
    http_client: Arc<Http>,
    settings: SettingsCache,
    kafka_producer: Arc<BotKafkaProducer>,
}

impl BotRequestActor {
    pub fn new(
        broker_addr: String,
        topic: String,
        http_client: Arc<Http>,
        settings: SettingsCache,
        kafka_producer: Arc<BotKafkaProducer>,
    ) -> Self {
        Self {
            broker_addr,
            topic,
            http_client,
            settings,
            kafka_producer,
        }
    }
}

impl Actor for BotRequestActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        tracing::info!("BotRequestActor started.");
        ctx.notify(StartListening);
    }

    fn stopped(&mut self, _ctx: &mut Self::Context) {
        tracing::info!("BotRequestActor stopped.");
    }
}

impl Handler<StartListening> for BotRequestActor {
    type Result = ();

    fn handle(&mut self, _msg: StartListening, ctx: &mut Self::Context) -> Self::Result {
        let broker = self.broker_addr.clone();
        let topic = self.topic.clone();
        let http_client = self.http_client.clone();
        let settings = self.settings.clone();
        let kafka_producer = self.kafka_producer.clone();
        let addr = ctx.address();

        actix::spawn(async move {
            let consumer: StreamConsumer = match ClientConfig::new()
                .set("bootstrap.servers", &broker)
                .set("group.id", "norppalive-discord-bot-request-group")
                .set("auto.offset.reset", "latest")
                .create()
            {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!("Failed to create bot-request consumer: {e}. Retrying in 5s...");
                    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                    addr.do_send(StartListening);
                    return;
                }
            };

            if let Err(e) = consumer.subscribe(&[&topic]) {
                tracing::error!("Failed to subscribe to {topic}: {e}. Retrying in 5s...");
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                addr.do_send(StartListening);
                return;
            }

            tracing::info!("BotRequestActor listening on topic: {topic}");

            loop {
                match consumer.recv().await {
                    Ok(m) => {
                        if let Some(Ok(payload)) = m.payload_view::<str>() {
                            match serde_json::from_str::<BotRequest>(payload) {
                                Ok(BotRequest::IsInGuild { guild_id, correlation_id }) => {
                                    handle_is_in_guild(
                                        &http_client,
                                        &kafka_producer,
                                        &guild_id,
                                        &correlation_id,
                                    ).await;
                                }
                                Ok(BotRequest::SendTestMessage {
                                    guild_id,
                                    channel_id,
                                    role_id,
                                    ping_role,
                                    correlation_id,
                                }) => {
                                    handle_send_test_message(
                                        &http_client,
                                        &settings,
                                        &kafka_producer,
                                        &guild_id,
                                        &channel_id,
                                        &role_id,
                                        ping_role,
                                        &correlation_id,
                                    ).await;
                                }
                                Err(e) => {
                                    tracing::warn!("Failed to parse bot request: {e} | {payload}");
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("Bot request Kafka error: {e:?}");
                    }
                }
            }
        });
    }
}

async fn handle_is_in_guild(
    http_client: &Http,
    kafka_producer: &BotKafkaProducer,
    guild_id: &str,
    correlation_id: &str,
) {
    let present = match guild_id.parse::<u64>() {
        Ok(id) => {
            // Try to get guild info via bot — if it succeeds, bot is in the guild
            match http_client.get_guild(serenity::all::GuildId::from(id)).await {
                Ok(_) => true,
                Err(_) => false,
            }
        }
        Err(_) => false,
    };

    let reply = BotReply::IsInGuild {
        correlation_id: correlation_id.to_string(),
        present,
    };
    let json = match serde_json::to_string(&reply) {
        Ok(json) => json,
        Err(e) => {
            tracing::error!("Failed to serialize is_in_guild reply: {e}");
            return;
        }
    };
    if let Err(e) = kafka_producer.publish_reply(guild_id, &json).await {
        tracing::error!("Failed to publish is_in_guild reply: {e}");
    }
}

async fn handle_send_test_message(
    http_client: &Http,
    _settings: &SettingsCache,
    kafka_producer: &BotKafkaProducer,
    guild_id: &str,
    channel_id: &str,
    role_id: &str,
    ping_role: bool,
    correlation_id: &str,
) {
    let channel = match channel_id.parse::<u64>() {
        Ok(id) => ChannelId::from(id),
        Err(_) => {
            let reply = BotReply::SendTestMessage {
                correlation_id: correlation_id.to_string(),
                success: false,
                error_message: "Invalid channel ID".to_string(),
            };
            let json = match serde_json::to_string(&reply) {
                Ok(json) => json,
                Err(e) => {
                    tracing::error!("Failed to serialize error reply for invalid channel ID: {e}");
                    return;
                }
            };
            if let Err(e) = kafka_producer.publish_reply(guild_id, &json).await {
                tracing::error!("Failed to publish error reply for invalid channel ID: {e}");
            }
            return;
        }
    };

    let mut content = "Tämä on testiviesti Norppalive-botilta!".to_string();
    if ping_role && !role_id.is_empty() && role_id != "0" {
        if let Ok(id) = role_id.parse::<u64>() {
            content.push_str(&format!("\n\n<@&{id}>"));
        }
    }

    let test_image = include_bytes!("../../assets/test_norppa.jpg");

    let attachment = CreateAttachment::bytes(test_image.as_slice(), "test_norppa.jpg");
    let payload = json!({
        "content": content,
        "flags": MessageFlags::SUPPRESS_EMBEDS.bits()
    });

    let (success, error_message) = match http_client
        .send_message(channel, vec![attachment], &payload)
        .await
    {
        Ok(_) => {
            tracing::info!("Sent test message to guild {guild_id} channel {channel_id}");
            (true, String::new())
        }
        Err(e) => {
            let err = format!("{e:?}");
            tracing::error!("Failed to send test message to guild {guild_id}: {err}");
            (false, err)
        }
    };

    let reply = BotReply::SendTestMessage {
        correlation_id: correlation_id.to_string(),
        success,
        error_message,
    };
    let json = match serde_json::to_string(&reply) {
        Ok(json) => json,
        Err(e) => {
            tracing::error!("Failed to serialize test message reply: {e}");
            return;
        }
    };
    if let Err(e) = kafka_producer.publish_reply(guild_id, &json).await {
        tracing::error!("Failed to publish test message reply: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bot_request_is_in_guild_deserializes() {
        let json = r#"{"type":"is_in_guild","guild_id":"123","correlation_id":"abc"}"#;
        let req: BotRequest = serde_json::from_str(json).unwrap();
        if let BotRequest::IsInGuild { guild_id, correlation_id } = req {
            assert_eq!(guild_id, "123");
            assert_eq!(correlation_id, "abc");
        } else {
            panic!("Wrong variant");
        }
    }

    #[test]
    fn bot_request_send_test_message_deserializes() {
        let json = r#"{"type":"send_test_message","guild_id":"123","channel_id":"456","role_id":"789","ping_role":true,"correlation_id":"xyz"}"#;
        let req: BotRequest = serde_json::from_str(json).unwrap();
        if let BotRequest::SendTestMessage { ping_role, .. } = req {
            assert!(ping_role);
        } else {
            panic!("Wrong variant");
        }
    }

    #[test]
    fn bot_reply_is_in_guild_serializes() {
        let reply = BotReply::IsInGuild {
            correlation_id: "abc".into(),
            present: true,
        };
        let json = serde_json::to_string(&reply).unwrap();
        assert!(json.contains("\"type\":\"is_in_guild\""));
        assert!(json.contains("\"present\":true"));
    }

    #[test]
    fn bot_reply_send_test_message_serializes() {
        let reply = BotReply::SendTestMessage {
            correlation_id: "xyz".into(),
            success: false,
            error_message: "Missing Access".into(),
        };
        let json = serde_json::to_string(&reply).unwrap();
        assert!(json.contains("\"success\":false"));
        assert!(json.contains("Missing Access"));
    }
}
