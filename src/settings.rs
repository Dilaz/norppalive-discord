use std::collections::HashMap;
use std::sync::Arc;
use serde::Deserialize;
use tokio::sync::RwLock;

/// Per-guild configuration loaded from the backend.
#[derive(Debug, Clone)]
pub struct GuildConfig {
    pub guild_id: String,
    pub channel_id: u64,
    pub role_id: Option<u64>,
    pub min_confidence: f32,
    pub active_hours_start: String,
    pub active_hours_end: String,
    pub bot_enabled: bool,
}

/// Shared, async-safe settings map.
pub type SettingsCache = Arc<RwLock<HashMap<String, GuildConfig>>>;

pub fn new_cache() -> SettingsCache {
    Arc::new(RwLock::new(HashMap::new()))
}

/// Kafka message format published by the backend on settings changes.
#[derive(Debug, Deserialize, Clone)]
pub struct SettingsUpdateEvent {
    pub guild_id: String,
    pub channel_id: String,
    pub role_id: String,
    pub min_confidence: f32,
    pub active_hours_start: String,
    pub active_hours_end: String,
    pub enabled: bool,
}

/// Parse a channel or role snowflake string. Returns None if empty or invalid.
pub fn parse_snowflake(s: &str) -> Option<u64> {
    if s.is_empty() { None } else { s.parse::<u64>().ok() }
}

/// Build a GuildConfig from a SettingsUpdateEvent.
pub fn config_from_event(ev: &SettingsUpdateEvent) -> Option<GuildConfig> {
    let channel_id = parse_snowflake(&ev.channel_id)?;
    Some(GuildConfig {
        guild_id: ev.guild_id.clone(),
        channel_id,
        role_id: parse_snowflake(&ev.role_id),
        min_confidence: ev.min_confidence,
        active_hours_start: ev.active_hours_start.clone(),
        active_hours_end: ev.active_hours_end.clone(),
        bot_enabled: ev.enabled,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_snowflake_valid() {
        assert_eq!(parse_snowflake("123456789012345678"), Some(123456789012345678u64));
    }

    #[test]
    fn parse_snowflake_empty_returns_none() {
        assert_eq!(parse_snowflake(""), None);
    }

    #[test]
    fn parse_snowflake_non_numeric_returns_none() {
        assert_eq!(parse_snowflake("not-a-number"), None);
    }

    #[test]
    fn settings_update_event_deserializes() {
        let json = r#"{
            "guild_id": "111",
            "channel_id": "222",
            "role_id": "333",
            "min_confidence": 0.75,
            "active_hours_start": "08:00",
            "active_hours_end": "22:00",
            "enabled": true
        }"#;
        let ev: SettingsUpdateEvent = serde_json::from_str(json).unwrap();
        assert_eq!(ev.guild_id, "111");
        assert!((ev.min_confidence - 0.75).abs() < 0.001);
        assert!(ev.enabled);
    }

    #[test]
    fn config_from_event_with_empty_role() {
        let ev = SettingsUpdateEvent {
            guild_id: "1".into(),
            channel_id: "999".into(),
            role_id: "".into(),
            min_confidence: 0.5,
            active_hours_start: "".into(),
            active_hours_end: "".into(),
            enabled: true,
        };
        let cfg = config_from_event(&ev).unwrap();
        assert_eq!(cfg.channel_id, 999u64);
        assert!(cfg.role_id.is_none());
    }

    #[test]
    fn config_from_event_invalid_channel_returns_none() {
        let ev = SettingsUpdateEvent {
            guild_id: "1".into(),
            channel_id: "".into(), // missing channel → skip guild
            role_id: "".into(),
            min_confidence: 0.5,
            active_hours_start: "".into(),
            active_hours_end: "".into(),
            enabled: true,
        };
        assert!(config_from_event(&ev).is_none());
    }
}
