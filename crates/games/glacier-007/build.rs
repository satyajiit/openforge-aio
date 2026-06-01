//! Emits `$OUT_DIR/game_generated.rs` from `manifest.toml` + `signatures/` +
//! `assets/icon.png`. All the work lives in `openforge-buildgen` (one shared
//! codegen for every game crate); see that crate's docs for the schema mirror.

fn main() {
    openforge_buildgen::generate();
}
