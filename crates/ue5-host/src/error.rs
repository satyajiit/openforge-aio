//! Typed errors for the host crate.

use std::path::PathBuf;

use openforge_host_common::HostCommonError;
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
