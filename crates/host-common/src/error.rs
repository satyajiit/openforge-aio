//! Errors for the shared host-side plumbing.
//!
//! Engine-agnostic on purpose: the per-engine `*-host` crate maps this into its
//! own `HostError` 1:1 (`From<HostCommonError>`), so the final error *value* a
//! caller sees is unchanged from the pre-dedup code. Variants are exactly the
//! set the shared modules ([`crate::injector`], [`crate::dll_path`],
//! [`crate::transport`]) can produce.

use std::path::PathBuf;

/// All error paths in the shared host plumbing funnel through this type.
#[derive(Debug, thiserror::Error)]
pub enum HostCommonError {
    #[error("DLL injection failed: {0}")]
    InjectionFailed(String),

    #[error("DLL not found at {0}")]
    DllNotFound(PathBuf),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("pipe disconnected")]
    Disconnected,

    /// Length prefix declared a frame larger than the protocol's cap.
    #[error("frame too large: {got} bytes (max {max})")]
    FrameOversize { got: u32, max: u32 },

    /// Length prefix declared a zero-length frame.
    #[error("frame too small: 0 bytes (min 1)")]
    FrameEmpty,

    #[error("postcard codec error: {0}")]
    Postcard(#[from] postcard::Error),

    #[error("Win32 call {call} failed (code {code})")]
    Win32 { call: &'static str, code: u32 },
}

/// Crate-local `Result` alias.
pub type Result<T> = std::result::Result<T, HostCommonError>;

impl HostCommonError {
    /// Build a `Win32` variant from the last OS error.
    #[cfg(windows)]
    pub(crate) fn last_win32(call: &'static str) -> Self {
        // SAFETY: `GetLastError` has no preconditions and is thread-local.
        let code = unsafe { windows::Win32::Foundation::GetLastError().0 };
        HostCommonError::Win32 { call, code }
    }
}
