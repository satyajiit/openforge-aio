//! Typed RPC client over the Glacier DLL's named pipe.
//!
//! Frame: `u32` LE length prefix + postcard body (see `glacier-protocol`).
//! The protocol is synchronous and stateful — one request, one response — so
//! this client is `!Sync`; [`crate::GlacierSession`] is the thread-safe facade.

use std::time::Duration;

use openforge_glacier_protocol::{
    GlacierField, GlacierType, GlacierTypeProp, GlacierValue, LogLevel, ModuleEntry,
    PROTOCOL_VERSION, PatternWire, Request, Response, encode_framed, parse_len_prefix,
    pipe_name_for_pid,
};
use tracing::{debug, info};

use crate::Welcome;
use crate::error::{HostError, Result};
use crate::pipe::PipeHandle;

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
        self.write_request(&Request::Ping)?;
        match self.read_response()? {
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
        self.write_request(&Request::ReadBytes { addr, len })?;
        match self.read_response()? {
            Response::Bytes(b) => Ok(b),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected Bytes")),
        }
    }

    pub fn write_bytes(&mut self, addr: u64, data: Vec<u8>) -> Result<()> {
        if data.is_empty() {
            return Ok(());
        }
        self.write_request(&Request::WriteBytes { addr, bytes: data })?;
        match self.read_response()? {
            Response::WriteOk => Ok(()),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected WriteOk")),
        }
    }

    pub fn enum_modules(&mut self) -> Result<Vec<ModuleEntry>> {
        self.write_request(&Request::EnumModules)?;
        match self.read_response()? {
            Response::Modules(v) => Ok(v),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected Modules")),
        }
    }

    pub fn scan_module(&mut self, module_name: &str, pattern: PatternWire) -> Result<Option<u64>> {
        self.write_request(&Request::ScanModule {
            module_name: module_name.to_string(),
            pattern,
        })?;
        match self.read_response()? {
            Response::Matches(v) => Ok(v.into_iter().next()),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected Matches")),
        }
    }

    pub fn scan_module_all(&mut self, module_name: &str, pattern: PatternWire) -> Result<Vec<u64>> {
        self.write_request(&Request::ScanModuleAll {
            module_name: module_name.to_string(),
            pattern,
        })?;
        match self.read_response()? {
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
        self.write_request(&Request::ScanHeapForU64 {
            needle,
            alignment,
            label: label.to_string(),
        })?;
        match self.read_response()? {
            Response::Matches(v) => Ok(v),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected Matches")),
        }
    }

    // -- reflection --------------------------------------------------------

    /// `Ok(None)` means the type name isn't in the registry.
    pub fn resolve_type(&mut self, name: &str) -> Result<Option<GlacierType>> {
        self.write_request(&Request::ResolveType {
            name: name.to_string(),
        })?;
        match self.read_response()? {
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
        self.write_request(&Request::EnumerateTypeProperties {
            name: name.to_string(),
        })?;
        match self.read_response()? {
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
        self.write_request(&Request::InstanceProperties { entity_va })?;
        match self.read_response()? {
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
        self.write_request(&Request::ResolveInstanceProperty {
            entity_va,
            name: name.to_string(),
        })?;
        match self.read_response()? {
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
        self.write_request(&Request::SetProperty {
            entity_va,
            property: property.to_string(),
            value,
        })?;
        match self.read_response()? {
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
        self.write_request(&Request::FindEntitiesWithProperty {
            property: property.to_string(),
            max_results,
        })?;
        match self.read_response()? {
            Response::Entities(v) => Ok(v),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected Entities")),
        }
    }

    // -- logging / lifecycle ----------------------------------------------

    pub fn set_log_level(&mut self, level: LogLevel) -> Result<()> {
        self.write_request(&Request::SetLogLevel(level))?;
        match self.read_response()? {
            Response::LogLines(_) => Ok(()),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected ack for SetLogLevel")),
        }
    }

    pub fn drain_log(&mut self, max_lines: u32) -> Result<Vec<String>> {
        self.write_request(&Request::DrainLog { max_lines })?;
        match self.read_response()? {
            Response::LogLines(v) => Ok(v),
            Response::Error(e) => Err(HostError::Server(e)),
            _ => Err(HostError::InvalidResponse("expected LogLines")),
        }
    }

    pub fn disconnect(mut self) -> Result<()> {
        self.write_request(&Request::Disconnect)?;
        match self.read_response() {
            Ok(Response::DisconnectAck) => Ok(()),
            Ok(Response::Error(e)) => Err(HostError::Server(e)),
            Ok(_) => Err(HostError::InvalidResponse("expected DisconnectAck")),
            Err(HostError::Disconnected) => Ok(()),
            Err(e) => Err(e),
        }
    }

    // ---- private framing -------------------------------------------------

    fn write_request(&mut self, req: &Request) -> Result<()> {
        let frame = encode_framed(req).map_err(HostError::Postcard)?;
        debug!(bytes = frame.len(), "tx frame");
        self.pipe.write_all(&frame)
    }

    fn read_response(&mut self) -> Result<Response> {
        let mut prefix = [0u8; 4];
        self.pipe.read_exact(&mut prefix)?;
        let len = parse_len_prefix(prefix)? as usize;
        let mut body = vec![0u8; len];
        self.pipe.read_exact(&mut body)?;
        debug!(bytes = body.len(), "rx frame");
        postcard::from_bytes(&body).map_err(HostError::Postcard)
    }
}

/// Send `Hello` + receive `Welcome`. Any other reply is fatal.
fn handshake(pipe: &PipeHandle) -> Result<Welcome> {
    let frame = encode_framed(&Request::Hello {
        client_version: PROTOCOL_VERSION,
    })
    .map_err(HostError::Postcard)?;
    pipe.write_all(&frame)?;

    let mut prefix = [0u8; 4];
    pipe.read_exact(&mut prefix)?;
    let len = parse_len_prefix(prefix)? as usize;
    let mut body = vec![0u8; len];
    pipe.read_exact(&mut body)?;
    let resp: Response = postcard::from_bytes(&body).map_err(HostError::Postcard)?;

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
