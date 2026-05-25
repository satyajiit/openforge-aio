//! Per-pipe-client state. Today: applied code patches that auto-revert on
//! disconnect. Built once per `serve_client` invocation; dropped (running
//! restore loops in the process) when the client goes away.

use std::collections::HashMap;

use crate::ops::code_patch;

/// Per-connection state. Held by the worker for the lifetime of one pipe
/// client. Drop runs the auto-restore loop for any code patch still in
/// flight, so a crashed trainer or a clean client disconnect both leave
/// the game's `.text` exactly as it was found.
pub struct ConnState {
    /// `addr -> original_bytes`. Inserted by `code_patch::apply`; removed
    /// by `code_patch::restore` (idempotent) and by `Drop` (auto-restore).
    pub applied_patches: HashMap<usize, Vec<u8>>,
}

impl ConnState {
    pub fn new() -> Self {
        Self {
            applied_patches: HashMap::new(),
        }
    }

    /// Restore every patch still tracked here, logging per-patch success /
    /// failure. Called from `Drop`; also callable on explicit teardown.
    pub fn restore_all(&mut self) {
        if self.applied_patches.is_empty() {
            return;
        }
        let count = self.applied_patches.len();
        crate::flog!("INFO", "conn-drop: restoring {count} applied patches");
        for (addr, original) in self.applied_patches.drain() {
            match code_patch::write_bytes_at(addr, &original) {
                Ok(()) => crate::flog!(
                    "INFO",
                    "conn-drop: restored 0x{addr:X} ({} bytes)",
                    original.len()
                ),
                Err(e) => crate::flog!("ERROR", "conn-drop: restore 0x{addr:X} failed: {e:?}"),
            }
        }
    }
}

impl Default for ConnState {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ConnState {
    fn drop(&mut self) {
        self.restore_all();
    }
}
