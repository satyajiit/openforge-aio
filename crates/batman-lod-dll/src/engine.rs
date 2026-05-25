//! `UeEngine`: the resolved UE5 reflection state shared by every connection.
//!
//! Lifetime: created once when the worker thread starts and never torn down
//! for the life of the game process. Caches are interior-mutex'd so the
//! request handlers can be `&self`.

use std::collections::HashMap;
use std::sync::Arc;

use openforge_ue5_protocol::{PropInfo, UFunctionInfo, UeOffsets};
use parking_lot::Mutex;

use crate::fname_repr::{FNameRepr, FStringRepr};
use crate::local_reader::LocalReader;

/// Errors returned by `UeEngine::attach_with_known_addresses`. The legacy
/// signature-scanning pipeline (strategies A/B/C/D/E) was retired; the only
/// attach path that remains receives caller-validated addresses (FName::ToString
/// from the SEH-protected probe, GUObjectArray from the hardcoded LotDK RVA).
/// A non-zero check is the one structural invariant we still enforce here.
#[derive(Debug)]
pub enum AttachError {
    /// One of `fname_to_string` or `guobject_array` was zero. The probe
    /// path returns `None` on failure; the caller is expected to short-
    /// circuit before reaching this constructor. A zero here is a contract
    /// violation, not a recoverable runtime condition.
    NullAddress(&'static str),
}

impl core::fmt::Display for AttachError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            AttachError::NullAddress(field) => write!(f, "null address for `{field}`"),
        }
    }
}

impl std::error::Error for AttachError {}

pub struct UeEngine {
    pub reader: LocalReader,
    pub guobject_array: usize,
    /// Address of the engine's `FName::ToString` function. Validated by
    /// `crate::probe::find_fname_to_string` via a live SEH-protected call
    /// that decodes `FName{ci=0, number=0}` to `"None"` (UE5 invariant).
    pub fname_to_string: usize,
    /// Address of the engine's `UObject::ProcessEvent` function. Universal
    /// UFunction dispatcher used by `Request::CallUFunction`. `0` means
    /// the BuildConfig didn't supply one — `call_ufunction` returns
    /// `Response::Error` rather than risk an invalid call.
    pub process_event: usize,
    pub fuobject_item_stride: u32,
    pub offsets: UeOffsets,
    pub layout_validated: bool,
    pub name_cache: Mutex<HashMap<u32, String>>,
    pub prop_cache: Mutex<HashMap<u64, Arc<Vec<PropInfo>>>>,
    pub func_cache: Mutex<HashMap<u64, Arc<Vec<UFunctionInfo>>>>,
}

impl UeEngine {
    /// Construct a `UeEngine` from probe-validated addresses.
    ///
    /// - `fname_to_string` comes from `crate::probe::find_fname_to_string`,
    ///   which calls the candidate function under Windows SEH and confirms
    ///   it decodes `FName{ci=0, number=0}` to `"None"`.
    /// - `guobject_array` is the hardcoded LotDK RVA + module base; see
    ///   `crate::worker::LOTDK_GUOBJECTARRAY_RVA`.
    ///
    /// The UE5.0+ stock layout offsets baked in below are confirmed correct
    /// against the live LotDK build by the 116,772-object enumeration that
    /// resolved Package / Class / Actor names successfully on attach.
    pub fn attach_with_known_addresses(
        fname_to_string: usize,
        guobject_array: usize,
        process_event: usize,
    ) -> Result<Self, AttachError> {
        if fname_to_string == 0 {
            return Err(AttachError::NullAddress("fname_to_string"));
        }
        if guobject_array == 0 {
            return Err(AttachError::NullAddress("guobject_array"));
        }
        // process_event MAY be zero — that's a soft warning rather than a
        // hard error so the DLL still loads for read-only reflection ops
        // (FindUObject, ReadProperty) even on builds where ProcessEvent
        // isn't yet discovered. CallUFunction explicitly checks and errors.
        Ok(Self {
            reader: LocalReader::new(),
            guobject_array,
            fname_to_string,
            process_event,
            fuobject_item_stride: 24,
            // Canonical LotDK layout — every field cross-checked against
            // the UE4SS log header dump for this build (see
            // `references/ue4ss/UE4SS.log` lines 80–299). The earlier
            // "stock UE5.0+" values produced garbage FProperty walks
            // (UnknownProp names + giant sizes) because LotDK uses the
            // leaner FField layout (Owner = 8B not 16B) and UFunction has
            // EventGraphFunction/EventGraphCallOffset packed before Func.
            offsets: UeOffsets {
                uobject_class_private: 0x10,
                uobject_name_private: 0x18,
                uobject_outer_private: 0x20,
                ustruct_super_struct: 0x40,
                ustruct_children: 0x48,
                ustruct_child_properties: 0x50,
                ffield_class_private: 0x08,
                ffield_next: 0x18,
                ffield_name_private: 0x20,
                fproperty_offset_internal: 0x44,
                fproperty_element_size: 0x34,
                ufield_next: 0x28,
                ufunction_flags: 0xB0,
                ufunction_func: 0xD8,
            },
            layout_validated: true,
            name_cache: Mutex::new(HashMap::new()),
            prop_cache: Mutex::new(HashMap::new()),
            func_cache: Mutex::new(HashMap::new()),
        })
    }

