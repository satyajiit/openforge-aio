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
//! The trainer is responsible for every step of finding game internals.
//! `fname_to_string_candidates` is the fast path: known-correct RVAs from a
//! verified build. A future `fname_to_string_aob` field will hold a
//! signature the in-DLL AOB scanner falls back to when the candidate RVAs
//! all miss (typically because the game shipped a patch that shifted
//! offsets). Users never need to drop UE4SS, run external tools, or edit
//! constants. If both paths fail, the host surfaces a "trainer needs an
//! update" toast; the fix ships in the next release.
//!
//! All addresses are RVAs (module-relative offsets); the worker adds the
//! live module base at runtime to obtain absolute addresses.

/// All compile-time inputs the DLL needs to attach to a specific UE5 build.
///
/// The struct is intentionally `Copy + Clone` and holds only `usize` slices
/// so it can be passed through the worker without lifetime juggling.
#[derive(Clone, Copy)]
pub struct BuildConfig {
    /// Human-readable build identifier, surfaced in logs.
    pub game_id: &'static str,
    /// Candidate RVAs for `FName::ToString`. The probe tests them in order
    /// under SEH and accepts the first that decodes `FName{ci=0, number=0}`
    /// to `"None"`. Multi-entry lists support multiple build hashes living
    /// under one DLL; LotDK currently ships a single entry.
    pub fname_to_string_candidates: &'static [usize],
    /// RVA of the `FChunkedFixedUObjectArray` inner struct (Objects** at +0,
    /// NumElements at +20, NumChunks at +28). UE4SS reports the outer
    /// `FUObjectArray` base at RVA `0x0B65C490`; the inner array lives at
    /// `outer + 0x10`, hence the `+ 0x10` here vs the UE4SS log line.
    pub guobject_array_rva: usize,
    /// RVA of the engine's `UObject::ProcessEvent` function. Called as
    /// `void ProcessEvent(UObject* Context, UFunction* Function, void* Parms)`
    /// — the universal UFunction dispatcher used by `Request::CallUFunction`.
    ///
    /// Source: UE4SS startup line `ProcessEvent address 0x1414ab884` —
    /// minus the module base (0x140000000) gives the RVA.
    pub process_event_rva: usize,
}

/// The active configuration baked into this DLL.
///
/// **Verified against UE4SS.log** (`references/ue4ss/UE4SS.log`):
/// ```text
/// [PS] Found FName::ToString: 0x141138230
/// [PS] Found GUObjectArray:   0x14b65c490
/// ```
/// With module base `0x140000000`:
///   - `fname_to_string` RVA = `0x01138230`
///   - `guobject_array` outer struct RVA = `0x0B65C490`
///   - `guobject_array` inner array RVA = `0x0B65C4A0` (outer + 0x10)
///
/// LotDK's exe has ASLR effectively off across launches; the offsets are
/// stable until the game ships a patched binary.
pub const ACTIVE: BuildConfig = BuildConfig {
    game_id: "batman-lod",
    fname_to_string_candidates: &[0x0113_8230],
    guobject_array_rva: 0x0B65_C4A0,
    process_event_rva: 0x014A_B884,
};
