//! OpenForge game module: 007 First Light (IO Interactive, Glacier 2 engine).
//!
//! Static game metadata + the (currently empty) declarative feature list come
//! from `manifest.toml` + `signatures/*.toml` via this crate's `build.rs`,
//! re-exported here as the crate's public API. Registering this crate is what
//! makes 007 First Light appear and be selectable in the app.
//!
//! Backend status: the Tier-2 injected DLL (`crates/glacier-007-dll`) is built.
//! It runs the shared `openforge-glacier-host` reflection engine *in-process*
//! and serves memory + reflection ops over a named pipe to the trainer's
//! `GlacierSession` (host side), exactly like the UE5 `Ue5Session`. The
//! discover CLI drives the same stack via `glacier-dll` (and the external-RPM
//! `glacier-walk` / `glacier-entity` remain for bootstrap validation).
//! `DLL_FILE_NAME` names that cdylib. Declarative gameplay features (which need
//! a `[glacier_reflection]` runtime locator + the app attach-backend dispatch)
//! are the next layer; until those land the crate ships zero features.

include!(concat!(env!("OUT_DIR"), "/game_generated.rs"));

/// Stable identifier for this game across the runtime, signatures, and
/// profiles. Sourced from `manifest.toml` so the const and the manifest
/// never drift.
pub const GAME_ID: &str = MANIFEST_GAME_ID;

pub struct GlacierGame;

impl openforge_runtime::Game for GlacierGame {
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

openforge_runtime::register_game!(GlacierGame);

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
    fn engine_kind_is_glacier2() {
        use openforge_runtime::Game;
        assert_eq!(
            GlacierGame.engine_kind(),
            Some(openforge_runtime::EngineKind::Glacier2)
        );
    }

    #[test]
    fn dll_file_name_points_at_glacier_dll() {
        // The Tier-2 backend (crates/glacier-007-dll) is built; the manifest
        // names the cdylib the host injects + drives over the named pipe.
        assert_eq!(DLL_FILE_NAME, "glacier_007_dll.dll");
    }
}
