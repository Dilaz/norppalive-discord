use std::{env::var, sync::Arc};
use serde::{Deserialize, Serialize};
use serenity::{all::{ActivityData, OnlineStatus, Ready, MessageFlags, CreateMessage, GatewayIntents, Http, Client}, async_trait, prelude::*};
use miette::Result;
use tracing::{error, info};
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::layer::SubscriberExt;
use actix::Actor;

mod error;
use error::NorppaliveError;
mod settings;
mod models;
mod actors;

const DEFAULT_ACTIVITY: &str = "Norbs";
const DISCORD_CHANNEL_ID_ENV_VAR: &str = "DISCORD_CHANNEL_ID";

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
        info!("Serenity Handler: {} is connected!", ready.user.name);
        ctx.set_presence(None, OnlineStatus::Online);
		info!("Serenity Handler: Setting activity to {}", DEFAULT_ACTIVITY);
        ctx.set_activity(Some(ActivityData::watching(DEFAULT_ACTIVITY)));
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct DetectionMessage {
    image: String,
    message: String,
}

#[actix::main]
async fn main() -> Result<(), NorppaliveError> {
    dotenvy::dotenv().ok();
    tracing_subscriber::registry()
    .with(
        tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "norppalive_discord=info,kafka=info".into()),
    )
    .with(tracing_subscriber::fmt::layer())
    .init();

    info!("Initializing Norppalive Discord bot with Actix actors...");

    let discord_token = var("DISCORD_TOKEN")?;
    let kafka_topic = var("KAFKA_TOPIC")?;
    let kafka_broker = var("KAFKA_BROKER")?;
    let discord_channel_id_str = var(DISCORD_CHANNEL_ID_ENV_VAR)?;
    let discord_channel_id = discord_channel_id_str
        .parse::<u64>()
        .map_err(|e| NorppaliveError::Config(format!("Invalid {}: {} - {}", DISCORD_CHANNEL_ID_ENV_VAR, discord_channel_id_str, e)))?;

    let serenity_http_client = Arc::new(Http::new(&discord_token));

    info!("Starting DiscordActor...");
    let discord_actor_addr = actors::discord::DiscordActor::new(serenity_http_client, discord_channel_id).start();

    info!("Starting KafkaConsumerActor...");
    let _kafka_actor_addr = actors::kafka::KafkaRdkafkaActor::new(kafka_broker, kafka_topic, discord_actor_addr).start();

    info!("Initializing main Serenity client for Discord gateway events...");
    let intents = GatewayIntents::GUILD_MESSAGES;
    let mut serenity_client = Client::builder(&discord_token, intents)
        .event_handler(Handler)
        .await
        .map_err(NorppaliveError::Discord)?;

    info!("Starting main Serenity client...");
    if let Err(why) = serenity_client.start().await {
        error!("Main Serenity client error: {:?}", why);
        return Err(NorppaliveError::Discord(why));
    }

    info!("Norppalive Discord bot shutting down normally.");
    Ok(())
}
