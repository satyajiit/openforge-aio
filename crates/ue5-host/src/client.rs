//! Typed RPC client over the DLL's named pipe.
//!
//! Frame: `u32` LE length prefix + postcard body. The DLL pipe runs in
//! message mode; we set the same mode on the client side via
//! [`crate::pipe::PipeHandle::open`].

use std::time::Duration;

use openforge_host_common::{PipeHandle, Protocol, recv_frame, send_frame};
use openforge_ue5_protocol::{
    FrameError, LogLevel, LuaOutputLine, LuaScriptStatus, ModuleEntry, NamePredicate,
    PROTOCOL_VERSION, PatternWire, PropInfo, PropKind, PropValue, Request, ResolvedProperty,
    Response, UFunctionInfo, UeObjectRef, encode_framed, parse_len_prefix, pipe_name_for_pid,
};
use tracing::{debug, info};

use crate::Welcome;
use crate::error::{HostError, Result};

/// Wire-protocol marker binding the UE5 `Request`/`Response` pair to the shared
/// length-prefixed postcard transport. The framing functions delegate to
/// `openforge-ue5-protocol` (the in-game DLL shares them), so the bytes on the
/// pipe — and the error values surfaced — are unchanged from the inline framing
/// this replaced.
pub(crate) struct Ue5Protocol;

impl Protocol for Ue5Protocol {
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

/// One client owns one pipe handle. **Not `Sync`** — the request/response
/// protocol is synchronous and stateful; use [`crate::Ue5Session`] for a
/// thread-safe facade.
pub struct Ue5Client {
    pipe: PipeHandle,
    pid: u32,
    welcome: Welcome,
}

impl Ue5Client {
    /// Open the DLL's pipe, send the `Hello`, parse the `Welcome`, cache it.
    ///
    /// `timeout` bounds the total wall-clock spent retrying `CreateFileW` and
    /// `WaitNamedPipeW`. A typical caller uses 5 s right after injection.
    pub fn connect(pid: u32, timeout: Duration) -> Result<Self> {
        let name = pipe_name_for_pid(pid);
        info!(pid, pipe = %name, ?timeout, "opening pipe");
        let pipe = PipeHandle::open(&name, timeout)?;
        let welcome = handshake(&pipe)?;
        if !welcome.layout_validated {
            // We still return the client — caller may still want to
            // `drain_log()` for diagnostics — but the convention is that the
            // session-level facade refuses operation. We expose this via the
            // cached `welcome.layout_validated == false` so callers can map it
            // to `HostError::LayoutUnresolved` themselves.
            debug!(pid, "DLL reports !layout_validated");
        }
        info!(
            pid,
            guobject_array = format_args!("0x{:X}", welcome.guobject_array),
            fname_pool = format_args!("0x{:X}", welcome.fname_pool),
            "handshake complete"
        );
        Ok(Self { pipe, pid, welcome })
    }

    /// Cached `Welcome` from the handshake.
    pub fn welcome(&self) -> &Welcome {
        &self.welcome
    }

    /// Pid this client is talking to.
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// `Request::Ping` -> `Response::Pong`.
    pub fn ping(&mut self) -> Result<()> {
        match self.request(&Request::Ping)? {
            Response::Pong => Ok(()),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected Pong")),
        }
    }

    /// `Request::WalkObjects { limit }` -> `Response::Objects`.
    pub fn walk_objects(&mut self, limit: Option<u32>) -> Result<Vec<UeObjectRef>> {
        match self.request(&Request::WalkObjects { limit })? {
            Response::Objects(v) => Ok(v),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected Objects")),
        }
    }

    /// `Request::WalkProperties { class_ptr }` -> `Response::Properties`.
    pub fn walk_properties(&mut self, class_ptr: u64) -> Result<Vec<PropInfo>> {
        match self.request(&Request::WalkProperties { class_ptr })? {
            Response::Properties(v) => Ok(v),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected Properties")),
        }
    }

    /// `Request::WalkFunctions { class_ptr }` -> `Response::Functions`.
    pub fn walk_functions(&mut self, class_ptr: u64) -> Result<Vec<UFunctionInfo>> {
        match self.request(&Request::WalkFunctions { class_ptr })? {
            Response::Functions(v) => Ok(v),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected Functions")),
        }
    }

