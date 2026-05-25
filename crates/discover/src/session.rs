//! Discovery session state. Persisted between subcommand invocations so the
//! `scan → narrow → narrow → pick → capture → extract-aob → emit` workflow
//! survives across CLI runs.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use openforge_runtime::ValueKind;

#[cfg(windows)]
use crate::watch_write::CapturedRegisters;

/// On non-Windows builds the `watch_write` module isn't compiled, so we
/// provide a tiny stand-in here so the `Session` shape stays platform-agnostic
/// (sessions can still be loaded on Linux/macOS for inspection / `emit`).
#[cfg(not(windows))]
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct CapturedRegisters {
    pub rax: u64,
    pub rbx: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rbp: u64,
    pub rsp: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub feature: String,
    pub display_name: String,
    pub kind: ValueKind,
    pub strict: bool,
    pub pid: u32,
    pub primary_module: String,
    pub module_base: usize,
    pub module_size: usize,
    pub epsilon: f64,
    /// Parallel slices: `candidates[i]` and `last_values[i]` belong together.
    pub candidates: Vec<usize>,
    pub last_values: Vec<f64>,
    pub selected_address: Option<usize>,
    pub captured_rip: Option<usize>,
    pub captured_bytes: Option<Vec<u8>>,
    /// Rax..R15 at the moment the HW breakpoint fired. Populated by
    /// `watch-write`; consumed by pointer-chain discovery to seed candidates.
    #[serde(default)]
    pub captured_registers: Option<CapturedRegisters>,
    /// Static-anchor candidates produced by `find-pointers`.
    #[serde(default)]
    pub candidate_pointers: Option<Vec<PointerHit>>,
    /// Multi-hop chain candidates produced by `trace-chain`.
    #[serde(default)]
    pub candidate_chains: Option<Vec<ChainCandidate>>,
    pub extracted_pattern: Option<String>,
    pub extracted_resolve: Option<ExtractedResolve>,
    pub history: Vec<HistoryStep>,
}

/// One source location whose 8-byte value points *near* the target address.
///
/// `trailing_offset = target - value` is the field offset you'd add to a
/// pointer that lands inside the same object — small values (< 0x400) are
/// strong evidence that `source_addr` holds (or holds a pointer near) an
/// object containing the target field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PointerHit {
    pub source_addr: usize,
    pub value: usize,
    pub trailing_offset: usize,
    pub module_name: Option<String>,
    pub module_offset: Option<usize>,
}

/// A single hop in a static→target pointer chain. Mirrors `emit::HopSection`
/// shape so the emit step can lift this almost verbatim.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainHopSpec {
    pub deref: bool,
    pub offset: i64,
}

/// A complete static-anchored chain: a module + offset to a static slot, plus
/// a sequence of deref+offset hops that lands at the target object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainCandidate {
    pub module_name: String,
    pub module_offset: usize,
    pub hops: Vec<ChainHopSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedResolve {
    pub kind: String, // "direct" | "rip_relative" | "offset"
    pub instruction_offset: usize,
    pub operand_size: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryStep {
    pub action: String,
    pub matches_after: usize,
}

impl Session {
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            fs::read_to_string(path).with_context(|| format!("read session {}", path.display()))?;
        Ok(serde_json::from_str(&text)?)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(self)?;
        fs::write(&tmp, text)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }
}
