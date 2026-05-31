//! An in-process [`openforge_core::Ctx`] backed by [`LocalReader`].
//!
//! This is the seam that lets the shared `openforge-glacier-host`
//! `GlacierReflection` engine — written against `&dyn Ctx` for external RPM —
//! run *inside* the game process unchanged. Every memory access becomes a
//! fault-isolated `ReadProcessMemory(GetCurrentProcess())` of our own address
//! space, and the heap/AOB scans run in-process (orders of magnitude faster
//! than the cross-process variants).
//!
//! The reflection-specific `Ctx` methods (`find_uobject`, `resolve_property`,
//! `call_ufunction`, …) are UE5 concepts and keep their default erroring
//! impls — `GlacierReflection` never calls them; it uses only `read_bytes`,
//! `main_module`, and `scan_heap_for_u64`.

use openforge_core::{Ctx, Error, Module, Pattern, Result};

use crate::local_reader::LocalReader;
use crate::pe::enumerate_modules;
use crate::scan;

/// In-process Ctx. Holds a snapshot of the loaded modules so the `&Module`-
/// returning trait methods can hand out borrows; the main module never moves
/// for the life of the process, so a one-time snapshot is correct.
pub struct LocalCtx {
    pid: u32,
    modules: Vec<Module>,
}

impl LocalCtx {
    /// Build a Ctx over the current process. Returns `None` if module
    /// enumeration came back empty (Toolhelp32 failure — unrecoverable).
    pub fn new() -> Option<Self> {
        let pid = unsafe { windows::Win32::System::Threading::GetCurrentProcessId() };
        let modules: Vec<Module> = enumerate_modules()
            .into_iter()
            .map(|m| Module {
                name: m.name,
                base: m.base,
                size: m.size,
                text_offset: m.text_offset,
                text_size: m.text_size,
            })
            .collect();
        if modules.is_empty() {
            return None;
        }
        Some(Self { pid, modules })
    }
}

impl Ctx for LocalCtx {
    fn pid(&self) -> u32 {
        self.pid
    }

    fn main_module(&self) -> &Module {
        // Toolhelp32 reports the EXE first.
        &self.modules[0]
    }

    fn module(&self, name: &str) -> Option<&Module> {
        if name.is_empty() {
            return Some(self.main_module());
        }
        self.modules
            .iter()
            .find(|m| m.name.eq_ignore_ascii_case(name))
    }

    fn read_bytes(&self, addr: usize, buf: &mut [u8]) -> Result<()> {
        LocalReader::new()
            .read_bytes(addr, buf)
            .map_err(|e| Error::Custom(format!("local read at 0x{addr:X}: {e:?}")))
    }

    fn write_bytes(&self, addr: usize, buf: &[u8]) -> Result<()> {
        LocalReader::new()
            .write_bytes(addr, buf)
            .map_err(|e| Error::Custom(format!("local write at 0x{addr:X}: {e:?}")))
    }

    fn scan_module(&self, name: &str, pattern: &Pattern) -> Result<Option<usize>> {
        Ok(scan::aob(name, pattern, true)
            .into_iter()
            .next()
            .map(|a| a as usize))
    }

    fn scan_module_all(&self, name: &str, pattern: &Pattern) -> Result<Vec<usize>> {
        Ok(scan::aob(name, pattern, false)
            .into_iter()
            .map(|a| a as usize)
            .collect())
    }

    fn scan_heap_for_u64_labeled(
        &self,
        needle: u64,
        alignment: usize,
        label: &str,
    ) -> Result<Vec<usize>> {
        Ok(scan::heap_for_u64(needle, alignment as u32, label)
            .into_iter()
            .map(|a| a as usize)
            .collect())
    }
}
