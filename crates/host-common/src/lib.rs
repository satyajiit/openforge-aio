//! Shared host-side engine plumbing: DLL injection (LoadLibrary remote-thread)
//! and named-pipe transport (length-prefixed postcard framing). Engine-agnostic;
//! per-engine protocol + session semantics live in the engine's own `*-host`
//! crate.
//!
//! Pure mechanical extraction from `openforge-ue5-host` / `openforge-glacier-host`
//! — no behavior change. Each `*-host` crate maps [`error::HostCommonError`]
//! into its own `HostError` 1:1, so error *values* are unchanged.
//!
//! Everything here is Win32-specific and only compiled on Windows.

#![cfg(windows)]

pub mod dll_path;
pub mod error;
pub mod injector;
pub mod transport;

pub use crate::dll_path::{DLL_PATH_ENV, resolve_dll_path};
pub use crate::error::{HostCommonError, Result as HostCommonResult};
pub use crate::injector::Injector;
pub use crate::transport::{PipeHandle, Protocol, recv_frame, send_frame};
