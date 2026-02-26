use actix::prelude::*;
use rdkafka::consumer::{Consumer, StreamConsumer};
use rdkafka::config::ClientConfig;
use rdkafka::message::Message;

use crate::settings::{config_from_event, SettingsCache, SettingsUpdateEvent};

#[derive(Message)]
#[rtype(result = "()")]
pub struct StartConsuming;

pub struct SettingsConsumerActor {
    pub broker_addr: String,
    pub topic: String,
    pub cache: SettingsCache,
}

impl SettingsConsumerActor {
    pub fn new(broker_addr: String, topic: String, cache: SettingsCache) -> Self {
        Self { broker_addr, topic, cache }
    }
}

impl Actor for SettingsConsumerActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        ctx.notify(StartConsuming);
    }
}

impl Handler<StartConsuming> for SettingsConsumerActor {
    type Result = ();

    fn handle(&mut self, _msg: StartConsuming, _ctx: &mut Self::Context) -> Self::Result {
        let broker = self.broker_addr.clone();
        let topic = self.topic.clone();
        let cache = self.cache.clone();

        actix::spawn(async move {
            let consumer: StreamConsumer = match ClientConfig::new()
                .set("bootstrap.servers", &broker)
                .set("group.id", "norppalive-discord-settings-group")
                .set("auto.offset.reset", "latest") // only apply new changes
                .create()
            {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!("Failed to create settings Kafka consumer: {e}");
                    return;
                }
            };

            if let Err(e) = consumer.subscribe(&[&topic]) {
                tracing::error!("Failed to subscribe to settings topic {topic}: {e}");
                return;
            }

            tracing::info!("SettingsConsumerActor listening on topic: {topic}");

            loop {
                match consumer.recv().await {
                    Ok(m) => {
                        if let Some(Ok(payload)) = m.payload_view::<str>() {
                            match serde_json::from_str::<SettingsUpdateEvent>(payload) {
                                Ok(ev) => {
                                    let guild_id = ev.guild_id.clone();
                                    match config_from_event(&ev) {
                                        Some(cfg) => {
                                            let mut map = cache.write().await;
                                            tracing::info!(
                                                "Settings updated for guild {}: channel={}, enabled={}",
                                                guild_id, cfg.channel_id, cfg.bot_enabled
                                            );
                                            map.insert(guild_id, cfg);
                                        }
                                        None => {
                                            tracing::warn!(
                                                "Settings update for guild {} has no valid channel_id, removing from cache",
                                                guild_id
                                            );
                                            cache.write().await.remove(&guild_id);
                                        }
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!("Failed to parse settings update: {e} | payload: {payload}");
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("Settings Kafka error: {e:?}");
                    }
                }
            }
        });
    }
}
