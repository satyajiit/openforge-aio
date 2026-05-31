//! OpenForge game module: 007 First Light (IO Interactive, Glacier 2 engine).
//!
//! Static game metadata + the (currently empty) declarative feature list come
//! from `manifest.toml` + `signatures/*.toml` via this crate's `build.rs`,
//! re-exported here as the crate's public API. Registering this crate is what
//! makes 007 First Light appear and be selectable in the app.
//!
//! Backend status: First Light's Glacier reflection type system is walked by
//! `openforge-glacier-host`, today reachable only through the
//! `openforge-discover` dev CLI (`glacier-walk` / `glacier-entity`) — there is
//! **no `[glacier_reflection]` runtime locator yet**. Gameplay features need
//! on-thread actuation (ZCL logic-node input pins + property setters; external
//! RPM can't fire them or reliably enumerate live entities), so the shipping
//! backend is a planned **injected DLL** served over a named pipe, like the UE5
//! `Ue5Session`. Until that lands the crate ships zero features and
//! `DLL_FILE_NAME` is empty as a placeholder.

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
    fn dll_file_name_is_placeholder_until_glacier_dll_lands() {
        // Empty for now: the Tier-2 glacier-dll backend isn't built yet, so
        // there is no DLL to name. attach.rs still hard-errors on this — wiring
        // the Glacier backend dispatch is the next step.
        assert_eq!(DLL_FILE_NAME, "");
    }
}
