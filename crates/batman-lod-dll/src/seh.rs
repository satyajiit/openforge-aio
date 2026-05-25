//! Rust bindings to the C SEH (Structured Exception Handling) shim.
//!
//! Calls into engine code at addresses we *think* are `FName::ToString` may
//! land on the wrong function or non-function bytes. Without SEH, such a
//! call corrupts the game process via an unhandled access violation. The C
//! shim wraps the call AND the subsequent read of the FString out-param in
//! `__try`/`__except`; the whole call+validate+read sequence runs under one
//! protected frame so a wrong-but-non-crashing function can't escape via
//! garbage data pointers.
//!
//! Returns:
//!   - `Some(decoded_string)` on success
//!   - `None` on any structured exception or implausible FString
//!
//! Note: `Some(s)` does NOT mean the function was actually `FName::ToString`.
//! The caller must check whether `s == "None"` (UE5's invariant for
//! `FName{ci=0, number=0}`) to confirm.

use crate::fname_repr::{FNameRepr, ToStringFn};

const OUT_BUF_CAP: usize = 64;

/// FFI type matching UE5's `void UObject::ProcessEvent(UFunction*, void*)`.
/// On x64 Windows, the implicit `this` is the first arg (rcx) — matching
/// our `extern "system"` signature with three pointer params.
pub(crate) type ProcessEventFn = unsafe extern "system" fn(
    *mut core::ffi::c_void,
    *mut core::ffi::c_void,
    *mut core::ffi::c_void,
);

unsafe extern "C" {
    /// Defined in `src/seh.c`. Returns the number of wide chars written
    /// (excluding trailing NUL) on success, -1 on SEH exception or implausible
    /// FString. The whole call + buffer read is done inside `__try`.
    fn openforge_seh_call_fname_to_string(
        fn_ptr: ToStringFn,
        fname: *const FNameRepr,
        out_chars: *mut u16,
        out_chars_cap: i32,
    ) -> i32;

    /// Defined in `src/seh.c`. Wraps a single ProcessEvent call in `__try`.
    /// Returns 0 on success, -1 if the engine call took a structured
    /// exception (typically: stale `obj` pointer after a world transition,
    /// wild `func` pointer, or an engine-internal assert).
    fn openforge_seh_call_process_event(
        fn_ptr: ProcessEventFn,
        obj: *mut core::ffi::c_void,
        func: *mut core::ffi::c_void,
        parms: *mut core::ffi::c_void,
    ) -> i32;
}

/// Call a candidate `FName::ToString` function pointer under SEH protection.
/// The shim does the call AND reads the resulting FString buffer; all inside
/// one `__try`. Anything that goes wrong (AV during call, garbage data
/// pointer, implausible length) becomes `None`.
///
/// # Safety
///
/// `fn_addr` must be a code address in the current process. The shim survives
/// it not actually being `FName::ToString`.
pub(crate) fn seh_call_fname_to_string(fn_addr: usize, fname: &FNameRepr) -> Option<String> {
    let fn_ptr: ToStringFn = unsafe { core::mem::transmute(fn_addr) };
    let mut buf = [0u16; OUT_BUF_CAP];
    let rc = unsafe {
        openforge_seh_call_fname_to_string(
            fn_ptr,
            fname as *const _,
            buf.as_mut_ptr(),
            OUT_BUF_CAP as i32,
        )
    };
    if rc < 0 {
        return None;
    }
    let take = (rc as usize).min(OUT_BUF_CAP);
    Some(String::from_utf16_lossy(&buf[..take]))
}

/// Call `UObject::ProcessEvent(obj, func, parms)` under SEH protection.
/// The C shim wraps the call in `__try`; an access violation (stale
/// `obj`/`func`, partial world-tear-down state, etc.) returns `false`
/// instead of crashing the game process.
///
/// `parms` is a caller-owned writable buffer sized to the UFunction's
/// `ParmsSize`. The engine reads inputs from it and writes outputs back
/// in place. On SEH failure, the buffer may be partially mutated — callers
/// either ignore it (the current contract) or treat partial bytes as
/// garbage.
///
/// # Safety
///
/// `fn_addr` MUST be a valid code address — typically `engine.process_event`,
/// which is itself validated against the build's known RVA. The shim
/// protects against `obj`/`func`/`parms` being garbage, but not against
/// `fn_addr` itself being invalid.
pub(crate) fn seh_call_process_event(
    fn_addr: usize,
    obj: usize,
    func: usize,
    parms: *mut u8,
) -> bool {
    if fn_addr == 0 {
        return false;
    }
    let fn_ptr: ProcessEventFn = unsafe { core::mem::transmute(fn_addr) };
    let rc = unsafe {
        openforge_seh_call_process_event(fn_ptr, obj as *mut _, func as *mut _, parms as *mut _)
    };
    rc == 0
}
