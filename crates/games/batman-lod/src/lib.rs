//! OpenForge game module: LEGO Batman — Legacy of the Dark Knight.
//!
//! Static game metadata + declarative feature list come from `manifest.toml`
//! and `signatures/*.toml` via this crate's `build.rs`. The build script
//! generates constants like `MANIFEST_GAME_ID`, `DLL_FILE_NAME`,
//! `PROCESS_NAMES`, etc., which we re-export here as the crate's public API.

include!(concat!(env!("OUT_DIR"), "/game_generated.rs"));

/// Stable identifier for this game across the runtime, signatures, and
/// profiles. Sourced from `manifest.toml` so the const and the manifest
/// never drift.
pub const GAME_ID: &str = MANIFEST_GAME_ID;

pub struct BatmanGame;

impl openforge_runtime::Game for BatmanGame {
    fn id(&self) -> &'static str {
        GAME_ID
    }
    fn display_name(&self) -> &'static str {
        DISPLAY_NAME
    }
    fn tagline(&self) -> &'static str {
        TAGLINE
    }
    fn process_names(&self) -> &'static [&'static str] {
        PROCESS_NAMES
    }
    fn primary_module(&self) -> Option<&'static str> {
        PRIMARY_MODULE
    }
    fn supported_versions(&self) -> &'static [&'static str] {
        SUPPORTED_VERSIONS
    }
    fn forbidden_services(&self) -> &'static [&'static str] {
        FORBIDDEN_SERVICES
    }
    fn icon_png(&self) -> &'static [u8] {
        ICON_PNG
    }
    fn requires_admin(&self) -> bool {
        REQUIRES_ADMIN
    }
    fn sort_order(&self) -> i32 {
        SORT_ORDER
    }
    fn declarative_features(&self) -> &'static [openforge_runtime::DeclFeatureSrc] {
        DECLARATIVE_FEATURES
    }
    fn dll_file_name(&self) -> &'static str {
        DLL_FILE_NAME
    }
    fn engine_kind(&self) -> Option<openforge_runtime::EngineKind> {
        ENGINE_KIND.and_then(openforge_runtime::EngineKind::from_manifest_str)
    }
    fn engine_dll(&self) -> Option<&'static str> {
        ENGINE_DLL
    }
}

openforge_runtime::register_game!(BatmanGame);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_id_matches_manifest() {
        assert_eq!(GAME_ID, MANIFEST_GAME_ID);
    }

    #[test]
    fn process_names_non_empty() {
        assert!(!PROCESS_NAMES.is_empty());
    }

    #[test]
    fn engine_kind_is_ue5() {
        use openforge_runtime::Game;
        assert_eq!(
            BatmanGame.engine_kind(),
            Some(openforge_runtime::EngineKind::Ue5)
        );
    }
}
