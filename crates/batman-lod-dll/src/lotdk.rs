//! LEGO Batman: Legacy of the Dark Knight — game-specific build configuration.
//!
//! # AIO scaling contract
//!
//! OpenForge is an All-In-One trainer. Every UE5 game we support gets its
//! own DLL crate, and every such DLL has exactly one file shaped like this
//! one — a single `BuildConfig` constant that the worker reads at attach.
//! No other module in the DLL is allowed to carry game-specific constants.
//!
//! To add a new game:
//! 1. Copy `crates/_template-dll/` (game-specific template).
//! 2. Edit only its `<game>.rs` config module (the analogue of this file).
//! 3. The probe + engine + reflection ops are inherited unchanged.
//!
//! # Auto-discovery (no manual user steps, ever)
//!
//! The trainer is responsible for every step of finding game internals, with
//! NO baked function addresses (a game patch relinks every RVA). Engine
//! globals are resolved structurally at attach: `FName::ToString` by `.text`
//! prologue signature + behavioral validation, `GUObjectArray` by structural
//! fingerprint, `ProcessEvent` by UObject-vtable universal-slot + prologue
//! match. Users never need to drop UE4SS, run external tools, or edit
//! constants. If a resolver fails (e.g. the game recompiled a prologue), the
//! host surfaces a "trainer needs an update" toast; the fix ships in the next
//! release as an updated signature.

/// All compile-time inputs the DLL needs to attach to a specific UE5 build.
///
/// The struct is intentionally `Copy + Clone` and holds only `usize` slices
/// so it can be passed through the worker without lifetime juggling.
#[derive(Clone, Copy)]
pub struct BuildConfig {
    /// Human-readable build identifier, surfaced in logs.
    pub game_id: &'static str,
    /// Prologue AOB signature(s) for `FName::ToString` — NOT addresses.
    /// `crate::locate::resolve_fname_to_string` scans the main module's
    /// `.text` for each, then behaviorally validates every hit by calling it
    /// with `FName{ci=0, number=0}` under SEH and requiring `"None"` (the UE5
    /// invariant). A signature survives relinks (a game patch relocates the
    /// function but emits identical bytes); the behavioral check guarantees
    /// correctness even if a signature matches more than one site. No baked
    /// absolute address, so it can't go stale on an update like a raw RVA.
    pub fname_to_string_sigs: &'static [&'static str],
    /// RVA of the `FChunkedFixedUObjectArray` inner struct (Objects** at +0,
    /// NumElements at +20, NumChunks at +28). UE4SS reports the outer
    /// `FUObjectArray` base; the inner array lives at `outer + 0x10`.
    ///
    /// This is only a FAST-PATH HINT. `crate::locate::resolve_guobject_array`
    /// validates it structurally and, if a game update / different store binary
    /// shifted it, re-discovers the array by its fingerprint. A wrong value
    /// here just costs one in-process scan — it never breaks reflection.
    pub guobject_array_rva: usize,
    /// Prologue AOB signature for the engine's `UObject::ProcessEvent`
    /// (`void ProcessEvent(UObject* Context, UFunction* Function, void* Parms)`)
    /// — the universal UFunction dispatcher used by `Request::CallUFunction`.
    ///
    /// NOT an address. The bare prologue is generic (~30 hits across `.text`),
    /// so `crate::locate::resolve_process_event` does NOT scan `.text` for it.
    /// Instead it walks the UObject vtable from a handful of distinct-class
    /// live objects, finds the universal slot whose target matches this
    /// prologue, and returns that pointer. No baked address AND no baked vtable
    /// index — it survives both relinks and vtable-layout shifts.
    pub process_event_sig: &'static str,
}

/// The active configuration baked into this DLL.
///
/// **No baked function addresses.** A game patch relinks the binary and shifts
/// every RVA (the 2026 patch moved `GUObjectArray` +0x4000 and `ProcessEvent`
/// +0x50, which is what broke the hardcoded-RVA build). So all three engine
/// globals are now resolved structurally at attach:
///   - `FName::ToString` — `.text` signature scan + behavioral `"None"` check.
///   - `GUObjectArray`    — `FChunkedFixedUObjectArray` fingerprint (the
///     `guobject_array_rva` below is only a fast-path hint, validated then
///     re-discovered on mismatch).
///   - `ProcessEvent`     — UObject-vtable universal-slot + prologue match.
///
/// The signatures below were extracted from the live build's prologues (see
/// `crates/discover/re/dump_rva.py` / `scan_aob.py` / `rpm_vtable.py`); the
/// FName one is unique in `.text`, and the ProcessEvent one matched the lone
/// universal vtable slot (index 76 on this build) — but neither encodes a
/// location, so they survive relinks.
pub const ACTIVE: BuildConfig = BuildConfig {
    game_id: "batman-lod",
    // FName::ToString prologue: callee-saved spills + `sub rsp,0x850` + stack
    // canary (`48 8B 05 ?? ?? ?? ??` RIP-rel is wildcarded) + the `Number==0`
    // fast-path compare. 53 bytes — unique in `.text` on this build.
    fname_to_string_sigs: &[
        "48 89 5C 24 18 48 89 6C 24 20 56 57 41 55 41 56 41 57 48 81 EC 50 08 00 00 \
         48 8B 05 ?? ?? ?? ?? 48 33 C4 48 89 84 24 40 08 00 00 45 33 ED 48 8B FA 44 39 69 04",
    ],
    guobject_array_rva: 0x0B66_04A0,
    // ProcessEvent prologue: `push rbp/rsi/rdi/r12-r15; sub rsp,imm32;
    // lea rbp,[rsp+imm8]; mov [rbp+imm32],rbx`. Generic on its own — only the
    // vtable cross-check pins it to ProcessEvent.
    process_event_sig: "40 55 56 57 41 54 41 55 41 56 41 57 48 81 EC ?? ?? ?? ?? 48 8D 6C 24 ?? 48 89 9D",
};
