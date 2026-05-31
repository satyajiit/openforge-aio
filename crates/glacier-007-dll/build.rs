//! Compile the C SEH shim (`src/seh.c`) so the DLL can call Glacier engine
//! functions under `__try`/`__except`. Rust does not expose Windows SEH
//! directly; the shim wraps each risky indirect call. Plain C (no `/EHa`) so
//! the `__try` intrinsic applies the default exception model.

fn main() {
    let mut build = cc::Build::new();
    build.file("src/seh.c");
    build.compile("openforge_glacier_seh");
    println!("cargo:rerun-if-changed=src/seh.c");
}
