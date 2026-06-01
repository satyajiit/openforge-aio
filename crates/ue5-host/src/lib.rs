//! `openforge-ue5-host`: inject a per-game OpenForge DLL into a UE5 game
//! process and drive its named-pipe protocol.
//!
//! AIO scaling: this crate is game-agnostic. Every public API that touches
//! "the DLL" takes either a `&Path` to the DLL file or a filename string —
//! the caller (which is game-aware) supplies them. The game crate that
//! owns the DLL is the canonical source of the filename, e.g.
//! `openforge_game_batman_lod::DLL_FILE_NAME`.
//!
//! The two crates that consume this:
//!
//! * `openforge-discover` — replaces its in-process `UeEngine` with a
//!   [`Ue5Session`], turning every reflection RPC (walk objects, props,
//!   funcs; read/write property) into a pipe round-trip.
//! * `openforge-app` (Tauri trainer) — uses [`Injector::inject`] to attach
//!   the DLL and [`Ue5Client`]/[`Ue5Session`] to drive feature workflows.
//!
//! ## Topology
//!
//! ```text
//!   trainer (this crate) ──inject──► game.exe
//!         │                            │
//!         │  named pipe (msg mode)     │
//!         └────────  \\.\pipe\…  ◄─────┘
//!                      ▲ length-prefixed postcard frames
//! ```
//!
//! The DLL (e.g. `openforge-batman-lod-dll`) is the server. We are always
//! the client. The protocol lives in `openforge-ue5-protocol`.

#![cfg(windows)]
// We use `unsafe` only inside `injector`, `pipe`, and the small helpers in
// `error::HostError::last_win32` — all guarded with SAFETY comments. Outside
// those modules, the public surface is safe Rust.
#![deny(rust_2018_idioms)]

pub mod backend;
mod client;
mod dll_path;
mod error;
mod injector;
mod pipe;
mod session;

pub use crate::backend::Ue5Backend;
pub use crate::client::Ue5Client;
pub use crate::dll_path::{DLL_PATH_ENV, resolve_dll_path};
pub use crate::error::{HostError, Result};
pub use crate::injector::Injector;
pub use crate::session::{DEFAULT_CONNECT_TIMEOUT, Ue5Session};

// Re-export the protocol so downstream crates can `use openforge_ue5_host::*`
// and not need a separate dependency on `openforge-ue5-protocol`.
pub use openforge_ue5_protocol as protocol;

/// Resolved UE5 layout, mirrored from `Response::Welcome` for ergonomic
/// host-side access.
///
/// Fields are 1:1 with the protocol's `Response::Welcome { .. }` variant.
#[derive(Debug, Clone)]
pub struct Welcome {
    pub server_version: u32,
    pub pid: u32,
    pub guobject_array: u64,
    pub fname_pool: u64,
    pub fname_pool_chunks_offset: u32,
    pub offsets: openforge_ue5_protocol::UeOffsets,
    pub fuobject_item_stride: u32,
    pub layout_validated: bool,
}
