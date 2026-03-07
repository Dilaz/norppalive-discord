use tracing::{info, warn};

use crate::proto::norppalive::v1::{
    bot_service_client::BotServiceClient, GetAllGuildSettingsRequest,
};
use crate::settings::{config_from_event, SettingsCache, SettingsUpdateEvent};

/// Connect to the backend and populate the settings cache with all guild configs.
/// Guilds with no valid channel_id are skipped with a warning.
pub async fn load_settings(
    backend_url: &str,
    cache: &SettingsCache,
    api_key: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    info!(
        "Connecting to backend at {} to fetch guild settings...",
        backend_url
    );
    let mut client = BotServiceClient::connect(backend_url.to_string()).await?;

    let mut request = tonic::Request::new(GetAllGuildSettingsRequest {});
    request.metadata_mut().insert(
        "x-bot-api-key",
        tonic::metadata::MetadataValue::try_from(api_key)?,
    );

    let resp = client.get_all_guild_settings(request).await?;

    let mut map = cache.write().await;
    let mut loaded_count: usize = 0;
    for s in resp.into_inner().settings {
        // Reuse config_from_event logic by converting proto → SettingsUpdateEvent.
        let ev = SettingsUpdateEvent {
            guild_id: s.guild_id,
            channel_id: s.channel_id,
            role_id: if s.role_id.is_empty() {
                None
            } else {
                Some(s.role_id)
            },
            enabled: s.bot_enabled,
        };
        let ev_guild_id = ev.guild_id.clone();
        match config_from_event(ev) {
            Some(cfg) => {
                info!(
                    "Loaded settings for guild {}: channel={}",
                    cfg.guild_id, cfg.channel_id
                );
                map.insert(ev_guild_id, cfg);
                loaded_count += 1;
            }
            None => {
                warn!("Guild {} has no valid channel_id, skipping", ev_guild_id);
            }
        }
    }
    info!("Loaded settings for {} guilds.", loaded_count);
    Ok(())
}
