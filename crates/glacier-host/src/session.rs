//! `Send + Sync` facade over the Glacier DLL pipe client.
//!
//! Implements [`openforge_core::Ctx`] (so the same memory primitives features
//! already use work over the injected backend) and exposes Glacier reflection
//! as inherent methods (`resolve_type`, `instance_properties`,
//! `resolve_instance_property`, `set_property`). The UE5-specific `Ctx`
//! reflection methods (`find_uobject`, `resolve_property`, …) keep their
//! erroring defaults — Glacier features go through the inherent methods.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use openforge_core::{Ctx, Error as CoreError, Module, Pattern, Result as CoreResult, Target};
use openforge_glacier_protocol::{
    FreezeHandle, GlacierField, GlacierType, GlacierTypeProp, GlacierValue, LogLevel, NodeFire,
    NodeInput, PatternWire, ValueKind,
};
use parking_lot::Mutex;
use tracing::info;

use openforge_host_common::Injector;

use crate::Welcome;
use crate::client::GlacierClient;
use crate::error::{HostError, Result};

/// Default wall-clock budget for opening the pipe after injection.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

struct Inner {
    client: Mutex<GlacierClient>,
    welcome: Welcome,
    /// Module list fetched once at attach via `EnumModules`, stored as
    /// `openforge_core::Module` so the `Ctx` `&Module`-returning methods can
    /// hand out borrows. The DLL reports the EXE first, so `modules[0]` is the
    /// main module.
    modules: Vec<Module>,
}

/// High-level facade. Holds the pipe behind a mutex; the request/response
/// protocol is synchronous, so the mutex serialises whole exchanges.
pub struct GlacierSession {
    inner: Arc<Inner>,
}

impl GlacierSession {
    /// Resolve a candidate process, inject `dll_path` (if not already loaded),
    /// and connect the pipe.
    pub fn attach(process_candidates: &[&str], dll_path: &Path) -> Result<Self> {
        let target = Target::attach_by_candidates(process_candidates).map_err(|e| match e {
            openforge_core::Error::ProcessNotFound(s) => HostError::ProcessNotFound(s),
            other => HostError::InjectionFailed(other.to_string()),
        })?;
        Self::attach_pid(target.pid, dll_path)
    }

    /// Inject + connect for an already-resolved pid.
    pub fn attach_pid(pid: u32, dll_path: &Path) -> Result<Self> {
        info!(pid, dll = %dll_path.display(), "injecting glacier DLL");
        Injector::inject(pid, dll_path)?;
        let mut client = GlacierClient::connect(pid, DEFAULT_CONNECT_TIMEOUT)?;
        let welcome = client.welcome().clone();

        // One up-front module enumeration so the `Ctx` impl can hand out
        // `&Module` borrows without locking the client per call.
        let module_entries = client.enum_modules()?;
        let modules: Vec<Module> = module_entries
            .into_iter()
            .map(|m| Module {
                name: m.name,
                base: m.base as usize,
                size: m.size as usize,
                text_offset: m.text_offset as usize,
                text_size: m.text_size as usize,
            })
            .collect();
        if modules.is_empty() {
            return Err(HostError::InjectionFailed(
                "DLL reported zero modules — Toolhelp32 likely failed".into(),
            ));
        }

        Ok(Self {
            inner: Arc::new(Inner {
                client: Mutex::new(client),
                welcome,
                modules,
            }),
        })
    }

    pub fn welcome(&self) -> &Welcome {
        &self.inner.welcome
    }

    pub fn pid(&self) -> u32 {
        self.inner.welcome.pid
    }

    /// Main module load base, cached at attach. A base shift across re-attach
    /// means the game restarted (ASLR) and any cached addresses are stale.
    pub fn main_module_base(&self) -> u64 {
        self.inner.modules[0].base as u64
    }

    pub fn ping(&self) -> Result<()> {
        self.inner.client.lock().ping()
    }

    pub fn drain_log(&self, max_lines: u32) -> Result<Vec<String>> {
        self.inner.client.lock().drain_log(max_lines)
    }

    pub fn set_log_level(&self, level: LogLevel) -> Result<()> {
        self.inner.client.lock().set_log_level(level)
    }

    /// Find the instruction that writes `addr` (in-process HW breakpoint;
    /// Denuvo-safe). Blocks up to `timeout_ms`.
    pub fn find_writer(
        &self,
        addr: u64,
        width: u8,
        timeout_ms: u32,
    ) -> Result<crate::client::WriterHit> {
        self.inner
            .client
            .lock()
            .find_writer(addr, width, timeout_ms)
    }

