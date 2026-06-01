//! Typed RPC client over the Glacier DLL's named pipe.
//!
//! Frame: `u32` LE length prefix + postcard body (see `glacier-protocol`).
//! The protocol is synchronous and stateful — one request, one response — so
//! this client is `!Sync`; [`crate::GlacierSession`] is the thread-safe facade.

use std::time::Duration;

use openforge_glacier_protocol::{
    FrameError, FreezeHandle, GlacierField, GlacierType, GlacierTypeProp, GlacierValue, LogLevel,
    ModuleEntry, NodeFire, NodeInput, PROTOCOL_VERSION, PatternWire, Request, Response, ValueKind,
    encode_framed, parse_len_prefix, pipe_name_for_pid,
};
use openforge_host_common::{PipeHandle, Protocol, recv_frame, send_frame};
use tracing::{debug, info};

use crate::Welcome;
use crate::error::{HostError, Result};

/// Wire-protocol marker binding the Glacier `Request`/`Response` pair to the
/// shared length-prefixed postcard transport. The framing functions delegate
/// to `openforge-glacier-protocol` (the in-game DLL shares them), so the bytes
/// on the pipe — and the error values surfaced — are unchanged from the inline
/// framing this replaced.
pub(crate) struct GlacierProtocol;

impl Protocol for GlacierProtocol {
    type Request = Request;
    type Response = Response;

    fn encode_request(req: &Request) -> std::result::Result<Vec<u8>, postcard::Error> {
        encode_framed(req)
    }

    fn parse_len(prefix: [u8; 4]) -> openforge_host_common::HostCommonResult<u32> {
        parse_len_prefix(prefix).map_err(|e| match e {
            FrameError::Oversize { got, max } => {
                openforge_host_common::HostCommonError::FrameOversize { got, max }
            }
            FrameError::Empty { .. } => openforge_host_common::HostCommonError::FrameEmpty,
            FrameError::Decode(e) => openforge_host_common::HostCommonError::Postcard(e),
        })
    }

    fn decode_response(body: &[u8]) -> std::result::Result<Response, postcard::Error> {
        postcard::from_bytes(body)
    }
}

/// Result of [`GlacierClient::find_writer`]: the instruction that wrote the
/// watched address, captured by an in-process HW data breakpoint. `rip` is the
/// trap RIP (the *next* instruction after the write); `bytes` is a window
/// starting at `bytes_start` to recover the writer's own start + synthesise an
/// AOB. `regs` is the integer register file in order
/// `[rax, rbx, rcx, rdx, rsi, rdi, rbp, rsp, r8..r15]`.
#[derive(Debug, Clone)]
pub struct WriterHit {
    pub rip: u64,
    pub bytes_start: u64,
    pub bytes: Vec<u8>,
    pub thread_id: u32,
    pub regs: [u64; 16],
}

/// One client owns one pipe handle.
pub struct GlacierClient {
    pipe: PipeHandle,
    pid: u32,
    welcome: Welcome,
}

impl GlacierClient {
    /// Open the DLL's pipe, send `Hello`, parse `Welcome`, cache it.
    pub fn connect(pid: u32, timeout: Duration) -> Result<Self> {
        let name = pipe_name_for_pid(pid);
        info!(pid, pipe = %name, ?timeout, "opening glacier pipe");
        let pipe = PipeHandle::open(&name, timeout)?;
        let welcome = handshake(&pipe)?;
        info!(
            pid,
            module_base = format_args!("0x{:X}", welcome.module_base),
            module_size = format_args!("0x{:X}", welcome.module_size),
            "glacier handshake complete"
        );
        Ok(Self { pipe, pid, welcome })
    }