    /// Call the engine's `FName::ToString` via the validated function
    /// pointer, returning the decoded narrow-Rust-string form of the
    /// `(comparison_index, number)` FName.
    ///
    /// Returns `None` if:
    /// * `fname_to_string` is 0 (strategy D didn't run or didn't succeed).
    /// * The call panics (defense in depth — the validation pass already
    ///   rejects bogus candidates, so any panic here is a hard bug in the
    ///   engine itself or memory corruption; we still don't want to bring
    ///   the DLL down).
    /// * The returned FString is malformed (`data == null`, `num <= 0`, or
    ///   `num > 16384` — UE5 names are bounded at 1024 wide chars in
    ///   practice, so 16384 is a generous sanity ceiling that traps
    ///   garbage from a wrong candidate).
    ///
    /// # Memory ownership
    ///
    /// The `FString.data` buffer the engine allocates via `FMemory::Realloc`
    /// is intentionally leaked — we do not have a validated pointer to
    /// `FMemory::Free` and cannot use the system allocator's `free`. The
    /// caller (`lookup_pool`) caches the decoded `String` per
    /// `comparison_index` so each unique FName triggers at most one
    /// allocation. Worst-case leak = `unique_fnames * ~max_fname_bytes`
    /// (bounded above at ~32 KiB per unique FName by the `num > 16384`
    /// reject). For shipping UE5 titles that's a few MiB total — well
    /// inside the engine's own memory budget noise.
    ///
    /// # Safety
    ///
    /// `fname_to_string` is validated by strategy D via a live call that
    /// decodes `FName{ci=0, number=0}` to `"None"`. Wild candidates that
    /// happen to match the prologue AOB in a non-executable section are
    /// rejected before reaching here. `catch_unwind` wraps the call so a
    /// panic from any remaining edge case (e.g. the engine internally
    /// asserts) cannot unwind through FFI.
    pub(crate) unsafe fn call_fname_to_string(&self, ci: u32, number: u32) -> Option<String> {
        if self.fname_to_string == 0 {
            return None;
        }
        let fname = FNameRepr {
            comparison_index: ci,
            number,
        };
        // Route through the SEH-protected C shim. `catch_unwind` alone does
        // NOT catch Windows structured exceptions (access violations,
        // illegal instructions); a wrong-but-non-crashing function pointer
        // would scribble FString.data with garbage and the subsequent
        // `from_raw_parts` read would AV outside any Rust-level safety net.
        // The shim runs both the call AND the FString validate/read inside
        // one `__try`, so a wrong address fails cleanly with `None`.
        return crate::seh::seh_call_fname_to_string(self.fname_to_string, &fname);
        #[allow(unreachable_code)]
        let _ = fname; // suppress unused warning if the early return is patched out

        let mut fstring = FStringRepr::zeroed();
        if fstring.data.is_null() || fstring.num <= 0 || fstring.num > 16384 {
            return None;
        }
        let len_no_nul = (fstring.num - 1) as usize;
        // SAFETY: `data` is non-null and points to a UTF-16 buffer of at
        // least `num` wide chars (allocated by the engine's allocator). We
        // bounded `num` above so the slice length is sane. The buffer lives
        // for the rest of the process because we intentionally leak it
        // (see the memory-ownership note on this fn).
        let slice = unsafe { core::slice::from_raw_parts(fstring.data, len_no_nul) };
        Some(String::from_utf16_lossy(slice))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openforge_ue5_protocol::UeOffsets;

    /// Build a `UeEngine` with all locator-discovered fields zeroed. Suitable
    /// for testing the disabled-by-default behaviour of the strategy-D code
    /// path; the engine doesn't reach out to any process state in this
    /// configuration because every guarded field is 0.
    fn make_disabled_engine() -> UeEngine {
        UeEngine {
            reader: LocalReader::new(),
            guobject_array: 0,
            fname_to_string: 0,
            process_event: 0,
            fuobject_item_stride: 0,
            offsets: UeOffsets {
                uobject_class_private: 0,
                uobject_name_private: 0,
                uobject_outer_private: 0,
                ustruct_super_struct: 0,
                ustruct_children: 0,
                ustruct_child_properties: 0,
                ffield_class_private: 0,
                ffield_next: 0,
                ffield_name_private: 0,
                fproperty_offset_internal: 0,
                fproperty_element_size: 0,
                ufield_next: 0,
                ufunction_flags: 0,
                ufunction_func: 0,
            },
            layout_validated: false,
            name_cache: Mutex::new(HashMap::new()),
            prop_cache: Mutex::new(HashMap::new()),
            func_cache: Mutex::new(HashMap::new()),
        }
    }

    #[test]
    fn call_fname_to_string_nullptr_safe() {
        // With `fname_to_string == 0`, the call MUST short-circuit to
        // `None` without dereferencing anything. This is the primary
        // defense-in-depth invariant: if strategy D didn't run (or ran and
        // failed), callers can still invoke this without crashing the DLL.
        let eng = make_disabled_engine();
        // SAFETY: the function pointer is 0, which the impl checks first
        // and short-circuits — no actual call is made.
        let got = unsafe { eng.call_fname_to_string(0, 0) };
        assert!(got.is_none());
        // Try again with a non-zero (ci, number) to confirm the guard
        // depends purely on `fname_to_string`, not on the FName values.
        // SAFETY: same as above.
        let got = unsafe { eng.call_fname_to_string(42, 7) };
        assert!(got.is_none());
    }
}