    // -- Glacier reflection (inherent) ------------------------------------

    pub fn resolve_type(&self, name: &str) -> Result<Option<GlacierType>> {
        self.inner.client.lock().resolve_type(name)
    }

    pub fn enumerate_type_properties(&self, name: &str) -> Result<Option<Vec<GlacierTypeProp>>> {
        self.inner.client.lock().enumerate_type_properties(name)
    }

    pub fn instance_properties(&self, entity_va: u64) -> Result<(u64, Vec<GlacierField>)> {
        self.inner.client.lock().instance_properties(entity_va)
    }

    pub fn resolve_instance_property(
        &self,
        entity_va: u64,
        name: &str,
    ) -> Result<Option<GlacierField>> {
        self.inner
            .client
            .lock()
            .resolve_instance_property(entity_va, name)
    }

    /// `Ok(true)` written; `Ok(false)` property not present; `Err` refused/fault.
    pub fn set_property(
        &self,
        entity_va: u64,
        property: &str,
        value: GlacierValue,
    ) -> Result<bool> {
        self.inner
            .client
            .lock()
            .set_property(entity_va, property, value)
    }

    /// Heap-scan for live entities carrying `property` (CRC32 match). Returns
    /// their `ZEntityImpl` VAs — the anchor for targeting a specific entity.
    pub fn find_entities_with_property(
        &self,
        property: &str,
        max_results: u32,
    ) -> Result<Vec<u64>> {
        self.inner
            .client
            .lock()
            .find_entities_with_property(property, max_results)
    }

    /// Fire a logic node's pin (engine `SignalInputPin`) on the game process.
    /// `Ok(true)` = the engine call returned; `Err` = SEH fault / unresolved
    /// engine fn. See [`GlacierClient::fire_node`].
    pub fn fire_node(&self, node_va: u64, inputs: Vec<NodeInput>, fire: NodeFire) -> Result<bool> {
        self.inner.client.lock().fire_node(node_va, inputs, fire)
    }

    /// Call an arbitrary engine fn `fn_va(rcx,rdx,r8,r9)` ON THE GAME THREAD via
    /// the DLL's executor (HW-bp rendezvous). Returns the raw RAX. Used to drive
    /// RE'd actuation handlers (e.g. ZCLEquipItem's equip handler) with a node
    /// pointer in RCX. See [`GlacierClient::game_thread_call`].
    pub fn game_thread_call(&self, fn_va: u64, args: Vec<u64>) -> Result<u64> {
        self.inner.client.lock().game_thread_call(fn_va, args)
    }

    /// Start a DLL-side guarded per-frame freeze (protocol v4). See
    /// [`GlacierClient::start_freeze`]. Used by god mode to hold current health
    /// by copying max (`source_offset`) each tick, difficulty-agnostically.
    #[allow(clippy::too_many_arguments)]
    pub fn start_freeze(
        &self,
        box_va: u64,
        write_offset: i64,
        source_offset: Option<i64>,
        value: GlacierValue,
        value_kind: ValueKind,
        guard_min: f32,
        guard_max: f32,
    ) -> Result<FreezeHandle> {
        self.inner.client.lock().start_freeze(
            box_va,
            write_offset,
            source_offset,
            value,
            value_kind,
            guard_min,
            guard_max,
        )
    }

    /// Stop a freeze started by [`GlacierSession::start_freeze`]. Idempotent.
    pub fn stop_freeze(&self, handle: FreezeHandle) -> Result<()> {
        self.inner.client.lock().stop_freeze(handle)
    }

    /// Query a freeze's `(writes, skipped, ticks)` counters.
    pub fn query_freeze_stats(&self, handle: FreezeHandle) -> Result<(u64, u64, u64)> {
        self.inner.client.lock().query_freeze_stats(handle)
    }