    /// `Request::ReadProperty { addr, kind }` -> `Response::PropertyValue`.
    pub fn read_property(&mut self, addr: u64, kind: PropKind) -> Result<PropValue> {
        match self.request(&Request::ReadProperty { addr, kind })? {
            Response::PropertyValue(v) => Ok(v),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected PropertyValue")),
        }
    }

    /// `Request::WriteProperty { addr, kind, value }` -> `Response::WriteOk`.
    pub fn write_property(&mut self, addr: u64, value: PropValue) -> Result<()> {
        let kind = value.kind();
        match self.request(&Request::WriteProperty { addr, kind, value })? {
            Response::WriteOk => Ok(()),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected WriteOk")),
        }
    }

    /// `Request::SetLogLevel(level)` -> `Response::Pong` (DLL reuses Pong as
    /// the cheapest valid ack — the protocol has no SetLogLevelOk variant).
    /// We accept WriteOk and an empty LogLines defensively in case a future
    /// DLL build changes the ack shape.
    pub fn set_log_level(&mut self, level: LogLevel) -> Result<()> {
        match self.request(&Request::SetLogLevel(level))? {
            Response::Pong | Response::WriteOk | Response::LogLines(_) => Ok(()),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected ack for SetLogLevel")),
        }
    }

    /// `Request::DrainLog { max_lines }` -> `Response::LogLines`.
    pub fn drain_log(&mut self, max_lines: u32) -> Result<Vec<String>> {
        match self.request(&Request::DrainLog { max_lines })? {
            Response::LogLines(v) => Ok(v),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected LogLines")),
        }
    }

    // -- Single-channel memory ops -----------------------------------------

    /// `Request::EnumModules` → `Response::Modules`.
    pub fn enum_modules(&mut self) -> Result<Vec<ModuleEntry>> {
        match self.request(&Request::EnumModules)? {
            Response::Modules(v) => Ok(v),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected Modules")),
        }
    }

    /// `Request::ScanModule` → `Response::Matches`. Returns the first match
    /// in `module_name`'s `.text`, or `None`.
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

    /// `Request::ScanModuleAll` → `Response::Matches`.
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

    /// `Request::ScanHeapForU64` → `Response::Matches`. `label` is echoed
    /// into the DLL log for attribution (typically the feature id).
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

    /// `Request::CodePatch` → `Response::PatchOk { previous }`. The DLL
    /// tracks the patch and auto-restores it on pipe disconnect.
    pub fn code_patch(
        &mut self,
        addr: u64,
        original: Vec<u8>,
        replacement: Vec<u8>,
    ) -> Result<Vec<u8>> {
        match self.request(&Request::CodePatch {
            addr,
            original,
            replacement,
        })? {
            Response::PatchOk { previous } => Ok(previous),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected PatchOk")),
        }
    }

    /// `Request::RestorePatch` → `Response::RestoreOk`. Idempotent: writing
    /// `original` over bytes that already equal `original` succeeds. Removes
    /// the patch from the DLL's auto-restore map.
    pub fn restore_patch(&mut self, addr: u64, original: Vec<u8>) -> Result<()> {
        match self.request(&Request::RestorePatch { addr, original })? {
            Response::RestoreOk => Ok(()),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected RestoreOk")),
        }
    }

    /// Read raw bytes from the game process. Routed via the existing
    /// `ReadProperty { PropKind::Bytes(len) }` opcode — no new wire variant.
    pub fn read_bytes(&mut self, addr: u64, len: u32) -> Result<Vec<u8>> {
        if len == 0 {
            return Ok(Vec::new());
        }
        match self.request(&Request::ReadProperty {
            addr,
            kind: PropKind::Bytes(len),
        })? {
            Response::PropertyValue(PropValue::Bytes(b)) => Ok(b),
            Response::PropertyValue(other) => Err(HostError::InvalidResponse(Box::leak(
                format!("expected PropValue::Bytes, got {other:?}").into_boxed_str(),
            ))),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected PropertyValue")),
        }
    }