    pub fn welcome(&self) -> &Welcome {
        &self.welcome
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn ping(&mut self) -> Result<()> {
        match self.request(&Request::Ping)? {
            Response::Pong => Ok(()),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected Pong")),
        }
    }

    // -- memory ops --------------------------------------------------------

    pub fn read_bytes(&mut self, addr: u64, len: u32) -> Result<Vec<u8>> {
        if len == 0 {
            return Ok(Vec::new());
        }
        match self.request(&Request::ReadBytes { addr, len })? {
            Response::Bytes(b) => Ok(b),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected Bytes")),
        }
    }

    pub fn write_bytes(&mut self, addr: u64, data: Vec<u8>) -> Result<()> {
        if data.is_empty() {
            return Ok(());
        }
        match self.request(&Request::WriteBytes { addr, bytes: data })? {
            Response::WriteOk => Ok(()),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected WriteOk")),
        }
    }

    pub fn enum_modules(&mut self) -> Result<Vec<ModuleEntry>> {
        match self.request(&Request::EnumModules)? {
            Response::Modules(v) => Ok(v),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected Modules")),
        }
    }

    pub fn scan_module(&mut self, module_name: &str, pattern: PatternWire) -> Result<Option<u64>> {
        match self.request(&Request::ScanModule {
            module_name: module_name.to_string(),
            pattern,
        })? {
            Response::Matches(v) => Ok(v.into_iter().next()),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected Matches")),
        }
    }

    pub fn scan_module_all(&mut self, module_name: &str, pattern: PatternWire) -> Result<Vec<u64>> {
        match self.request(&Request::ScanModuleAll {
            module_name: module_name.to_string(),
            pattern,
        })? {
            Response::Matches(v) => Ok(v),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected Matches")),
        }
    }

    pub fn scan_heap_for_u64(
        &mut self,
        needle: u64,
        alignment: u32,
        label: &str,
    ) -> Result<Vec<u64>> {
        match self.request(&Request::ScanHeapForU64 {
            needle,
            alignment,
            label: label.to_string(),
        })? {
            Response::Matches(v) => Ok(v),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected Matches")),
        }
    }

    /// Watch `addr` for a write of `width` bytes via an in-process HW
    /// breakpoint (Denuvo-safe — no debugger attach). Blocks up to `timeout_ms`.
    /// Returns the writing instruction's RIP, a byte window around it, the
    /// faulting thread id, and the integer register file at the trap.
    pub fn find_writer(&mut self, addr: u64, width: u8, timeout_ms: u32) -> Result<WriterHit> {
        match self.request(&Request::FindWriter {
            addr,
            width,
            timeout_ms,
        })? {
            Response::WriterFound {
                rip,
                bytes_start,
                bytes,
                thread_id,
                regs,
            } => Ok(WriterHit {
                rip,
                bytes_start,
                bytes,
                thread_id,
                regs,
            }),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected WriterFound")),
        }
    }

    /// Overwrite `.text` at `addr` with `patched` (DLL verifies it equals
    /// `original` first, VirtualProtects, and registers it for auto-restore on
    /// disconnect). Used by `GlacierSession::patch_code`.
    pub fn code_patch(&mut self, addr: u64, original: Vec<u8>, patched: Vec<u8>) -> Result<()> {
        match self.request(&Request::CodePatch {
            addr,
            original,
            patched,
        })? {
            Response::WriteOk => Ok(()),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected WriteOk")),
        }
    }

    /// Restore `original` at `addr` and drop it from the DLL's auto-restore set.
    pub fn restore_patch(&mut self, addr: u64, original: Vec<u8>) -> Result<()> {
        match self.request(&Request::RestorePatch { addr, original })? {
            Response::WriteOk => Ok(()),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected WriteOk")),
        }
    }

    // -- reflection --------------------------------------------------------

    /// `Ok(None)` means the type name isn't in the registry.
    pub fn resolve_type(&mut self, name: &str) -> Result<Option<GlacierType>> {
        match self.request(&Request::ResolveType {
            name: name.to_string(),
        })? {
            Response::ResolvedType(t) => Ok(Some(t)),
            Response::NotFound => Ok(None),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse(
                "expected ResolvedType | NotFound",
            )),
        }
    }

