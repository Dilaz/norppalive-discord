use crate::actors::discord::{DiscordActor, SendDetection};
use crate::models::DetectionMessage;
use actix::prelude::*;
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{Consumer, StreamConsumer};
use rdkafka::message::Message;
use std::time::Duration;

#[derive(Message)]
#[rtype(result = "()")]
pub struct PollKafka;

pub struct KafkaRdkafkaActor {
    pub broker_addr: String,
    pub topic: String,
    pub discord_actor_addr: Addr<DiscordActor>,
}

impl KafkaRdkafkaActor {
    pub fn new(broker_addr: String, topic: String, discord_actor_addr: Addr<DiscordActor>) -> Self {
        Self {
            broker_addr,
            topic,
            discord_actor_addr,
        }
    }

    fn process_message(discord_addr: Addr<DiscordActor>, payload: &str) {
        tracing::info!("Received message: {}", payload);
        match serde_json::from_str::<DetectionMessage>(payload) {
            Ok(detection) => {
                discord_addr.do_send(SendDetection { detection });
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to parse Kafka message as DetectionMessage: {} | error: {}",
                    payload,
                    e
                );
            }
        }
    }
}

impl Actor for KafkaRdkafkaActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        ctx.notify(PollKafka);
    }
}

impl Handler<PollKafka> for KafkaRdkafkaActor {
    type Result = ();

    fn handle(&mut self, _msg: PollKafka, _ctx: &mut Self::Context) -> Self::Result {
        let broker = self.broker_addr.clone();
        let topic = self.topic.clone();
        let discord_addr = self.discord_actor_addr.clone();

        actix::spawn(async move {
            let consumer: StreamConsumer = match ClientConfig::new()
                .set("bootstrap.servers", &broker)
                .set("group.id", "norppalive-discord-group")
                .set("auto.offset.reset", "earliest")
                .create()
            {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!("Failed to create Kafka consumer: {}", e);
                    return;
                }
            };

            if let Err(e) = consumer.subscribe(&[&topic]) {
                tracing::error!("Can't subscribe to specified topic: {}", e);
                return;
            }

            loop {
                match consumer.recv().await {
                    Ok(m) => match m.payload_view::<str>() {
                        Some(Ok(payload)) => {
                            KafkaRdkafkaActor::process_message(discord_addr.clone(), payload);
                        }
                        Some(Err(e)) => {
                            tracing::warn!("Invalid UTF-8 in Kafka message: {:?}", e);
                        }
                        None => {
                            tracing::debug!("Received Kafka message with empty payload");
                        }
                    },
                    Err(e) => {
                        tracing::error!("Kafka error: {:?}", e);
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            }
        });
        // No need to schedule another poll, the spawned task runs forever
    }
}
