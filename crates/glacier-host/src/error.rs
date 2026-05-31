//! Typed errors for the Glacier host-driver (`client` feature).

use std::path::PathBuf;

use openforge_glacier_protocol::FrameError;

/// All host-driver error paths funnel through this type.
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

    #[error("unexpected response variant: {0}")]
    InvalidResponse(&'static str),

    #[error("Win32 call {call} failed (code {code})")]
    Win32 { call: &'static str, code: u32 },
}

/// Crate-local `Result` alias for the host driver.
pub type Result<T> = std::result::Result<T, HostError>;

impl HostError {
    /// Build a `Win32` variant from the last OS error.
    pub(crate) fn last_win32(call: &'static str) -> Self {
        // SAFETY: `GetLastError` has no preconditions and is thread-local.
        let code = unsafe { windows::Win32::Foundation::GetLastError().0 };
        HostError::Win32 { call, code }
    }
}
