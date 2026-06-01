//! The per-game `manifest.toml` schema. Owned in `openforge-runtime` so that
//! game `build.rs` files, the discovery CLI, and the scaffolder all share one
//! authoritative parser.

use serde::{Deserialize, Serialize};

use crate::error::{RuntimeError, RuntimeResult};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GameManifest {
    /// Engine + config-format declaration (`[engine]`). `#[serde(default)]`
    /// so legacy manifests without the block still parse with today's
    /// behavior: `schema == 1`, `kind == None` (back-compat guarantee).
    #[serde(default)]
    pub engine: EngineDecl,
    pub game: GameManifestBody,
    #[serde(default)]
    pub icon: Option<IconSpec>,
}

/// Which game engine a manifest targets. Parsed in this phase; nothing
/// dispatches on it for runtime behavior yet (that arrives a later phase).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineKind {
    Ue5,
    Glacier2,
}

impl EngineKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            EngineKind::Ue5 => "ue5",
            EngineKind::Glacier2 => "glacier2",
        }
    }

    /// Parse a manifest `[engine].kind` string into an [`EngineKind`].
    /// Mirrors the `#[serde(rename_all = "snake_case")]` mapping so the
    /// build-time codegen (which embeds the raw string) and the runtime agree.
    /// Returns `None` for an unrecognized kind (legacy fallback handles that).
    pub fn from_manifest_str(s: &str) -> Option<EngineKind> {
        match s {
            "ue5" => Some(EngineKind::Ue5),
            "glacier2" => Some(EngineKind::Glacier2),
            _ => None,
        }
    }
}

/// The `[engine]` declaration block. Schema/kind/config-format/dll only in
/// this phase; per-engine data-constant sub-tables (`[engine.ue5]` etc.)
/// arrive with a later phase.
///
/// `Default` is hand-written (not derived) so an entirely-absent `[engine]`
/// block — which deserializes via `GameManifest`'s struct-level
/// `#[serde(default)]` to `EngineDecl::default()` — yields the same
/// `schema == 1` legacy value as a present-but-empty `[engine]` table (which
/// fills `schema` via the field-level `default_schema`). A derived `Default`
/// would give `schema == 0` and break the back-compat contract.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineDecl {
    /// Schema version. Absent on legacy manifests => 1. Schema >= 2 must set `kind`.
    #[serde(default = "default_schema")]
    pub schema: u8,
    /// Declared engine. None on legacy (schema 1) => inferred by
    /// `legacy_engine_for()` at dispatch time (a LATER phase).
    #[serde(default)]
    pub kind: Option<EngineKind>,
    /// Default config format for this game's signatures (per-file extension
    /// still wins). Defaults to Toml.
    #[serde(default)]
    pub config_format: crate::format::ConfigFormat,
    /// Optional explicit DLL name override (else the engine backend's default
    /// is used; resolved a LATER phase).
    #[serde(default)]
    pub dll: Option<String>,
}

impl Default for EngineDecl {
    fn default() -> Self {
        Self {
            schema: default_schema(),
            kind: None,
            config_format: crate::format::ConfigFormat::default(),
            dll: None,
        }
    }
}

fn default_schema() -> u8 {
    1
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
        Self::parse_with(toml_text, crate::format::ConfigFormat::Toml)
    }

    /// Parse from `src` in the given [`crate::format::ConfigFormat`]. `parse`
    /// is the TOML-pinned convenience wrapper. Behavior for TOML is identical
    /// to the previous `toml::from_str` path (same `RuntimeError::SignatureParse`
    /// error variant — kept stable for golden snapshots).
    pub fn parse_with(src: &str, fmt: crate::format::ConfigFormat) -> RuntimeResult<Self> {
        crate::format::parse_str(src, fmt).map_err(RuntimeError::SignatureParse)
    }

    pub fn validate(&self) -> RuntimeResult<()> {
        // Schema >= 2 manifests must declare an engine kind. Schema 1 / an
        // absent `[engine]` block stays valid (back-compat). Nothing
        // dispatches on `kind` yet — that is a later phase.
        if self.engine.schema >= 2 && self.engine.kind.is_none() {
            return Err(RuntimeError::SignatureInvalid {
                feature: self.game.id.clone(),
                reason: "manifest [engine] schema >= 2 requires kind = \"ue5\"|\"glacier2\"".into(),
            });
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::ConfigFormat;

    const LEGACY: &str = r#"
        [game]
        id = "legacy-game"
        display_name = "Legacy Game"
        process_names = ["legacy.exe"]
    "#;

    #[test]
    fn legacy_manifest_without_engine_block_is_backcompat() {
        let m = GameManifest::parse(LEGACY).expect("parse legacy manifest");
        assert_eq!(m.engine.schema, 1);
        assert_eq!(m.engine.kind, None);
        assert_eq!(m.engine.config_format, ConfigFormat::Toml);
        assert_eq!(m.engine.dll, None);
        m.validate().expect("legacy manifest validates");
    }

    #[test]
    fn schema_2_without_kind_fails_validate() {
        let src = r#"
            [engine]
            schema = 2

            [game]
            id = "no-kind"
            display_name = "No Kind"
            process_names = ["nk.exe"]
        "#;
        let m = GameManifest::parse(src).expect("parse manifest");
        assert_eq!(m.engine.schema, 2);
        assert_eq!(m.engine.kind, None);
        let err = m
            .validate()
            .expect_err("schema 2 without kind must fail validate");
        match err {
            RuntimeError::SignatureInvalid { reason, .. } => {
                assert!(
                    reason.contains("schema >= 2 requires kind"),
                    "reason was: {reason}"
                );
            }
            other => panic!("expected SignatureInvalid, got {other:?}"),
        }
    }

    #[test]
    fn glacier2_kind_parses() {
        let src = r#"
            [engine]
            schema = 2
            kind = "glacier2"
            dll = "glacier_007_dll.dll"

            [game]
            id = "g2"
            display_name = "G2"
            process_names = ["g2.exe"]
        "#;
        let m = GameManifest::parse(src).expect("parse manifest");
        assert_eq!(m.engine.kind, Some(EngineKind::Glacier2));
        assert_eq!(m.engine.kind.unwrap().as_str(), "glacier2");
        assert_eq!(m.engine.dll.as_deref(), Some("glacier_007_dll.dll"));
        m.validate().expect("validates");
    }

    #[test]
    fn ue5_kind_parses() {
        let src = r#"
            [engine]
            schema = 2
            kind = "ue5"

            [game]
            id = "u5"
            display_name = "U5"
            process_names = ["u5.exe"]
        "#;
        let m = GameManifest::parse(src).expect("parse manifest");
        assert_eq!(m.engine.kind, Some(EngineKind::Ue5));
        assert_eq!(m.engine.kind.unwrap().as_str(), "ue5");
        m.validate().expect("validates");
    }
}
