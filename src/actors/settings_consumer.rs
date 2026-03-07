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
        tracing::info!("SettingsConsumerActor started.");
        ctx.notify(StartConsuming);
    }

    fn stopped(&mut self, _ctx: &mut Self::Context) {
        tracing::info!("SettingsConsumerActor stopped.");
    }
}

impl Handler<StartConsuming> for SettingsConsumerActor {
    type Result = ();

    fn handle(&mut self, _msg: StartConsuming, ctx: &mut Self::Context) -> Self::Result {
        let broker = self.broker_addr.clone();
        let topic = self.topic.clone();
        let cache = self.cache.clone();
        let addr = ctx.address();

        actix::spawn(async move {
            let consumer: StreamConsumer = match ClientConfig::new()
                .set("bootstrap.servers", &broker)
                .set("group.id", "norppalive-discord-settings-group")
                .set("auto.offset.reset", "latest") // only apply new changes
                .create()
            {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!("Failed to create settings Kafka consumer: {e}. Retrying in 5s...");
                    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                    addr.do_send(StartConsuming);
                    return;
                }
            };

            if let Err(e) = consumer.subscribe(&[&topic]) {
                tracing::error!("Failed to subscribe to settings topic {topic}: {e}. Retrying in 5s...");
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                addr.do_send(StartConsuming);
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
                                    match config_from_event(ev) {
                                        Some(cfg) => {
                                            let mut map = cache.write().await;
                                            tracing::info!(
                                                "Settings updated for guild {}: channel={}, enabled={}",
                                                cfg.guild_id, cfg.channel_id, cfg.bot_enabled
                                            );
                                            map.insert(guild_id, cfg);
                                        }
                                        None => {
                                            let mut map = cache.write().await;
                                            tracing::warn!(
                                                "Settings update for guild {} has no valid channel_id, removing from cache",
                                                guild_id
                                            );
                                            map.remove(&guild_id);
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
