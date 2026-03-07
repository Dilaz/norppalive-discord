use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Per-guild configuration loaded from the backend.
#[derive(Debug, Clone)]
pub struct GuildConfig {
    pub guild_id: String,
    pub channel_id: u64,
    pub role_id: Option<u64>,
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
    pub enabled: bool,
}

/// Parse a channel or role snowflake string. Returns None if empty, zero, or invalid.
pub fn parse_snowflake(s: &str) -> Option<u64> {
    match s.parse::<u64>().ok() {
        Some(0) | None => None,
        id => id,
    }
}

/// Build a GuildConfig from a SettingsUpdateEvent, consuming it to avoid cloning strings.
pub fn config_from_event(ev: SettingsUpdateEvent) -> Option<GuildConfig> {
    let channel_id = parse_snowflake(&ev.channel_id)?;
    Some(GuildConfig {
        guild_id: ev.guild_id,
        channel_id,
        role_id: ev.role_id.as_deref().and_then(parse_snowflake),
        bot_enabled: ev.enabled,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_snowflake_valid() {
        assert_eq!(
            parse_snowflake("123456789012345678"),
            Some(123456789012345678u64)
        );
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
            "enabled": true
        }"#;
        let ev: SettingsUpdateEvent = serde_json::from_str(json).unwrap();
        assert_eq!(ev.guild_id, "111");
        assert!(ev.enabled);
    }

    #[test]
    fn config_from_event_with_empty_role() {
        let ev = SettingsUpdateEvent {
            guild_id: "1".into(),
            channel_id: "999".into(),
            role_id: None,
            enabled: true,
        };
        let cfg = config_from_event(ev).unwrap();
        assert_eq!(cfg.channel_id, 999u64);
        assert!(cfg.role_id.is_none());
    }

    #[test]
    fn config_from_event_invalid_channel_returns_none() {
        let ev = SettingsUpdateEvent {
            guild_id: "1".into(),
            channel_id: "".into(), // missing channel → skip guild
            role_id: None,
            enabled: true,
        };
        assert!(config_from_event(ev).is_none());
    }

    #[test]
    fn new_cache_is_empty() {
        let cache = new_cache();
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let map = cache.read().await;
            assert!(map.is_empty());
        });
    }

    #[test]
    fn parse_snowflake_max_u64() {
        assert_eq!(parse_snowflake("18446744073709551615"), Some(u64::MAX));
    }

    #[test]
    fn parse_snowflake_overflow_returns_none() {
        // u64::MAX + 1
        assert_eq!(parse_snowflake("18446744073709551616"), None);
    }

    #[test]
    fn parse_snowflake_leading_zeros() {
        assert_eq!(parse_snowflake("00123"), Some(123));
    }

    #[test]
    fn parse_snowflake_negative_string() {
        assert_eq!(parse_snowflake("-1"), None);
    }

    #[test]
    fn config_from_event_zero_channel_returns_none() {
        let ev = SettingsUpdateEvent {
            guild_id: "1".into(),
            channel_id: "0".into(),
            role_id: None,
            enabled: true,
        };
        assert!(config_from_event(ev).is_none());
    }

    #[test]
    fn config_from_event_zero_role_id_becomes_none() {
        let ev = SettingsUpdateEvent {
            guild_id: "1".into(),
            channel_id: "999".into(),
            role_id: Some("0".into()),
            enabled: true,
        };
        let cfg = config_from_event(ev).unwrap();
        assert!(cfg.role_id.is_none());
    }

    #[test]
    fn config_from_event_disabled_bot() {
        let ev = SettingsUpdateEvent {
            guild_id: "1".into(),
            channel_id: "100".into(),
            role_id: None,
            enabled: false,
        };
        let cfg = config_from_event(ev).unwrap();
        assert!(!cfg.bot_enabled);
    }

    #[test]
    fn settings_event_deserializes_without_role_id() {
        let json = r#"{
            "guild_id": "1",
            "channel_id": "2",
            "enabled": true
        }"#;
        let ev: SettingsUpdateEvent = serde_json::from_str(json).unwrap();
        assert!(ev.role_id.is_none());
    }

    #[test]
    fn settings_event_deserializes_with_null_role_id() {
        let json = r#"{
            "guild_id": "1",
            "channel_id": "2",
            "role_id": null,
            "enabled": true
        }"#;
        let ev: SettingsUpdateEvent = serde_json::from_str(json).unwrap();
        assert!(ev.role_id.is_none());
    }

    #[tokio::test]
    async fn cache_insert_and_read() {
        let cache = new_cache();
        {
            let mut map = cache.write().await;
            map.insert(
                "guild1".into(),
                GuildConfig {
                    guild_id: "guild1".into(),
                    channel_id: 100,
                    role_id: None,
                    bot_enabled: true,
                },
            );
        }
        let map = cache.read().await;
        assert_eq!(map.len(), 1);
        assert_eq!(map["guild1"].channel_id, 100);
    }

    #[tokio::test]
    async fn cache_update_overwrites() {
        let cache = new_cache();
        let cfg = GuildConfig {
            guild_id: "g".into(),
            channel_id: 1,
            role_id: None,
            bot_enabled: true,
        };
        {
            let mut map = cache.write().await;
            map.insert("g".into(), cfg);
        }
        {
            let mut map = cache.write().await;
            map.get_mut("g").unwrap().channel_id = 999;
        }
        let map = cache.read().await;
        assert_eq!(map["g"].channel_id, 999);
    }

    #[test]
    fn config_from_event_full_roundtrip() {
        let ev = SettingsUpdateEvent {
            guild_id: "42".into(),
            channel_id: "100".into(),
            role_id: Some("200".into()),
            enabled: true,
        };
        let cfg = config_from_event(ev).unwrap();
        assert_eq!(cfg.guild_id, "42");
        assert_eq!(cfg.channel_id, 100u64);
        assert_eq!(cfg.role_id, Some(200u64));
        assert!(cfg.bot_enabled);
    }
}
