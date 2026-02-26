use tracing::{info, warn};

use crate::proto::norppalive::v1::{
    bot_service_client::BotServiceClient, GetAllGuildSettingsRequest,
};
use crate::settings::{config_from_event, SettingsCache, SettingsUpdateEvent};

/// Connect to the backend and populate the settings cache with all guild configs.
/// Guilds with no valid channel_id are skipped with a warning.
pub async fn load_settings(backend_url: &str, cache: &SettingsCache) -> Result<(), Box<dyn std::error::Error>> {
    info!("Connecting to backend at {} to fetch guild settings...", backend_url);
    let mut client = BotServiceClient::connect(backend_url.to_string()).await?;
    let resp = client
        .get_all_guild_settings(GetAllGuildSettingsRequest {})
        .await?;

    let mut map = cache.write().await;
    for s in resp.into_inner().settings {
        // Reuse config_from_event logic by converting proto → SettingsUpdateEvent.
        // proto GuildSettings.bot_enabled maps to SettingsUpdateEvent.enabled.
        // proto GuildSettings.min_confidence is f32; cast to f64.
        let ev = SettingsUpdateEvent {
            guild_id: s.guild_id,
            channel_id: s.channel_id,
            role_id: if s.role_id.is_empty() { None } else { Some(s.role_id) },
            min_confidence: s.min_confidence as f64,
            active_hours_start: s.active_hours_start,
            active_hours_end: s.active_hours_end,
            enabled: s.bot_enabled,
        };
        match config_from_event(&ev) {
            Some(cfg) => {
                info!("Loaded settings for guild {}: channel={}", cfg.guild_id, cfg.channel_id);
                map.insert(cfg.guild_id.clone(), cfg);
            }
            None => {
                warn!("Guild {} has no valid channel_id, skipping", ev.guild_id);
            }
        }
    }
    info!("Loaded settings for {} guilds.", map.len());
    Ok(())
}