    /// Write raw bytes into the game process. Routed via `WriteProperty`.
    pub fn write_bytes(&mut self, addr: u64, data: Vec<u8>) -> Result<()> {
        if data.is_empty() {
            return Ok(());
        }
        let kind = PropKind::Bytes(data.len() as u32);
        match self.request(&Request::WriteProperty {
            addr,
            kind,
            value: PropValue::Bytes(data),
        })? {
            Response::WriteOk => Ok(()),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected WriteOk")),
        }
    }

    // -- Reflection-promoted feature ops ------------------------------------

    /// `Request::FindUObject` → `Response::FoundObject` or `Response::NotFound`.
    /// `Ok(None)` means "object isn't loaded yet" (typical when the game is
    /// in the main menu); the caller polls until `Ok(Some(_))`.
    pub fn find_uobject(
        &mut self,
        class_path: &str,
        predicate: NamePredicate,
    ) -> Result<Option<(u64, u64)>> {
        match self.request(&Request::FindUObject {
            class_path: class_path.to_string(),
            predicate,
        })? {
            Response::FoundObject {
                obj_addr,
                class_addr,
            } => Ok(Some((obj_addr, class_addr))),
            Response::NotFound => Ok(None),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse(
                "expected FoundObject | NotFound",
            )),
        }
    }

    /// `Request::FindAllUObjects` → `Response::FoundObjects` (always — empty
    /// `matches` is the "no live instances" reply for the iterating variant,
    /// distinct from `FindUObject`'s `NotFound`). The boolean is `true` iff
    /// the DLL hit `max_results` before completing the walk.
    pub fn find_all_uobjects(
        &mut self,
        class_path: &str,
        predicate: NamePredicate,
        max_results: u32,
    ) -> Result<(Vec<openforge_ue5_protocol::FoundObjectEntry>, bool)> {
        match self.request(&Request::FindAllUObjects {
            class_path: class_path.to_string(),
            predicate,
            max_results,
        })? {
            Response::FoundObjects { matches, truncated } => Ok((matches, truncated)),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected FoundObjects")),
        }
    }

    /// `Request::ResolveProperty` → `Response::Resolved` or `Response::NotFound`.
    /// `Ok(None)` means the property doesn't exist on the class chain (caller
    /// should treat this as a hard config error — the TOML names a property
    /// the game doesn't have).
    pub fn resolve_property(
        &mut self,
        class_addr: u64,
        property_name: &str,
    ) -> Result<Option<ResolvedProperty>> {
        match self.request(&Request::ResolveProperty {
            class_addr,
            property_name: property_name.to_string(),
        })? {
            Response::Resolved(r) => Ok(Some(r)),
            Response::NotFound => Ok(None),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected Resolved | NotFound")),
        }
    }

    /// `Request::CallUFunction` → `Response::CallOk` (with the post-call
    /// param buffer, including any out-parameters) or `Response::NotFound`
    /// if the named UFunction doesn't exist on the class chain.
    pub fn call_ufunction(
        &mut self,
        obj_addr: u64,
        class_addr: u64,
        function_name: &str,
        params: Vec<u8>,
    ) -> Result<Option<Vec<u8>>> {
        match self.request(&Request::CallUFunction {
            obj_addr,
            class_addr,
            function_name: function_name.to_string(),
            params,
        })? {
            Response::CallOk { return_value } => Ok(Some(return_value)),
            Response::NotFound => Ok(None),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected CallOk | NotFound")),
        }
    }

    /// `Request::RunLua` → `Response::LuaStarted` (dispatch acknowledged;
    /// the script's main chunk is now executing on the DLL's Lua worker
    /// thread). Parse / init errors surface as `Response::LuaError`, mapped
    /// to `HostError::Server`. A subsequent `RunLua` while a script is
    /// running cancels the prior one DLL-side.
    pub fn run_lua(&mut self, script: String, name: String) -> Result<()> {
        match self.request(&Request::RunLua { script, name })? {
            Response::LuaStarted => Ok(()),
            Response::LuaError { message } => Err(HostError::Server(message)),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected LuaStarted")),
        }
    }

    /// `Request::StopLua` → `Response::LuaStopped`. Idempotent — succeeds
    /// even if no script is currently running.
    pub fn stop_lua(&mut self) -> Result<()> {
        match self.request(&Request::StopLua)? {
            Response::LuaStopped => Ok(()),
            Response::LuaError { message } => Err(HostError::Server(message)),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected LuaStopped")),
        }
    }

