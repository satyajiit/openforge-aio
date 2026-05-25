//! Helpers to wrap request handlers in `catch_unwind` so a panic in any one
//! handler returns a `Response::Error(...)` instead of bringing the game down.
//!
//! NOTE: with `panic = "abort"` in the release profile, `catch_unwind` is a
//! no-op and a panic still aborts the process. The handlers must therefore
//! avoid panicking; we keep the `catch_unwind` wrapper so the contract still
//! holds when built with `panic = "unwind"`, and so callers don't have to know
//! which profile is in use.

use std::panic::{AssertUnwindSafe, catch_unwind};

/// Run `f` under a panic guard. On panic, return a string describing the
/// reason (best-effort: panic payloads are often `&str` or `String`).
pub fn guarded<R>(f: impl FnOnce() -> R) -> Result<R, String> {
    let res = catch_unwind(AssertUnwindSafe(f));
    res.map_err(|p| {
        if let Some(s) = p.downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = p.downcast_ref::<String>() {
            s.clone()
        } else {
            "non-string panic payload".to_string()
        }
    })
}
