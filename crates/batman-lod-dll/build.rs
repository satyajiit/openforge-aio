//! Build script: compile the C SEH (Structured Exception Handling) shim so
//! that calls into engine code can survive access violations when an AOB
//! match turns out to be the wrong function. Rust does not expose Windows
//! `__try`/`__except` directly; the C shim wraps each risky function call.

fn main() {
    let mut build = cc::Build::new();
    build.file("src/seh.c");
    // MSVC: SEH is built-in via /EHa-compatible code. The compiler's __try
    // intrinsic only works in functions WITHOUT /EHa exception unwinding.
    // We use plain C here so the default model applies.
    build.compile("openforge_ue5_seh");
    println!("cargo:rerun-if-changed=src/seh.c");
}
