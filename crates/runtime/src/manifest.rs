//! The per-game `manifest.toml` schema. Owned in `openforge-runtime` so that
//! game `build.rs` files, the discovery CLI, and the scaffolder all share one
//! authoritative parser.

use serde::{Deserialize, Serialize};

use crate::error::{RuntimeError, RuntimeResult};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GameManifest {
    pub game: GameManifestBody,
    #[serde(default)]
    pub icon: Option<IconSpec>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GameManifestBody {
    pub id: String,
    pub display_name: String,
    #[serde(default)]
    pub tagline: String,
    pub process_names: Vec<String>,
    #[serde(default)]
    pub primary_module: Option<String>,
    #[serde(default)]
    pub supported_versions: Vec<String>,
    #[serde(default)]
    pub forbidden_services: Vec<String>,
    #[serde(default = "default_true")]
    pub requires_admin: bool,
    #[serde(default = "default_sort_order")]
    pub sort_order: i32,
    /// File name of the per-game DLL that the trainer injects into the
    /// running game on Activate (e.g. `"batman_lod_dll.dll"`). The host
    /// crate resolves it to an absolute path via `resolve_dll_path`. Empty
    /// for games that don't ship a DLL (rare; reserved for future engines
    /// that don't need in-process reflection).
    #[serde(default)]
    pub dll_file_name: String,
}

fn default_true() -> bool {
    true
}

fn default_sort_order() -> i32 {
    1000
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IconSpec {
    pub path: String,
}

impl GameManifest {
    pub fn parse(toml_text: &str) -> RuntimeResult<Self> {
        toml::from_str(toml_text).map_err(|e| RuntimeError::SignatureParse(e.to_string()))
    }

    pub fn validate(&self) -> RuntimeResult<()> {
        if self.game.id.is_empty()
            || !self
                .game
                .id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        {
            return Err(RuntimeError::SignatureInvalid {
                feature: self.game.id.clone(),
                reason: "game id must be non-empty and match [a-z0-9_-]+".into(),
            });
        }
        if self.game.process_names.is_empty() {
            return Err(RuntimeError::SignatureInvalid {
                feature: self.game.id.clone(),
                reason: "process_names must contain at least one entry".into(),
            });
        }
        if !self.game.dll_file_name.is_empty() && !self.game.dll_file_name.ends_with(".dll") {
            return Err(RuntimeError::SignatureInvalid {
                feature: self.game.id.clone(),
                reason: "dll_file_name must end with .dll".into(),
            });
        }
        Ok(())
    }
}
