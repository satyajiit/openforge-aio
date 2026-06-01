//! Shared injected-DLL primitives (PE introspection, SEH guard, panic guard,
//! fault-isolated local reads, ring log) used by every per-game reflection DLL.
//! Pure mechanical extraction — no behavior change; see git history.

pub mod local_reader;
pub mod panic_guard;
pub mod pe;
