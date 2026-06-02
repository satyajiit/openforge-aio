//! Resolution + caching of the Glacier engine actuation functions.
//!
//! ZCL logic nodes are driven by standalone engine free-functions (not vtable
//! slots), located by an AOB over the main module's `.text` — the same
//! `PATTERN_HOOK` discipline ZHMModSDK uses on Hitman WoA. We only *read*
//! `.text` to find an address and then *call* it via a normal indirect call
//! (SEH-guarded); we never patch the image, so Denuvo's anti-tamper has
//! nothing to trip on.
//!
//! ABI (MS x64): `bool fn(ZEntityRef /*RCX = entity_va + 8*/,
//! u32 /*EDX = pin/property id*/, const ZObjectRef& /*R8*/ [, bool /*R9*/])`.

use openforge_core::Pattern;
use parking_lot::Mutex;

use crate::scan;

/// `SignalInputPin(ZEntityRef, u32 pinId, const ZObjectRef&)` prologue.
///
/// The trailing `48 8B 29` (`mov rbp,[rcx]`) is the tell: RCX is the
/// `ZEntityRef` the function immediately dereferences. The wildcard at index 13
/// is the `sub rsp, imm8` stack-alloc size. Verified UNIQUE in
/// `007FirstLight.exe` `.text` (VA `0x14C767890` on the dump build); we still
/// resolve dynamically so a game patch that shifts the address is handled.
const SIGNAL_INPUT_PIN_BYTES: [u8; 17] = [
    0x48, 0x89, 0x6C, 0x24, 0x20, 0x56, 0x41, 0x56, 0x41, 0x57, 0x48, 0x83, 0xEC, 0x00, 0x48, 0x8B,
    0x29,
];
const SIGNAL_INPUT_PIN_MASK: [u8; 17] = [
    0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0xFF, 0xFF,
    0xFF,
];

/// The literal `Activate` input-pin id from `Pins.h`. Glacier pin ids are
/// IOI's proprietary signed-i32 string hash — **not** CRC32 — so this is a
/// harvested constant, not something we can recompute. Most ZCL nodes fire on
/// this pin.
#[allow(dead_code)] // harvested pin-id constant; retained for future pin-based actuation
pub const PIN_ACTIVATE: u32 = 0x4F10_66FB;

static SIGNAL_INPUT_PIN_VA: Mutex<Option<u64>> = Mutex::new(None);

/// Resolve `SignalInputPin`'s absolute VA (cached for the life of the DLL).
///
/// Returns `None` if the pattern is absent OR not unique in `.text` — we
/// refuse an ambiguous match rather than risk firing into the wrong function.
pub fn signal_input_pin() -> Option<u64> {
    let mut slot = SIGNAL_INPUT_PIN_VA.lock();
    if let Some(va) = *slot {
        return Some(va);
    }
    let va = resolve_unique(
        &SIGNAL_INPUT_PIN_BYTES,
        &SIGNAL_INPUT_PIN_MASK,
        "SignalInputPin",
    )?;
    *slot = Some(va);
    Some(va)
}

/// Scan the main module's `.text` for `(bytes, mask)` and return the address
/// iff exactly one match exists. Logs the count for any other outcome.
fn resolve_unique(bytes: &[u8], mask: &[u8], name: &str) -> Option<u64> {
    let pat = match Pattern::from_wire(bytes, mask) {
        Ok(p) => p,
        Err(e) => {
            crate::flog!("ERROR", "engine: bad {name} pattern: {e}");
            return None;
        }
    };
    let hits = scan::aob("", &pat, false);
    match hits.len() {
        1 => {
            let va = hits[0];
            crate::flog!("INFO", "engine: resolved {name} @ 0x{va:X}");
            Some(va)
        }
        0 => {
            crate::flog!("WARN", "engine: {name}: 0 matches in .text");
            None
        }
        n => {
            crate::flog!(
                "WARN",
                "engine: {name}: {n} matches (ambiguous) — refusing to pick"
            );
            None
        }
    }
}
