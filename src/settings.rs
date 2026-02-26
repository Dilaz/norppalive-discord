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
    pub min_confidence: f64,
    /// HH:MM in 24-hour UTC, e.g. "08:00". Empty string means no restriction.
    pub active_hours_start: String,
    /// HH:MM in 24-hour UTC, e.g. "22:00". Empty string means no restriction.
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
    #[serde(default)]
    pub role_id: Option<String>,
    pub min_confidence: f64,
    /// HH:MM in 24-hour UTC, e.g. "08:00". Empty string means no restriction.
    pub active_hours_start: String,
    /// HH:MM in 24-hour UTC, e.g. "22:00". Empty string means no restriction.
    pub active_hours_end: String,
    pub enabled: bool,
}

/// Parse a channel or role snowflake string. Returns None if empty, zero, or invalid.
pub fn parse_snowflake(s: &str) -> Option<u64> {
    match s.parse::<u64>().ok() {
        Some(0) | None => None,
        id => id,
    }
}

/// Build a GuildConfig from a SettingsUpdateEvent.
pub fn config_from_event(ev: &SettingsUpdateEvent) -> Option<GuildConfig> {
    let channel_id = parse_snowflake(&ev.channel_id)?;
    Some(GuildConfig {
        guild_id: ev.guild_id.clone(),
        channel_id,
        role_id: ev.role_id.as_deref().and_then(parse_snowflake),
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
    fn parse_snowflake_zero_returns_none() {
        assert_eq!(parse_snowflake("0"), None);
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
        assert_eq!(ev.min_confidence, 0.75);
        assert!(ev.enabled);
    }

    #[test]
    fn config_from_event_with_empty_role() {
        let ev = SettingsUpdateEvent {
            guild_id: "1".into(),
            channel_id: "999".into(),
            role_id: None,
            min_confidence: 0.5f64,
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
            role_id: None,
            min_confidence: 0.5,
            active_hours_start: "".into(),
            active_hours_end: "".into(),
            enabled: true,
        };
        assert!(config_from_event(&ev).is_none());
    }

    #[test]
    fn config_from_event_full_roundtrip() {
        let ev = SettingsUpdateEvent {
            guild_id: "42".into(),
            channel_id: "100".into(),
            role_id: Some("200".into()),
            min_confidence: 0.85,
            active_hours_start: "08:00".into(),
            active_hours_end: "22:00".into(),
            enabled: true,
        };
        let cfg = config_from_event(&ev).unwrap();
        assert_eq!(cfg.guild_id, "42");
        assert_eq!(cfg.channel_id, 100u64);
        assert_eq!(cfg.role_id, Some(200u64));
        assert_eq!(cfg.min_confidence, 0.85);
        assert_eq!(cfg.active_hours_start, "08:00");
        assert!(cfg.bot_enabled);
    }
}
