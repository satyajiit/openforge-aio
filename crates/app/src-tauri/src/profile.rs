use std::collections::HashMap;
use std::fs;
use std::path::Path;

use openforge_runtime::Value;
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameProfile {
    pub schema_version: u32,
    pub game_id: String,
    pub last_seen_version: Option<String>,
    pub feature_state: HashMap<String, FeatureProfile>,
    /// Per-session resolved-address cache. Keyed by `feature_id`. Populated
    /// on a successful attach so the next attach within the same game
    /// session can skip the full heap-scan (which is multi-second on big
    /// UE5 titles). Entries are invalidated automatically on `module_base`
    /// mismatch (process restart) or when the feature's `quick_check`
    /// rejects them.
    #[serde(default)]
    pub feature_address_cache: HashMap<String, CachedAddress>,
}

/// One entry in the resolved-address cache. The `module_base` is used as a
/// cheap session-fingerprint: when the game restarts, ASLR puts the main
/// module at a new base and every cached address is stale.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedAddress {
    pub address: u64,
    pub game_version: String,
    pub module_base: u64,
    pub captured_unix_secs: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeatureProfile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frozen: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_patch_applied: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_apply_on_attach: Option<bool>,
}

impl GameProfile {
    pub fn empty(game_id: &str) -> Self {
        Self {
            schema_version: 2,
            game_id: game_id.to_string(),
            last_seen_version: None,
            feature_state: HashMap::new(),
            feature_address_cache: HashMap::new(),
        }
    }
}

pub fn load(path: &Path, game_id: &str) -> AppResult<GameProfile> {
    if !path.exists() {
        return Ok(GameProfile::empty(game_id));
    }
    let text = fs::read_to_string(path)?;
    serde_json::from_str(&text).map_err(|e| AppError::Persistence(e.to_string()))
}

pub fn save(path: &Path, profile: &GameProfile) -> AppResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(profile)?;
    fs::write(&tmp, json)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn missing_profile_returns_empty() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("missing.json");
        let p = load(&path, "batman").unwrap();
        assert_eq!(p.game_id, "batman");
        assert!(p.feature_state.is_empty());
    }

    #[test]
    fn round_trip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("batman.json");
        let mut p = GameProfile::empty("batman");
        p.feature_state.insert(
            "studs".to_string(),
            FeatureProfile {
                value: Some(Value::I32(999_999)),
                frozen: Some(false),
                ..Default::default()
            },
        );
        save(&path, &p).unwrap();
        let p2 = load(&path, "batman").unwrap();
        assert_eq!(p2.feature_state.len(), 1);
    }
}
