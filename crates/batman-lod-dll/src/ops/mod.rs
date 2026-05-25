//! In-DLL implementations of the single-channel memory ops.
//!
//! All ops execute in the game's address space (we are the DLL). Reads and
//! writes go through `LocalReader`, which routes via
//! `ReadProcessMemory(GetCurrentProcess())` /
//! `WriteProcessMemory(GetCurrentProcess())` — both documented to fault-isolate
//! (return `FALSE` on bad pages, no SEH frame required). The code-patch op
//! additionally wraps writes in a `VirtualProtect(PAGE_EXECUTE_READWRITE)`
//! dance so it can overwrite instructions in `.text`.
//!
//! Op state that needs to outlive a single request — currently just the
//! applied-patches map for auto-restore on disconnect — lives in
//! [`state::ConnState`], which the worker constructs per pipe client and
//! drops (running the auto-restore loop) when the client goes away.

pub mod aob_scan;
pub mod code_patch;
pub mod heap_scan;
pub mod lua;
pub mod reflection;
pub mod state;
