use std::{env::var, sync::Arc};
use actix::Actor;
use miette::Result;
use serenity::all::{
    ActivityData, Client, CreateMessage, GatewayIntents, Http, MessageFlags, OnlineStatus, Ready,
};
use serenity::async_trait;
use serenity::prelude::*;
use tracing::{error, info};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

mod error;
mod grpc;
mod models;
mod settings;
mod actors;

use error::NorppaliveError;

pub mod proto {
    pub mod norppalive {
        pub mod v1 {
            tonic::include_proto!("norppalive.v1");
        }
    }
}

const DEFAULT_ACTIVITY: &str = "Norbs";

struct Handler;

#[async_trait]
impl EventHandler for Handler {
    async fn message(&self, ctx: Context, msg: serenity::all::Message) {
        if msg.content == "!ping" {
            let message = CreateMessage::new()
                .content("Pong!")
                .flags(MessageFlags::SUPPRESS_EMBEDS);
            if let Err(why) = msg.channel_id.send_message(&ctx.http, message).await {
                error!("Error sending ping response: {why:?}");
            }
        }
    }

    async fn ready(&self, ctx: Context, ready: Ready) {
        info!("{} is connected!", ready.user.name);
        ctx.set_presence(None, OnlineStatus::Online);
        ctx.set_activity(Some(ActivityData::watching(DEFAULT_ACTIVITY)));
    }
}

#[actix::main]
async fn main() -> Result<(), NorppaliveError> {
    dotenvy::dotenv().ok();
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "norppalive_discord=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Starting Norppalive Discord bot...");

    let discord_token = var("DISCORD_TOKEN")?;
    let kafka_broker = var("KAFKA_BROKER")?;
    let kafka_detection_topic = var("KAFKA_TOPIC")?;
    let kafka_settings_topic = var("KAFKA_SETTINGS_TOPIC")
        .unwrap_or_else(|_| "guild-settings-update".to_string());
    let backend_url = var("BACKEND_GRPC_URL")
        .unwrap_or_else(|_| "http://backend:50051".to_string());

    // Fetch all guild settings from the backend before starting actors
    let cache = settings::new_cache();
    grpc::load_settings(&backend_url, &cache)
        .await
        .map_err(|e| NorppaliveError::Config(format!("Failed to load guild settings from backend: {e}")))?;

    let serenity_http_client = Arc::new(Http::new(&discord_token));

    info!("Starting DiscordActor...");
    let discord_actor_addr = actors::discord::DiscordActor::new(
        serenity_http_client.clone(),
        cache.clone(),
    )
    .start();

    info!("Starting KafkaConsumerActor (detections)...");
    let _kafka_detection_addr = actors::kafka::KafkaRdkafkaActor::new(
        kafka_broker.clone(),
        kafka_detection_topic,
        discord_actor_addr,
    )
    .start();

    info!("Starting SettingsConsumerActor...");
    let _settings_consumer_addr = actors::settings_consumer::SettingsConsumerActor::new(
        kafka_broker,
        kafka_settings_topic,
        cache,
    )
    .start();

    info!("Connecting to Discord gateway...");
    let intents = GatewayIntents::GUILD_MESSAGES;
    let mut serenity_client = Client::builder(&discord_token, intents)
        .event_handler(Handler)
        .await
        .map_err(NorppaliveError::Discord)?;

    if let Err(why) = serenity_client.start().await {
        error!("Serenity client error: {why:?}");
        return Err(NorppaliveError::Discord(why));
    }

    info!("Norppalive Discord bot shutting down.");
    Ok(())
}