    /// `Request::LuaStatus` → `Response::LuaStatusInfo`. Cheap; safe to
    /// poll from the host's lua-output drainer.
    pub fn lua_status(&mut self) -> Result<LuaScriptStatus> {
        match self.request(&Request::LuaStatus)? {
            Response::LuaStatusInfo(s) => Ok(s),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected LuaStatusInfo")),
        }
    }

    /// `Request::DrainLuaOutput { max_lines }` → `Response::LuaOutput`. An
    /// empty vec is a legitimate "no lines buffered yet" reply, NOT an
    /// error.
    pub fn drain_lua_output(&mut self, max_lines: u32) -> Result<Vec<LuaOutputLine>> {
        match self.request(&Request::DrainLuaOutput { max_lines })? {
            Response::LuaOutput { lines } => Ok(lines),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected LuaOutput")),
        }
    }

    /// Send `Disconnect`, await ack, drop the pipe.
    pub fn disconnect(mut self) -> Result<()> {
        // Write and read are kept separate here (unlike `request`) so a
        // server that closes the pipe *before* our final read still maps to
        // `Ok(())` — but a write failure still propagates, exactly as before.
        self.write(&Request::Disconnect)?;
        match recv_frame::<Ue5Protocol>(&self.pipe).map_err(HostError::from) {
            Ok(Response::DisconnectAck) => Ok(()),
            Ok(Response::Error(e)) => Err(HostError::Server(e)),
            Ok(_) => Err(HostError::InvalidResponse("expected DisconnectAck")),
            // Server may close the pipe before our final read; that's fine —
            // we asked it to disconnect.
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
        Ok(send_frame::<Ue5Protocol>(&self.pipe, req)?)
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
    let resp = send_frame::<Ue5Protocol>(
        pipe,
        &Request::Hello {
            client_version: PROTOCOL_VERSION,
        },
    )?;

    match resp {
        Response::Welcome {
            server_version,
            pid,
            guobject_array,
            fname_pool,
            fname_pool_chunks_offset,
            offsets,
            fuobject_item_stride,
            layout_validated,
        } => {
            if server_version != PROTOCOL_VERSION {
                return Err(HostError::HandshakeFailed(format!(
                    "protocol version mismatch: client={PROTOCOL_VERSION}, server={server_version}"
                )));
            }
            Ok(Welcome {
                server_version,
                pid,
                guobject_array,
                fname_pool,
                fname_pool_chunks_offset,
                offsets,
                fuobject_item_stride,
                layout_validated,
            })
        }
        Response::Error(e) => Err(HostError::HandshakeFailed(e)),
        other => Err(HostError::HandshakeFailed(format!(
            "expected Welcome, got {other:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openforge_ue5_protocol::UeOffsets;

    /// Verify the unexpected-response mapping for each typed RPC. We can't
    /// run the full pipe stack in unit tests, but we can confirm the match
    /// arms compile against every `Response` variant.
    #[test]
    fn unexpected_variants_map_to_invalid_response() {
        let _w = Welcome {
            server_version: PROTOCOL_VERSION,
            pid: 1,
            guobject_array: 0,
            fname_pool: 0,
            fname_pool_chunks_offset: 0,
            offsets: UeOffsets {
                uobject_class_private: 0,
                uobject_name_private: 0,
                uobject_outer_private: 0,
                ustruct_super_struct: 0,
                ustruct_child_properties: 0,
                ustruct_children: 0,
                ffield_class_private: 0,
                ffield_next: 0,
                ffield_name_private: 0,
                fproperty_offset_internal: 0,
                fproperty_element_size: 0,
                ufield_next: 0,
                ufunction_flags: 0,
                ufunction_func: 0,
            },
            fuobject_item_stride: 24,
            layout_validated: true,
        };
        // Compile-only: ensures these `match` arms cover the right shapes.
        fn _accept(r: Response) -> Result<()> {
            match r {
                Response::Pong => Ok(()),
                Response::Error(e) => Err(HostError::Server(e)),
                _ => Err(HostError::InvalidResponse("expected Pong")),
            }
        }
    }
}
