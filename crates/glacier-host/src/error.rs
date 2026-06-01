//! Typed errors for the Glacier host-driver (`client` feature).

use std::path::PathBuf;

use openforge_glacier_protocol::FrameError;
use openforge_host_common::HostCommonError;

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

/// Map the shared host-plumbing error into this crate's `HostError` 1:1, so
/// the error *value* a caller sees from `Injector` / `resolve_dll_path` / the
/// pipe transport is unchanged from the pre-dedup code.
impl From<HostCommonError> for HostError {
    fn from(e: HostCommonError) -> Self {
        match e {
            HostCommonError::InjectionFailed(s) => HostError::InjectionFailed(s),
            HostCommonError::DllNotFound(p) => HostError::DllNotFound(p),
            HostCommonError::Io(e) => HostError::Io(e),
            HostCommonError::Disconnected => HostError::Disconnected,
            HostCommonError::FrameOversize { got, max } => {
                HostError::Protocol(FrameError::Oversize { got, max })
            }
            HostCommonError::FrameEmpty => HostError::Protocol(FrameError::Empty { got: 0 }),
            HostCommonError::Postcard(e) => HostError::Postcard(e),
            HostCommonError::Win32 { call, code } => HostError::Win32 { call, code },
        }
    }
}
