//! Typed errors for the host crate.

use std::path::PathBuf;

use openforge_ue5_protocol::FrameError;

/// All error paths in the host crate funnel through this type.
///
/// Variants intentionally distinguish *where* the failure happened so callers
/// can decide whether to retry (transient Win32 / IO), re-inject
/// (`InjectionFailed`, `Disconnected`), or surface a hard error to the user
/// (`Server`, `LayoutUnresolved`).
#[derive(Debug, thiserror::Error)]
pub enum HostError {
    #[error("process not found: {0}")]
    ProcessNotFound(String),

    #[error("DLL injection failed: {0}")]
    InjectionFailed(String),

    #[error("DLL not found at {0}")]
    DllNotFound(PathBuf),

    #[error("handshake with DLL failed: {0}")]
    HandshakeFailed(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("protocol/frame error: {0}")]
    Protocol(#[from] FrameError),

    #[error("postcard codec error: {0}")]
    Postcard(#[from] postcard::Error),

    #[error("server-side error: {0}")]
    Server(String),

    #[error("pipe disconnected")]
    Disconnected,

    #[error("DLL reported layout not validated; host refuses to operate")]
    LayoutUnresolved,

    #[error("unexpected response variant: {0}")]
    InvalidResponse(&'static str),

    #[error("Win32 call {call} failed (code {code})")]
    Win32 { call: &'static str, code: u32 },
}

/// Crate-local `Result` alias.
pub type Result<T> = std::result::Result<T, HostError>;

impl HostError {
    /// Build a `Win32` variant from the last OS error.
    #[cfg(windows)]
    pub(crate) fn last_win32(call: &'static str) -> Self {
        // SAFETY: `GetLastError` has no preconditions and is thread-local.
        let code = unsafe { windows::Win32::Foundation::GetLastError().0 };
        HostError::Win32 { call, code }
    }
}
