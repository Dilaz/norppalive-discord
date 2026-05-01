use actix::Actor;
use miette::Result;
use serenity::all::{
    ActivityData, ChannelId, Client, Command, CommandOptionType, CommandType, CreateCommand,
    CreateCommandOption, CreateMessage, GatewayIntents, Http, Interaction, MessageFlags,
    OnlineStatus, Ready,
};
use serenity::async_trait;
use serenity::prelude::*;
use std::{env::var, sync::Arc, time::Duration};
use tokio::sync::Semaphore;
use tracing::{error, info, warn};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

mod actors;
mod commands;
mod error;
mod grpc;
mod kafka_producer;
mod models;
mod settings;

use commands::norpantunnistus::{CommandDeps, CONTEXT_COMMAND_NAME, SLASH_COMMAND_NAME};
use commands::rate_limit::RateLimiter;

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
    log_channel_id: Option<u64>,
    command_deps: Option<Arc<CommandDeps>>,
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

        let cmds = if self.command_deps.is_some() {
            let slash = CreateCommand::new(SLASH_COMMAND_NAME)
                .description("Tunnista norppia kuvasta")
                .add_option(
                    CreateCommandOption::new(
                        CommandOptionType::Attachment,
                        "image",
                        "Kuva tunnistettavaksi",
                    )
                    .required(true),
                );
            let context_menu = CreateCommand::new(CONTEXT_COMMAND_NAME).kind(CommandType::Message);
            vec![slash, context_menu]
        } else {
            // Empty list clears any previously-registered global commands
            // so /norpantunnistus disappears from Discord when disabled.
            Vec::new()
        };
        match Command::set_global_commands(&ctx.http, cmds).await {
            Ok(registered) => info!("Registered {} global commands", registered.len()),
            Err(e) => error!("Failed to register global commands: {e}"),
        }
    }

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        let Some(cmd) = interaction.command() else {
            return;
        };
        let Some(deps) = self.command_deps.clone() else {
            return;
        };
        match cmd.data.name.as_str() {
            SLASH_COMMAND_NAME | CONTEXT_COMMAND_NAME => {
                tokio::spawn(async move {
                    commands::norpantunnistus::handle(ctx, cmd, deps).await;
                });
            }
            other => {
                warn!("Received unknown command: {other}");
            }
        }
    }

    async fn guild_create(&self, ctx: Context, guild: serenity::all::Guild, is_new: Option<bool>) {
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

            if let Some(channel_id) = self.log_channel_id {
                let msg = CreateMessage::new().content(format!(
                    "\u{2705} Bot added to **{}** (ID: {}, members: {})",
                    guild.name, guild.id, guild.member_count
                ));
                if let Err(e) = ChannelId::new(channel_id)
                    .send_message(&ctx.http, msg)
                    .await
                {
                    error!("Failed to send log channel notification for guild_join: {e}");
                }
            }
        }
    }

    async fn guild_delete(
        &self,
        ctx: Context,
        incomplete: serenity::all::UnavailableGuild,
        _full: Option<serenity::all::Guild>,
    ) {
        if incomplete.unavailable {
            info!(
                "Guild {} is unavailable (outage), not publishing leave event",
                incomplete.id
            );
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

        if let Some(channel_id) = self.log_channel_id {
            let msg = CreateMessage::new()
                .content(format!("\u{274c} Bot removed from guild {}", incomplete.id));
            if let Err(e) = ChannelId::new(channel_id)
                .send_message(&ctx.http, msg)
                .await
            {
                error!("Failed to send log channel notification for guild_leave: {e}");
            }
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
    let kafka_settings_topic =
        var("KAFKA_SETTINGS_TOPIC").unwrap_or_else(|_| "guild-settings-update".to_string());
    let kafka_guild_events_topic =
        var("KAFKA_GUILD_EVENTS_TOPIC").unwrap_or_else(|_| "bot-guild-events".to_string());
    let kafka_error_events_topic =
        var("KAFKA_ERROR_EVENTS_TOPIC").unwrap_or_else(|_| "bot-error-events".to_string());
    let kafka_reply_topic =
        var("KAFKA_BOT_REPLY_TOPIC").unwrap_or_else(|_| "bot-reply".to_string());
    let backend_url =
        var("BACKEND_GRPC_URL").unwrap_or_else(|_| "http://backend:50051".to_string());

    let bot_api_key = var("BOT_API_KEY")?;

    let log_channel_id: Option<u64> = var("LOG_CHANNEL_ID")
        .ok()
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse().ok());

    // Fetch all guild settings from the backend before starting actors
    let cache = settings::new_cache();
    grpc::load_settings(&backend_url, &cache, &bot_api_key)
        .await
        .map_err(|e| {
            NorppaliveError::Config(format!("Failed to load guild settings from backend: {e}"))
        })?;

    let bot_kafka_producer = Arc::new(
        kafka_producer::BotKafkaProducer::new(
            &kafka_broker,
            kafka_guild_events_topic,
            kafka_error_events_topic,
            kafka_reply_topic,
        )
        .map_err(|e| {
            NorppaliveError::Config(format!("Failed to create bot Kafka producer: {e}"))
        })?,
    );

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

    let kafka_bot_request_topic =
        var("KAFKA_BOT_REQUEST_TOPIC").unwrap_or_else(|_| "bot-request".to_string());

    info!("Starting BotRequestActor...");
    let _bot_request_addr = actors::bot_request::BotRequestActor::new(
        kafka_broker,
        kafka_bot_request_topic,
        serenity_http_client.clone(),
        cache.clone(),
        bot_kafka_producer.clone(),
    )
    .start();

    let detect_enabled = var("DETECT_ENABLED")
        .ok()
        .map(|s| {
            !matches!(
                s.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "no" | "off"
            )
        })
        .unwrap_or(true);

    let command_deps = if detect_enabled {
        let detect_url = var("DETECT_API_URL")
            .unwrap_or_else(|_| "http://norppalive-api:8080/detect".to_string());
        let detect_cooldown_secs: u64 = var("DETECT_COOLDOWN_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(30);
        let detect_daily_max: u32 = var("DETECT_DAILY_MAX")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(20);
        Some(Arc::new(CommandDeps {
            rate_limiter: Arc::new(RateLimiter::new(
                Duration::from_secs(detect_cooldown_secs),
                detect_daily_max,
            )),
            semaphore: Arc::new(Semaphore::new(1)),
            http_client: reqwest::Client::builder()
                .timeout(Duration::from_secs(60))
                .build()
                .map_err(|e| {
                    NorppaliveError::Config(format!("reqwest client build failed: {e}"))
                })?,
            detect_url,
        }))
    } else {
        info!("/norpantunnistus disabled via DETECT_ENABLED");
        None
    };

    info!("Connecting to Discord gateway...");
    let intents = GatewayIntents::GUILD_MESSAGES | GatewayIntents::GUILDS;
    let handler = Handler {
        kafka_producer: bot_kafka_producer.clone(),
        log_channel_id,
        command_deps,
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
