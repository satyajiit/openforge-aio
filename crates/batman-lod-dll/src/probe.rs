//! Game-agnostic `FName::ToString` locator.
//!
//! Given a candidate-RVA slice (from the active [`crate::lotdk::BuildConfig`]
//! or any future build's config), call each candidate under Windows SEH and
//! return the first that decodes `FName{ci=0, number=0}` to `"None"` — the
//! structural invariant every UE5 `FName::ToString` upholds.
//!
//! Crash safety: every call runs entirely inside the C SEH shim — the call,
//! the FString validation, AND the wide-char read are all under one `__try`.
//! A wrong candidate cannot escape via a garbage FString.data pointer.

use crate::fname_repr::FNameRepr;
use crate::seh::seh_call_fname_to_string;

/// Probe each candidate RVA in order. Returns the absolute address of the
/// first candidate that produces `"None"` for `FName{ci=0, number=0}`.
///
/// Inputs:
///   - `module_base`: live base address of the game module.
///   - `candidates`: RVAs to test (from `crate::locate::resolve_fname_to_string`,
///     which derives them by scanning `.text` for `BuildConfig.fname_to_string_sigs`).
pub fn find_fname_to_string(module_base: usize, candidates: &[usize]) -> Option<usize> {
    crate::flog!(
        "INFO",
        "probe: testing {} candidates (module_base=0x{module_base:X})",
        candidates.len()
    );

    let fname_none = FNameRepr {
        comparison_index: 0,
        number: 0,
    };

    for (i, rva) in candidates.iter().enumerate() {
        let addr = module_base + rva;
        crate::flog!(
            "INFO",
            "probe[{i}]: calling 0x{addr:X} (RVA 0x{rva:X}) under SEH"
        );

        match seh_call_fname_to_string(addr, &fname_none) {
            Some(s) if s == "None" => {
                crate::flog!(
                    "INFO",
                    "probe[{i}]: 0x{addr:X} -> \"None\" — MATCH (FName::ToString)"
                );
                return Some(addr);
            }
            Some(s) => {
                crate::flog!(
                    "INFO",
                    "probe[{i}]: 0x{addr:X} -> \"{s}\" (not None) — reject"
                );
            }
            None => {
                crate::flog!(
                    "INFO",
                    "probe[{i}]: 0x{addr:X} -> SEH exception or implausible FString — reject"
                );
            }
        }
    }

    crate::flog!(
        "ERROR",
        "probe: no candidate returned \"None\"; FName::ToString not found"
    );
    None
}
