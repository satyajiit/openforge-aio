//! Re-export hub for every shipped OpenForge game crate.
//!
//! Single edit point per new game. The `openforge-cli new-game` scaffolder
//! inserts a new `pub use openforge_game_<slug>;` line in alphabetical order
//! and appends to the `FORCE_LINK` slice.
//!
//! Each game crate's `inventory::submit!` calls fire at link time; merely
//! depending on the crate is enough to populate `openforge_runtime::REGISTRY`,
//! but the `#[used]` static below provides belt-and-suspenders against
//! aggressive linker dead-code elimination.

pub use openforge_game_batman_lod;

/// Touch every bundled game crate's `GAME_ID` symbol so the linker can't drop
/// the crate's `inventory::submit!` items. Call once during app startup; the
/// call site is a no-op at runtime.
pub fn ensure_linked() {
    #[used]
    static FORCE_LINK: &[&str] = &[openforge_game_batman_lod::GAME_ID];
    let _ = FORCE_LINK;
}