    // -- Weapon give (host-composed; NO protocol op / NO DLL rebuild) -----
    //
    // Composes three shipped v6 ops — `ScanHeapForU64` + `ReadBytes` +
    // `GameThreadCall` — to (a) list every present firearm by readable name and
    // (b) grant a weapon by firing its pickup node. The algorithm is lifted
    // verbatim from the discover-CLI `run_list_firearms`/`run_give_weapon` so
    // the app and the CLI share one implementation. See the
    // `project_glacier_weapon_give_pickup_node` RE record: each firearm's pickup
    // node is `firearm-0x3B8`, pre-wired to itself; firing the
    // `ZCLTriggerPlayerItemPickup` handler `0x1415305F0` with `RCX=RDX=node` on
    // the game thread grants it. Only firearms in a grantable (dropped /
    // available) state take; the rest fault-skip harmlessly.

    /// List every present `ZFirearmCharacterEntity`, grouped by weapon type
    /// (shared `ZFirearmAudioDefinition` at `firearm+0xA8`). Pure read path
    /// (`ScanHeapForU64` + `ReadBytes`). The `display` name is the curated
    /// human-readable name when the model code is known, else the model code.
    pub fn list_firearms(&self) -> Result<Vec<FirearmType>> {
        let hits = self
            .scan_heap_for_u64_labeled(FIREARM_VT, 8, "firearm_vt")
            .map_err(|e| HostError::Server(format!("list_firearms scan: {e}")))?;
        let instances: Vec<u64> = hits
            .into_iter()
            .map(|h| h as u64)
            .filter(|&h| h >= 0x1_0000_0000)
            .collect();

        let mut groups: Vec<FirearmType> = Vec::new();
        for fw in instances {
            let audio = gl_ru64(self, fw + 0xA8).unwrap_or(0);
            let code = if audio != 0 {
                gl_read_zstr(self, audio + 0x18).unwrap_or_else(|| "<unnamed>".into())
            } else {
                "<no-audio-def>".into()
            };
            let h = gl_ru64(self, fw + 0x10).unwrap_or(0);
            let inst = FirearmInstance {
                firearm_va: fw,
                node_va: fw - 0x3B8,
                handle_idx: (h & 0xFFFF_FFFF) as u32,
                handle_gen: (h >> 32) as u32,
            };
            match groups.iter_mut().find(|g| g.audio_def_va == audio) {
                Some(g) => g.instances.push(inst),
                None => groups.push(FirearmType {
                    display: pretty_weapon_name(&code).unwrap_or(&code).to_string(),
                    code,
                    audio_def_va: audio,
                    instances: vec![inst],
                }),
            }
        }
        Ok(groups)
    }

    /// Give a weapon by model code (case-insensitive substring): scan present
    /// firearms and fire the pickup node of every match, in this one call so
    /// node VAs can't go stale. Per-firearm faults (non-grantable firearms) are
    /// swallowed. Returns `(matched, fired)`.
    pub fn give_weapon(&self, filter: &str) -> Result<(u32, u32)> {
        let needle = filter.to_lowercase();
        let hits = self
            .scan_heap_for_u64_labeled(FIREARM_VT, 8, "firearm_vt")
            .map_err(|e| HostError::Server(format!("give_weapon scan: {e}")))?;
        let instances: Vec<u64> = hits
            .into_iter()
            .map(|h| h as u64)
            .filter(|&h| h >= 0x1_0000_0000)
            .collect();

        let (mut matched, mut fired) = (0u32, 0u32);
        for fw in instances {
            let audio = match gl_ru64(self, fw + 0xA8) {
                Some(a) if a != 0 => a,
                _ => continue,
            };
            let code = gl_read_zstr(self, audio + 0x18).unwrap_or_default();
            if code.is_empty() || !code.to_lowercase().contains(&needle) {
                continue;
            }
            matched += 1;
            let node = fw - 0x3B8;
            match self.game_thread_call(PICKUP_HANDLER, vec![node, node, 0, 0]) {
                Ok(_) => {
                    fired += 1;
                    info!(firearm = format!("0x{fw:X}"), %code, "give_weapon: fired pickup node");
                }
                Err(e) => {
                    tracing::debug!(firearm = format!("0x{fw:X}"), %code, error = %e, "give_weapon: skip (non-grantable / fault)");
                }
            }
        }
        Ok((matched, fired))
    }

