//! Rust binding to the C SEH shim (`seh.c`) for guarded indirect calls into
//! Glacier engine functions.
//!
//! Calling an AOB-resolved engine function against a live (possibly stale or
//! mistyped) entity can fault; without SEH that fault is an unrecoverable
//! access violation in the game process. The C shim wraps the call in
//! `__try`/`__except`, so a fault becomes `None` here instead of a crash.

// One generic guarded thunk; see `seh.c`. `a0..a3` land in RCX/RDX/R8/R9.
unsafe extern "C" {
    fn openforge_seh_call4(
        fn_ptr: *const core::ffi::c_void,
        a0: u64,
        a1: u64,
        a2: u64,
        a3: u64,
        out_ret: *mut u64,
    ) -> i32;
}

/// Call the engine function at `fn_addr` with up to four integer/pointer args
/// (MS x64: `a0→RCX, a1→RDX, a2→R8, a3→R9`), under SEH protection. Returns the
/// raw `RAX` on success, or `None` if `fn_addr` is null or the call took a
/// structured exception (access violation, illegal instruction, …).
///
/// A `bool`-returning engine function leaves its result in `AL`; read the low
/// byte of the returned `u64` (`ret & 0xFF`).
///
/// # Safety
///
/// `fn_addr` must be a code address in the current process. The shim survives
/// it being the wrong function (an AV is caught), but cannot undo non-faulting
/// side effects a "successful" call into the wrong code might cause — callers
/// must resolve `fn_addr` from a unique AOB and validate the target entity.
pub fn seh_call4(fn_addr: u64, a0: u64, a1: u64, a2: u64, a3: u64) -> Option<u64> {
    if fn_addr == 0 {
        return None;
    }
    let mut out: u64 = 0;
    // SAFETY: `fn_addr` is non-null; the C shim wraps the call in __try/__except
    // so any structured exception returns rc != 0 rather than unwinding here.
    let rc = unsafe {
        openforge_seh_call4(
            fn_addr as *const core::ffi::c_void,
            a0,
            a1,
            a2,
            a3,
            &mut out as *mut u64,
        )
    };
    if rc == 0 { Some(out) } else { None }
}
