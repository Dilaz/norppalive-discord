use rdkafka::config::ClientConfig;
use rdkafka::producer::{FutureProducer, FutureRecord};
use serde::Serialize;
use std::time::Duration;

#[derive(Clone)]
pub struct BotKafkaProducer {
    producer: FutureProducer,
    guild_events_topic: String,
    error_events_topic: String,
    reply_topic: String,
}

#[derive(Debug, Serialize)]
pub struct GuildEventPayload {
    pub event: String,
    pub guild_id: String,
    pub timestamp: String,
}

#[derive(Debug, Serialize)]
pub struct BotErrorPayload {
    pub guild_id: String,
    pub channel_id: String,
    pub error_type: String,
    pub error_message: String,
    pub timestamp: String,
}

impl BotKafkaProducer {
    pub fn new(
        broker: &str,
        guild_events_topic: String,
        error_events_topic: String,
        reply_topic: String,
    ) -> Result<Self, String> {
        let producer: FutureProducer = ClientConfig::new()
            .set("bootstrap.servers", broker)
            .set("message.timeout.ms", "5000")
            .create()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            producer,
            guild_events_topic,
            error_events_topic,
            reply_topic,
        })
    }

    pub async fn publish_guild_event(&self, payload: &GuildEventPayload) -> Result<(), String> {
        let json = serde_json::to_string(payload).map_err(|e| e.to_string())?;
        self.producer
            .send(
                FutureRecord::to(&self.guild_events_topic)
                    .key(&payload.guild_id)
                    .payload(&json),
                Duration::from_secs(5),
            )
            .await
            .map_err(|(e, _)| e.to_string())?;
        Ok(())
    }

    pub async fn publish_error_event(&self, payload: &BotErrorPayload) -> Result<(), String> {
        let json = serde_json::to_string(payload).map_err(|e| e.to_string())?;
        self.producer
            .send(
                FutureRecord::to(&self.error_events_topic)
                    .key(&payload.guild_id)
                    .payload(&json),
                Duration::from_secs(5),
            )
            .await
            .map_err(|(e, _)| e.to_string())?;
        Ok(())
    }

    pub async fn publish_reply(&self, key: &str, payload: &str) -> Result<(), String> {
        self.producer
            .send(
                FutureRecord::to(&self.reply_topic)
                    .key(key)
                    .payload(payload),
                Duration::from_secs(5),
            )
            .await
            .map_err(|(e, _)| e.to_string())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guild_event_payload_serializes() {
        let payload = GuildEventPayload {
            event: "guild_leave".into(),
            guild_id: "123".into(),
            timestamp: "2026-03-07T12:00:00Z".into(),
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("\"event\":\"guild_leave\""));
        assert!(json.contains("\"guild_id\":\"123\""));
    }

    #[test]
    fn bot_error_payload_serializes() {
        let payload = BotErrorPayload {
            guild_id: "123".into(),
            channel_id: "456".into(),
            error_type: "missing_permissions".into(),
            error_message: "Missing Access".into(),
            timestamp: "2026-03-07T12:00:00Z".into(),
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("\"error_type\":\"missing_permissions\""));
    }
}