    /// Find every loaded humanoid's authoritative health box via the
    /// layout-agnostic invariant `base*scale == max` (with `0 < current <= max`).
    /// Anchors on the player box (the god_mode `{100,100}` + 1.0-multiplier-block
    /// fingerprint), reads a `±span` window of the health-box pool, and returns
    /// the ENEMY boxes (player excluded). Mission / area / reload-safe: re-anchors
    /// and re-scans every call. `span == 0` → default 2 MiB each side.
    pub fn scan_enemy_health_boxes(&self, span: usize) -> Result<Vec<EnemyHealthBox>> {
        let span = if span == 0 { 0x20_0000 } else { span };
        let center = self.find_player_box_va().ok_or_else(|| {
            HostError::Server("player health box not found (load into active gameplay)".into())
        })?;
        let start = center.saturating_sub(span);
        let total = span.saturating_mul(2);

        // Bulk-read the window in 64 KiB chunks; unreadable gaps stay zero and
        // are rejected by the invariant below (never a basis for a write).
        let mut buf = vec![0u8; total];
        let chunk = 0x1_0000usize;
        let mut off = 0usize;
        while off < total {
            let n = chunk.min(total - off);
            let _ = self.read_bytes(start + off, &mut buf[off..off + n]);
            off += n;
        }

        let mut boxes: Vec<EnemyHealthBox> = Vec::new();
        let mut o = 0usize;
        while o + 0x10 <= total {
            let g = |k: usize| {
                f32::from_le_bytes([buf[o + k], buf[o + k + 1], buf[o + k + 2], buf[o + k + 3]])
            };
            let (current, max, base, scale) = (g(0), g(4), g(8), g(0xC));
            if health_box_ok(current, max, base, scale) {
                let va = (start + o) as u64;
                // Exclude the player (anchor + the base~100 signature).
                if (start + o) != center && (base - 100.0).abs() > 1.0 {
                    boxes.push(EnemyHealthBox {
                        va,
                        current,
                        max,
                        base,
                        scale,
                    });
                }
            }
            o += 8;
        }
        Ok(boxes)
    }

    /// One-hit-kill: knock every loaded enemy's current health down to `value`
    /// (e.g. 1.0). Re-reads each box IMMEDIATELY before writing and re-checks the
    /// invariant, so a box freed/reused since the window snapshot is skipped — the
    /// write can only ever land on a live health box, never stray memory. Returns
    /// the number of enemy boxes set. Safe to call on a loop (re-anchors each time).
    pub fn ohk_all_enemies(&self, span: usize, value: f32) -> Result<usize> {
        let boxes = self.scan_enemy_health_boxes(span)?;
        let new_bytes = value.to_le_bytes();
        let mut written = 0usize;
        for b in boxes {
            // SAFETY re-check against live memory (the scan used a snapshot).
            let mut live = [0u8; 0x10];
            if self.read_bytes(b.va as usize, &mut live).is_err() {
                continue;
            }
            let h = |k: usize| f32::from_le_bytes([live[k], live[k + 1], live[k + 2], live[k + 3]]);
            let (c2, m2, b2, s2) = (h(0), h(4), h(8), h(0xC));
            if health_box_ok(c2, m2, b2, s2)
                && (b2 - 100.0).abs() > 1.0
                && self.write_bytes(b.va as usize, &new_bytes).is_ok()
            {
                written += 1;
            }
        }
        Ok(written)
    }

    /// Locate the player's authoritative health box (the scan arena anchor) via
    /// the god_mode fingerprint: the `{100,100}` base pair @ box+0x90, preceded by
    /// the 1.0-multiplier block, base health 100 @ +0x08. Returns the box base VA
    /// (current health @ +0x00).
    fn find_player_box_va(&self) -> Option<usize> {
        const NEEDLE: u64 = 0x42C8_0000_42C8_0000; // {100.0f, 100.0f} @ box+0x90
        let hits = self
            .scan_heap_for_u64_labeled(NEEDLE, 8, "player_box")
            .ok()?;
        for hit in hits {
            // box = hit - 0x90; read box-0x70..box+0x10 (0x80 bytes @ hit-0x100).
            let mut b = [0u8; 0x80];
            if self.read_bytes(hit.wrapping_sub(0x100), &mut b).is_err() {
                continue;
            }
            let f = |o: usize| f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
            // 1.0 multiplier block at box-0x70,-0x60,..,-0x10 (buf 0x00,0x10,..,0x60).
            if !(0..=6usize).all(|k| (f(k * 0x10) - 1.0).abs() < 0.001) {
                continue;
            }
            let current = f(0x70);
            let max = f(0x74);
            let base = f(0x78);
            let scale = f(0x7C);
            if (base - 100.0).abs() < 1.0
                && current > 0.0
                && (1.0..=10_000.0).contains(&max)
                && (0.4..=3.5).contains(&scale)
            {
                return Some(hit.wrapping_sub(0x90));
            }
        }
        None
    }
}

