//! Template build.rs. The scaffolder copies this file verbatim into new game
//! crates. All codegen lives in `openforge-buildgen` (one shared generator for
//! every game crate); see that crate's docs for the manifest schema mirror.

fn main() {
    openforge_buildgen::generate();
}
