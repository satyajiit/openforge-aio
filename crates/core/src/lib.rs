//! OpenForge core.
//!
//! Process attach, memory I/O, AOB (array-of-bytes) scanning, and address resolution
//! primitives. All consumers (the trainer app, the discovery CLI, individual game
//! crates) build on this.

pub mod ctx;
pub mod error;
pub mod memory;
pub mod module;
pub mod pattern;
pub mod process;
pub mod resolve;
pub mod target;

pub use crate::ctx::{Ctx, FieldKind, FoundObject, Predicate, ResolvedField};
pub use crate::error::{Error, Result};
pub use crate::memory::MemoryRegion;
pub use crate::module::Module;
pub use crate::pattern::Pattern;
pub use crate::process::{ProcessInfo, ProcessSnapshot, wait_for_pid_exit};
pub use crate::resolve::Resolve;
pub use crate::target::{ProgressFn, Target};