    /// `Ok(None)` means the type name isn't in the registry; `Ok(Some(vec))`
    /// (possibly empty) is a resolved type's static property list.
    pub fn enumerate_type_properties(
        &mut self,
        name: &str,
    ) -> Result<Option<Vec<GlacierTypeProp>>> {
        match self.request(&Request::EnumerateTypeProperties {
            name: name.to_string(),
        })? {
            Response::TypeProperties(v) => Ok(Some(v)),
            Response::NotFound => Ok(None),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse(
                "expected TypeProperties | NotFound",
            )),
        }
    }

    /// Returns `(obj_base, fields)` for a live entity.
    pub fn instance_properties(&mut self, entity_va: u64) -> Result<(u64, Vec<GlacierField>)> {
        match self.request(&Request::InstanceProperties { entity_va })? {
            Response::InstanceProps {
                obj_base, fields, ..
            } => Ok((obj_base, fields)),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected InstanceProps")),
        }
    }

    /// `Ok(None)` means the property isn't present on the live instance.
    pub fn resolve_instance_property(
        &mut self,
        entity_va: u64,
        name: &str,
    ) -> Result<Option<GlacierField>> {
        match self.request(&Request::ResolveInstanceProperty {
            entity_va,
            name: name.to_string(),
        })? {
            Response::ResolvedField(f) => Ok(Some(f)),
            Response::NotFound => Ok(None),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse(
                "expected ResolvedField | NotFound",
            )),
        }
    }

    /// `Ok(true)` = written; `Ok(false)` = property not present on the
    /// instance; `Err(Server(_))` = refused (e.g. getter/setter) or fault.
    pub fn set_property(
        &mut self,
        entity_va: u64,
        property: &str,
        value: GlacierValue,
    ) -> Result<bool> {
        match self.request(&Request::SetProperty {
            entity_va,
            property: property.to_string(),
            value,
        })? {
            Response::WriteOk => Ok(true),
            Response::NotFound => Ok(false),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected WriteOk | NotFound")),
        }
    }

    /// Heap-scan for live entities carrying `property`. Returns their VAs.
    pub fn find_entities_with_property(
        &mut self,
        property: &str,
        max_results: u32,
    ) -> Result<Vec<u64>> {
        match self.request(&Request::FindEntitiesWithProperty {
            property: property.to_string(),
            max_results,
        })? {
            Response::Entities(v) => Ok(v),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected Entities")),
        }
    }

    // -- engine-call actuation (protocol v3) ------------------------------

    /// Fire a logic node: configure `inputs` (when supported) then signal the
    /// pin selected by `fire`. `Ok(true)` = the engine call returned;
    /// `Ok(false)` = node/property not present; `Err(Server(_))` = SEH fault or
    /// unresolved engine function.
    pub fn fire_node(
        &mut self,
        node_va: u64,
        inputs: Vec<NodeInput>,
        fire: NodeFire,
    ) -> Result<bool> {
        match self.request(&Request::FireNode {
            node_va,
            inputs,
            fire,
        })? {
            Response::WriteOk => Ok(true),
            Response::NotFound => Ok(false),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected WriteOk | NotFound")),
        }
    }

    // -- dynamic guarded freeze (protocol v4) -----------------------------

    /// Start a DLL-side guarded per-frame freeze. For difficulty-agnostic god
    /// mode, hold `write_offset` by copying the live sibling at `source_offset`
    /// (e.g. current := max) — pass `value`/`value_kind` for the constant-stamp
    /// mode (`source_offset = None`). The `[guard_min, guard_max]` band skips a
    /// freed/reused box instead of corrupting it. Returns the freeze handle.
    #[allow(clippy::too_many_arguments)]
    pub fn start_freeze(
        &mut self,
        box_va: u64,
        write_offset: i64,
        source_offset: Option<i64>,
        value: GlacierValue,
        value_kind: ValueKind,
        guard_min: f32,
        guard_max: f32,
    ) -> Result<FreezeHandle> {
        match self.request(&Request::StartFreeze {
            box_va,
            write_offset,
            source_offset,
            value,
            value_kind,
            guard_min,
            guard_max,
        })? {
            Response::FreezeStarted { handle } => Ok(handle),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected FreezeStarted")),
        }
    }

    /// Stop a freeze started by [`GlacierClient::start_freeze`]. Idempotent.
    pub fn stop_freeze(&mut self, handle: FreezeHandle) -> Result<()> {
        match self.request(&Request::StopFreeze { handle })? {
            Response::FreezeStopped => Ok(()),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected FreezeStopped")),
        }
    }

    /// Query a running freeze's `(writes, skipped, ticks)` counters.
    pub fn query_freeze_stats(&mut self, handle: FreezeHandle) -> Result<(u64, u64, u64)> {
        match self.request(&Request::QueryFreezeStats { handle })? {
            Response::FreezeStats {
                writes,
                skipped,
                ticks,
            } => Ok((writes, skipped, ticks)),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected FreezeStats")),
        }
    }

    // -- logging / lifecycle ----------------------------------------------

    pub fn set_log_level(&mut self, level: LogLevel) -> Result<()> {
        match self.request(&Request::SetLogLevel(level))? {
            Response::LogLines(_) => Ok(()),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected ack for SetLogLevel")),
        }
    }

    pub fn drain_log(&mut self, max_lines: u32) -> Result<Vec<String>> {
        match self.request(&Request::DrainLog { max_lines })? {
            Response::LogLines(v) => Ok(v),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected LogLines")),
        }
    }

    pub fn disconnect(mut self) -> Result<()> {
        // Write and read are kept separate here (unlike `request`) so a
        // server that closes the pipe *before* our final read still maps to
        // `Ok(())` — but a write failure still propagates, exactly as before.
        self.write(&Request::Disconnect)?;
        match recv_frame::<GlacierProtocol>(&self.pipe).map_err(HostError::from) {
            Ok(Response::DisconnectAck) => Ok(()),
            Ok(Response::Error(e)) => Err(HostError::Server(e)),
            Ok(_) => Err(HostError::InvalidResponse("expected DisconnectAck")),
            Err(HostError::Disconnected) => Ok(()),
            Err(e) => Err(e),
        }
    }

    // ---- private framing helpers -----------------------------------------

    /// Send one request and read exactly one response over the shared
    /// length-prefixed postcard transport. Replaces the per-client framing
    /// copy with [`openforge_host_common::send_frame`] — same bytes, same
    /// error values (mapped via `From<HostCommonError>`).
    fn request(&mut self, req: &Request) -> Result<Response> {
        Ok(send_frame::<GlacierProtocol>(&self.pipe, req)?)
    }

    /// Write one framed request without reading a response. Only `disconnect`
    /// needs the write/read split; everything else uses [`Self::request`].
    fn write(&mut self, req: &Request) -> Result<()> {
        let frame = encode_framed(req).map_err(HostError::Postcard)?;
        debug!(bytes = frame.len(), "tx frame");
        Ok(self.pipe.write_all(&frame)?)
    }
}

/// Send `Hello` + receive `Welcome`. Any other reply is fatal.
fn handshake(pipe: &PipeHandle) -> Result<Welcome> {
    let resp = send_frame::<GlacierProtocol>(
        pipe,
        &Request::Hello {
            client_version: PROTOCOL_VERSION,
        },
    )?;

    match resp {
        Response::Welcome {
            server_version,
            pid,
            module_base,
            module_size,
        } => {
            if server_version != PROTOCOL_VERSION {
                return Err(HostError::HandshakeFailed(format!(
                    "protocol version mismatch: client={PROTOCOL_VERSION}, server={server_version}"
                )));
            }
            Ok(Welcome {
                server_version,
                pid,
                module_base,
                module_size,
            })
        }
        Response::Error(e) => Err(HostError::HandshakeFailed(e)),
        other => Err(HostError::HandshakeFailed(format!(
            "expected Welcome, got {other:?}"
        ))),
    }
}
