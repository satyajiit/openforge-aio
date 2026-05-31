//! OpenForge game module: 007 First Light (IO Interactive, Glacier 2 engine).
//!
//! Static game metadata + declarative feature list come from `manifest.toml`
//! and `signatures/*.toml` via this crate's `build.rs`, which generates the
//! constants re-exported here as the crate's public API.
//!
//! Unlike the UE5 games, First Light ships **no DLL** (`DLL_FILE_NAME == ""`):
//! the trainer attaches via external `ReadProcessMemory`/`WriteProcessMemory`
//! (an `openforge_core::Target` as the `Ctx`). Features use non-`[reflection]`
//! locators — `[signature]` / `[heap_scan]` / `[pointer_chain]` — or the
//! Glacier-specific `[glacier_reflection]` block, which the runtime resolves
//! through `openforge-glacier-host` over that same `Ctx`. (UE5 `[reflection]`
//! is unsupported here — `Target` has no in-process DLL to walk objects.)

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
    fn external_rpm_backend_declares_no_dll() {
        // First Light is the external-RPM backend; an empty DLL name is the
        // signal attach.rs uses to pick the Target/Ctx path over UE5 injection.
        assert_eq!(DLL_FILE_NAME, "");
    }
}
