//! Wrap request handling in `catch_unwind` so a panic anywhere in the dispatch
//! path returns to the worker loop instead of unwinding into the game.
//!
//! The workspace builds every injected DLL with `panic = "unwind"` (see the
//! root `Cargo.toml` rationale): an injected reflection server walking
//! untrusted pointer chains must never abort the host process on a single
//! panic. `catch_unwind` turns a panic into a recoverable `Err(String)`.

use std::panic::{AssertUnwindSafe, catch_unwind};

/// Run `f` under a panic guard. On panic, return a best-effort reason string
/// (panic payloads are usually `&str` or `String`).
pub fn guarded<R>(f: impl FnOnce() -> R) -> Result<R, String> {
    catch_unwind(AssertUnwindSafe(f)).map_err(|p| {
        if let Some(s) = p.downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = p.downcast_ref::<String>() {
            s.clone()
        } else {
            "non-string panic payload".to_string()
        }
    })
}
