//! DLL-side per-frame guarded freeze registry + thread (protocol v4).
//!
//! [`start_freeze`] registers a freeze slot; an independent thread
//! ([`freeze_loop`], spawned once from `DllMain` alongside the worker)
//! re-stamps every active slot ~60 times a second. The thread is INDEPENDENT of
//! the pipe — a client disconnect must not stop an in-game freeze.
//!
//! Every write is gated by a read-before-write plausibility check: the f32 at
//! the write target must be finite and within `[guard_min, guard_max]`. A
//! freed/reused box (the host's resolved address went stale after a checkpoint
//! reload) reads garbage and is SKIPPED, not corrupted — this is the
//! broad-freeze crash lesson encoded in the production path. The dynamic
//! `source_offset` mode copies a sibling field each tick (e.g. current := max),
//! so god mode is difficulty-agnostic without baking in a max value.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use parking_lot::Mutex;

use openforge_dll_common::local_reader::LocalReader;
use openforge_dll_common::panic_guard::guarded;
use openforge_glacier_protocol::{GlacierValue, Response, ValueKind};

/// ~60 Hz poll. A fixed poll thread is Denuvo-safe (no `.text` detour / frame
/// hook) and fast enough to win the write race against the game's own stores.
const TICK_MS: u64 = 16;

struct FreezeSlot {
    handle: u32,
    box_va: u64,
    write_offset: i64,
    /// When `Some`, copy `width` bytes from `box_va + this` to the write target
    /// each tick. When `None`, stamp `value_bytes`.
    source_offset: Option<i64>,
    value_bytes: Vec<u8>,
    width: usize,
    guard_min: f32,
    guard_max: f32,
    writes: AtomicU64,
    skipped: AtomicU64,
    ticks: AtomicU64,
    active: AtomicBool,
}

static FREEZE: Mutex<Vec<Arc<FreezeSlot>>> = Mutex::new(Vec::new());
static NEXT_HANDLE: AtomicU32 = AtomicU32::new(1);

/// Register a guarded freeze. Returns [`Response::FreezeStarted`].
pub fn start_freeze(
    box_va: u64,
    write_offset: i64,
    source_offset: Option<i64>,
    value: GlacierValue,
    value_kind: ValueKind,
    guard_min: f32,
    guard_max: f32,
) -> Response {
    let width = value_kind.size() as usize;
    if width == 0 || width > 8 {
        return Response::Error(format!("StartFreeze: unsupported value width {width}"));
    }
    let handle = NEXT_HANDLE.fetch_add(1, Ordering::SeqCst);
    let slot = Arc::new(FreezeSlot {
        handle,
        box_va,
        write_offset,
        source_offset,
        value_bytes: value.to_le_bytes(),
        width,
        guard_min,
        guard_max,
        writes: AtomicU64::new(0),
        skipped: AtomicU64::new(0),
        ticks: AtomicU64::new(0),
        active: AtomicBool::new(true),
    });
    FREEZE.lock().push(slot);
    Response::FreezeStarted { handle }
}

/// Mark a freeze inactive (the loop drops it on its next pass). Idempotent.
pub fn stop_freeze(handle: u32) -> Response {
    for s in FREEZE.lock().iter() {
        if s.handle == handle {
            s.active.store(false, Ordering::SeqCst);
        }
    }
    Response::FreezeStopped
}

/// Report a freeze's running counters, or `Error` for an unknown handle.
pub fn query_stats(handle: u32) -> Response {
    for s in FREEZE.lock().iter() {
        if s.handle == handle {
            return Response::FreezeStats {
                writes: s.writes.load(Ordering::SeqCst),
                skipped: s.skipped.load(Ordering::SeqCst),
                ticks: s.ticks.load(Ordering::SeqCst),
            };
        }
    }
    Response::Error(format!("unknown freeze handle {handle}"))
}

/// The freeze worker thread body. Spawned once from `DllMain`; runs until the
/// DLL's `SHUTDOWN` flag is set. Each pass is panic-guarded so a bad slot can
/// never unwind into the game.
pub fn freeze_loop() {
    crate::flog!("INFO", "freeze thread start (~{} Hz)", 1000 / TICK_MS);
    let reader = LocalReader::new();
    while !crate::SHUTDOWN.load(Ordering::SeqCst) {
        let _ = guarded(|| tick(&reader));
        std::thread::sleep(Duration::from_millis(TICK_MS));
    }
    crate::flog!("INFO", "freeze thread exit");
}

fn tick(reader: &LocalReader) {
    // Snapshot the Arc list without holding the lock across mem ops, and drop
    // slots that were stopped.
    let slots: Vec<Arc<FreezeSlot>> = {
        let mut guard = FREEZE.lock();
        guard.retain(|s| s.active.load(Ordering::SeqCst));
        guard.clone()
    };
    for s in &slots {
        s.ticks.fetch_add(1, Ordering::SeqCst);
        let target = s.box_va.wrapping_add(s.write_offset as u64) as usize;

        // Read-before-write guard: the write target must currently hold a
        // plausible f32 (the box is still alive). A freed/reused box reads
        // garbage and is skipped, not corrupted.
        let mut cur = [0u8; 4];
        if reader.read_bytes(target, &mut cur).is_err() {
            s.skipped.fetch_add(1, Ordering::SeqCst);
            continue;
        }
        let v = f32::from_le_bytes(cur);
        if !(v.is_finite() && v >= s.guard_min && v <= s.guard_max) {
            s.skipped.fetch_add(1, Ordering::SeqCst);
            continue;
        }

        // Resolve the bytes to write: a live sibling copy (current := max) or
        // the constant.
        let mut srcbuf = [0u8; 8];
        let bytes: &[u8] = match s.source_offset {
            Some(off) => {
                let src = s.box_va.wrapping_add(off as u64) as usize;
                if reader.read_bytes(src, &mut srcbuf[..s.width]).is_err() {
                    s.skipped.fetch_add(1, Ordering::SeqCst);
                    continue;
                }
                &srcbuf[..s.width]
            }
            None => &s.value_bytes,
        };

        if reader.write_bytes(target, bytes).is_ok() {
            s.writes.fetch_add(1, Ordering::SeqCst);
        } else {
            s.skipped.fetch_add(1, Ordering::SeqCst);
        }
    }
}
