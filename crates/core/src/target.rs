//! `Target`: an attached game process backed by a raw `ProcessHandle`.
//! Implements [`crate::ctx::Ctx`] via direct `ReadProcessMemory` /
//! `WriteProcessMemory` calls.
//!
//! **Status**: the trainer (`openforge-app`) uses
//! [`openforge_ue5_host::Ue5Session`] as its `Ctx` impl, NOT this type.
//! Every trainer read / write / scan / patch crosses the pipe to a per-game
//! DLL. `Target` remains for the `openforge-discover` dev CLI, which scans
//! arbitrary processes from the host side without a DLL injected.

use std::sync::Arc;

use crate::ctx::Ctx;
use crate::error::{Error, Result};
use crate::memory;
use crate::module::{self, Module};
use crate::pattern::Pattern;
use crate::process::{ProcessHandle, ProcessSnapshot};

/// Callback signature for heap-scan progress reporting.
///
/// Args: `(label, current_bytes, total_bytes)`. The label identifies which
/// feature scan is in flight (typically the feature id). Implementations
/// should be cheap — the callback fires roughly every 16 MB of scanned data.
pub type ProgressFn = Arc<dyn Fn(&str, u64, u64) + Send + Sync>;

pub struct Target {
    pub pid: u32,
    pub process_name: String,
    handle: ProcessHandle,
    modules: Vec<Module>,
    main_module_idx: usize,
    progress: Option<ProgressFn>,
}

impl Target {
    /// Attach by trying each candidate executable name (case-insensitive),
    /// returning the first match.
    pub fn attach_by_candidates(candidates: &[&str]) -> Result<Self> {
        let snap = ProcessSnapshot::capture()?;
        let (pid, name) = snap
            .find_by_candidates(candidates)
            .map(|i| (i.pid, i.name.clone()))
            .ok_or_else(|| Error::ProcessNotFound(candidates.join(" | ")))?;
        Self::attach_by_pid(pid, &name)
    }

    pub fn attach_by_pid(pid: u32, expected_name: &str) -> Result<Self> {
        let handle = ProcessHandle::open(pid)?;
        let modules = module::enumerate(pid, &handle)?;
        let main_module_idx = modules
            .iter()
            .position(|m| m.name.eq_ignore_ascii_case(expected_name))
            .or_else(|| {
                let needle = expected_name.to_ascii_lowercase();
                modules
                    .iter()
                    .position(|m| m.name.to_ascii_lowercase().contains(&needle))
            })
            .unwrap_or(0);
        Ok(Self {
            pid,
            process_name: expected_name.to_string(),
            handle,
            modules,
            main_module_idx,
            progress: None,
        })
    }

    pub fn modules(&self) -> &[Module] {
        &self.modules
    }

    pub fn main(&self) -> &Module {
        &self.modules[self.main_module_idx]
    }

    /// Enumerate all committed, non-executable, non-guard RW regions in the
    /// target's address space. Exposed for heap-shape scanners (e.g. the UE5
    /// FNamePool last-resort locator) that need to inspect bytes outside the
    /// main module image.
    pub fn rw_regions(&self) -> Vec<memory::MemoryRegion> {
        memory::enumerate_rw_regions(&self.handle)
    }

    /// Install a progress callback. Must be called BEFORE the `Target` is
    /// wrapped in `Arc` (the field is set on the owned value). Once installed,
    /// every heap-scan reports through this callback.
    pub fn set_progress(&mut self, f: ProgressFn) {
        self.progress = Some(f);
    }

    fn resolve_module(&self, name: &str) -> Option<&Module> {
        if name.is_empty() {
            Some(self.main())
        } else {
            self.modules
                .iter()
                .find(|m| m.name.eq_ignore_ascii_case(name))
        }
    }

    fn read_text(&self, m: &Module) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; m.text_size];
        memory::read_bytes(&self.handle, m.text_base(), &mut buf)?;
        Ok(buf)
    }
}

impl Ctx for Target {
    fn pid(&self) -> u32 {
        self.pid
    }

    fn main_module(&self) -> &Module {
        self.main()
    }

    fn module(&self, name: &str) -> Option<&Module> {
        self.resolve_module(name)
    }

    fn read_bytes(&self, addr: usize, buf: &mut [u8]) -> Result<()> {
        memory::read_bytes(&self.handle, addr, buf)
    }

    fn write_bytes(&self, addr: usize, buf: &[u8]) -> Result<()> {
        memory::write_bytes(&self.handle, addr, buf)
    }

    fn scan_module(&self, name: &str, pattern: &Pattern) -> Result<Option<usize>> {
        let m = self
            .resolve_module(name)
            .ok_or_else(|| Error::ModuleNotFound(name.to_string()))?;
        let bytes = self.read_text(m)?;
        Ok(pattern.scan(&bytes).map(|off| m.text_base() + off))
    }

    fn scan_module_all(&self, name: &str, pattern: &Pattern) -> Result<Vec<usize>> {
        let m = self
            .resolve_module(name)
            .ok_or_else(|| Error::ModuleNotFound(name.to_string()))?;
        let bytes = self.read_text(m)?;
        Ok(pattern
            .scan_all(&bytes)
            .into_iter()
            .map(|off| m.text_base() + off)
            .collect())
    }

    fn scan_heap_for_u64(&self, needle: u64, alignment: usize) -> Result<Vec<usize>> {
        self.scan_heap_for_u64_labeled(needle, alignment, "")
    }

    fn scan_heap_for_u64_labeled(
        &self,
        needle: u64,
        alignment: usize,
        label: &str,
    ) -> Result<Vec<usize>> {
        let progress = self.progress.clone();
        let on_progress = move |current: u64, total: u64| {
            if let Some(cb) = progress.as_ref() {
                cb(label, current, total);
            }
        };
        Ok(memory::scan_rw_for_u64(
            &self.handle,
            needle,
            alignment.max(1),
            on_progress,
        ))
    }
}
