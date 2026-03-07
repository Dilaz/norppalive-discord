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
mod kafka_producer;
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

struct Handler {
    kafka_producer: Arc<kafka_producer::BotKafkaProducer>,
}

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

    async fn guild_create(&self, _ctx: Context, guild: serenity::all::Guild, is_new: Option<bool>) {
        // is_new == Some(true) means the bot was just added (not a cache fill on startup)
        if is_new == Some(true) {
            info!("Bot added to guild: {} ({})", guild.name, guild.id);
            let payload = kafka_producer::GuildEventPayload {
                event: "guild_join".into(),
                guild_id: guild.id.to_string(),
                timestamp: chrono::Utc::now().to_rfc3339(),
            };
            if let Err(e) = self.kafka_producer.publish_guild_event(&payload).await {
                error!("Failed to publish guild_join event: {e}");
            }
        }
    }

    async fn guild_delete(&self, _ctx: Context, incomplete: serenity::all::UnavailableGuild, _full: Option<serenity::all::Guild>) {
        if incomplete.unavailable {
            info!("Guild {} is unavailable (outage), not publishing leave event", incomplete.id);
            return;
        }
        info!("Bot removed from guild: {}", incomplete.id);
        let payload = kafka_producer::GuildEventPayload {
            event: "guild_leave".into(),
            guild_id: incomplete.id.to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
        };
        if let Err(e) = self.kafka_producer.publish_guild_event(&payload).await {
            error!("Failed to publish guild_leave event: {e}");
        }
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
    let kafka_guild_events_topic = var("KAFKA_GUILD_EVENTS_TOPIC")
        .unwrap_or_else(|_| "bot-guild-events".to_string());
    let kafka_error_events_topic = var("KAFKA_ERROR_EVENTS_TOPIC")
        .unwrap_or_else(|_| "bot-error-events".to_string());
    let kafka_reply_topic = var("KAFKA_BOT_REPLY_TOPIC")
        .unwrap_or_else(|_| "bot-reply".to_string());
    let backend_url = var("BACKEND_GRPC_URL")
        .unwrap_or_else(|_| "http://backend:50051".to_string());

    let bot_api_key = var("BOT_API_KEY")?;

    // Fetch all guild settings from the backend before starting actors
    let cache = settings::new_cache();
    grpc::load_settings(&backend_url, &cache, &bot_api_key)
        .await
        .map_err(|e| NorppaliveError::Config(format!("Failed to load guild settings from backend: {e}")))?;

    let bot_kafka_producer = Arc::new(kafka_producer::BotKafkaProducer::new(
        &kafka_broker,
        kafka_guild_events_topic,
        kafka_error_events_topic,
        kafka_reply_topic,
    ).map_err(|e| NorppaliveError::Config(format!("Failed to create bot Kafka producer: {e}")))?);

    let serenity_http_client = Arc::new(Http::new(&discord_token));

    info!("Starting DiscordActor...");
    let discord_actor_addr = actors::discord::DiscordActor::new(
        serenity_http_client.clone(),
        cache.clone(),
        bot_kafka_producer.clone(),
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
        kafka_broker.clone(),
        kafka_settings_topic,
        cache.clone(),
    )
    .start();

    let kafka_bot_request_topic = var("KAFKA_BOT_REQUEST_TOPIC")
        .unwrap_or_else(|_| "bot-request".to_string());

    info!("Starting BotRequestActor...");
    let _bot_request_addr = actors::bot_request::BotRequestActor::new(
        kafka_broker,
        kafka_bot_request_topic,
        serenity_http_client.clone(),
        cache.clone(),
        bot_kafka_producer.clone(),
    )
    .start();

    info!("Connecting to Discord gateway...");
    let intents = GatewayIntents::GUILD_MESSAGES | GatewayIntents::GUILDS;
    let handler = Handler {
        kafka_producer: bot_kafka_producer.clone(),
    };
    let mut serenity_client = Client::builder(&discord_token, intents)
        .event_handler(handler)
        .await
        .map_err(NorppaliveError::Discord)?;

    if let Err(why) = serenity_client.start().await {
        error!("Serenity client error: {why:?}");
        return Err(NorppaliveError::Discord(why));
    }

    info!("Norppalive Discord bot shutting down.");
    Ok(())
}