/// The shared health-attribute invariant for a candidate {current,max,base,scale}
/// quad: `base*scale == max` (1% tolerance), with plausible ranges and a live
/// `0 < current <= max`. Every loaded humanoid health box satisfies it regardless
/// of archetype; random memory almost never does.
fn health_box_ok(current: f32, max: f32, base: f32, scale: f32) -> bool {
    (10.0..=5000.0).contains(&max)
        && (10.0..=5000.0).contains(&base)
        && (0.25..=4.0).contains(&scale)
        && (base * scale - max).abs() <= max * 0.01 + 0.5
        && current > 0.0
        && current <= max * 1.01
}

/// One enemy health box: its base VA (current health @ +0x00) and the decoded
/// attribute quad at scan time.
#[derive(Debug, Clone, Copy)]
pub struct EnemyHealthBox {
    pub va: u64,
    pub current: f32,
    pub max: f32,
    pub base: f32,
    pub scale: f32,
}

/// vtable of `ZFirearmCharacterEntity` (the `+0xC8` sub-object, the give-weapon
/// anchor) — the heap-scan needle for present firearms. Fixed VA (no-ASLR main
/// module), PER-BUILD: re-derive via RTTI on a fresh `--dump-module` dump after
/// a game update (`re_tools.py rtti .?AVZFirearmCharacterEntity@@`, pick the COL
/// at sub-object offset 0xC8).
const FIREARM_VT: u64 = 0x1_42DF_5760;
/// `ZCLTriggerPlayerItemPickup` handler, fired on the game thread to grant.
/// PER-BUILD function VA. Re-derived on the current build by AOB-matching the
/// old handler's distinctive prologue (`push rsi; sub rsp,0x60; mov rax,[rip+
/// handle_table]; mov rsi,rcx; mov edx,[rcx+0x20]; …; mov edi,0x1ac; …; cmp
/// [rcx+0x24],r8d; cmp [rcx+0x18],0`) — a byte-identical instruction match.
/// Was 0x1415305F0 on the May-31 build.
const PICKUP_HANDLER: u64 = 0x1_4153_0F91;

/// One present firearm instance: object base VA, its pickup node, and the grant
/// handle (idx = low u32, gen = high u32 of `firearm+0x10`).
#[derive(Debug, Clone)]
pub struct FirearmInstance {
    pub firearm_va: u64,
    pub node_va: u64,
    pub handle_idx: u32,
    pub handle_gen: u32,
}

/// One weapon type present in the world, keyed by shared audio-def ptr.
#[derive(Debug, Clone)]
pub struct FirearmType {
    /// `m_firearmItemType` model code, e.g. `"Pistol_WaltherPPK"`.
    pub code: String,
    /// Human-readable name (curated) when known, else the model code.
    pub display: String,
    pub audio_def_va: u64,
    pub instances: Vec<FirearmInstance>,
}

/// Read a little-endian u64 at `va` through the pipe; `None` on a short/failed
/// read.
fn gl_ru64(session: &GlacierSession, va: u64) -> Option<u64> {
    let mut b = [0u8; 8];
    session.read_bytes(va as usize, &mut b).ok()?;
    Some(u64::from_le_bytes(b))
}

/// Decode a Glacier `ZString` (16-byte descriptor) at `field_va`: len = low 30
/// bits of the u32 bitfield @+0, `char*` @+8. Rejects empty / oversize / null.
fn gl_read_zstr(session: &GlacierSession, field_va: u64) -> Option<String> {
    let len = (gl_ru64(session, field_va)? & 0x3FFF_FFFF) as usize;
    if len == 0 || len > 128 {
        return None;
    }
    let ptr = gl_ru64(session, field_va + 8)?;
    if ptr < 0x1_0000 {
        return None;
    }
    let mut buf = vec![0u8; len];
    session.read_bytes(ptr as usize, &mut buf).ok()?;
    Some(
        String::from_utf8_lossy(&buf)
            .trim_end_matches('\0')
            .to_string(),
    )
}

