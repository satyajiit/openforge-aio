use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub schema_version: u32,
    pub theme_mode: ThemeMode,
    pub last_active_game: Option<String>,
    pub hotkeys_enabled: bool,
    pub telemetry: Telemetry,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Telemetry {
    Off,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: 1,
            theme_mode: ThemeMode::Dark,
            last_active_game: None,
            hotkeys_enabled: false,
            telemetry: Telemetry::Off,
        }
    }
}

pub fn load(path: &Path) -> AppResult<Settings> {
    if !path.exists() {
        let s = Settings::default();
        save(path, &s)?;
        return Ok(s);
    }
    let text = fs::read_to_string(path)?;
    serde_json::from_str(&text).map_err(|e| AppError::Persistence(e.to_string()))
}

pub fn save(path: &Path, settings: &Settings) -> AppResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(settings)?;
    fs::write(&tmp, json)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn default_settings_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let s = load(&path).unwrap();
        assert_eq!(s.schema_version, 1);
        save(&path, &s).unwrap();
        let s2 = load(&path).unwrap();
        assert_eq!(s.theme_mode, s2.theme_mode);
    }
}