/// Curated model-code → human-readable name map. The pretty names are NOT in
/// the executable (they load from external `.locr` localization at runtime —
/// see the `re:display-names` RE), so this offline table is the canonical
/// source of clean labels. Unknown codes fall back to the model code.
fn pretty_weapon_name(code: &str) -> Option<&'static str> {
    Some(match code {
        "Pistol_WaltherPPK" => "Walther PPK",
        "Pistol_Light" => "Light Pistol",
        "Shotgun_Benelli" => "Benelli Shotgun",
        "Shotgun_Mossberg" => "Mossberg Shotgun",
        "SMG_MP5" => "MP5",
        "SMG_Compact" => "Compact SMG",
        "AR_KS1" => "KS1 Assault Rifle",
        _ => return None,
    })
}

// SAFETY: `Inner` holds only `Mutex<T: Send>` + plain data. The pipe HANDLE in
// `GlacierClient` is treated as `Send` (see `pipe::PipeHandle`) and is never
// exposed across threads without the `client` mutex.
unsafe impl Send for GlacierSession {}
unsafe impl Sync for GlacierSession {}

/// `Ctx` impl proxying every read / write / scan through the pipe to the DLL.
impl Ctx for GlacierSession {
    fn pid(&self) -> u32 {
        self.inner.welcome.pid
    }

    fn main_module(&self) -> &Module {
        &self.inner.modules[0]
    }

    fn module(&self, name: &str) -> Option<&Module> {
        if name.is_empty() {
            return Some(self.main_module());
        }
        self.inner
            .modules
            .iter()
            .find(|m| m.name.eq_ignore_ascii_case(name))
    }

    fn read_bytes(&self, addr: usize, buf: &mut [u8]) -> CoreResult<()> {
        if buf.is_empty() {
            return Ok(());
        }
        let bytes = self
            .inner
            .client
            .lock()
            .read_bytes(addr as u64, buf.len() as u32)
            .map_err(host_err)?;
        if bytes.len() != buf.len() {
            return Err(CoreError::Custom(format!(
                "glacier ipc read_bytes: short response ({}/{})",
                bytes.len(),
                buf.len()
            )));
        }
        buf.copy_from_slice(&bytes);
        Ok(())
    }

    fn write_bytes(&self, addr: usize, buf: &[u8]) -> CoreResult<()> {
        if buf.is_empty() {
            return Ok(());
        }
        self.inner
            .client
            .lock()
            .write_bytes(addr as u64, buf.to_vec())
            .map_err(host_err)
    }

    fn scan_module(&self, name: &str, pattern: &Pattern) -> CoreResult<Option<usize>> {
        let (bytes, mask) = pattern.wire();
        let wire = PatternWire { bytes, mask };
        let result = self
            .inner
            .client
            .lock()
            .scan_module(name, wire)
            .map_err(host_err)?;
        Ok(result.map(|a| a as usize))
    }

    /// Route code patches through the DLL's `CodePatch` op so it can
    /// VirtualProtect RX `.text` (the default verify-and-write can't) and
    /// auto-restore on disconnect. Returns `original` (the DLL verified the
    /// live bytes equal it before patching).
    fn patch_code(&self, addr: usize, original: &[u8], replacement: &[u8]) -> CoreResult<Vec<u8>> {
        self.inner
            .client
            .lock()
            .code_patch(addr as u64, original.to_vec(), replacement.to_vec())
            .map_err(host_err)?;
        Ok(original.to_vec())
    }

    fn restore_code(&self, addr: usize, original: &[u8]) -> CoreResult<()> {
        self.inner
            .client
            .lock()
            .restore_patch(addr as u64, original.to_vec())
            .map_err(host_err)
    }

    fn scan_module_all(&self, name: &str, pattern: &Pattern) -> CoreResult<Vec<usize>> {
        let (bytes, mask) = pattern.wire();
        let wire = PatternWire { bytes, mask };
        let v = self
            .inner
            .client
            .lock()
            .scan_module_all(name, wire)
            .map_err(host_err)?;
        Ok(v.into_iter().map(|a| a as usize).collect())
    }

    fn scan_heap_for_u64_labeled(
        &self,
        needle: u64,
        alignment: usize,
        label: &str,
    ) -> CoreResult<Vec<usize>> {
        let v = self
            .inner
            .client
            .lock()
            .scan_heap_for_u64(needle, alignment as u32, label)
            .map_err(host_err)?;
        Ok(v.into_iter().map(|a| a as usize).collect())
    }
}

fn host_err(e: HostError) -> CoreError {
    CoreError::Custom(format!("glacier-host ipc: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<GlacierSession>();
    }
}
