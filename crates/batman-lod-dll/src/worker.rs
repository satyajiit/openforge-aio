//! Pipe-server worker thread. Locates UE5 reflection state, then spins on a
//! single-client named-pipe loop until the process exits.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::sync::Arc;
use std::time::Duration;

use openforge_ue5_protocol::{
    LogLevel, MAX_FRAME_BYTES, ModuleEntry, PROTOCOL_VERSION, PatternWire, PropKind, PropValue,
    Request, Response, UeObjectRef, UeOffsets, decode, encode_framed, parse_len_prefix,
    pipe_name_for_pid,
};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, FlushFileBuffers, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, NMPWAIT_USE_DEFAULT_WAIT,
    PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::core::PCWSTR;

use crate::engine::UeEngine;
use crate::log_ring::LogRing;
use crate::lotdk;
use crate::ops::{aob_scan, code_patch, heap_scan, reflection, state::ConnState};
use crate::panic_guard::guarded;
use std::sync::Arc as StdArc;

const PIPE_BUFFER_SIZE: u32 = 64 * 1024;

/// Entry point for the worker thread spawned from `DllMain`.
pub fn worker_entry(log: Arc<LogRing>) {
    crate::flog!("INFO", "worker_entry start");
    // Give the host loader a moment to settle so we don't probe globals that
    // are still being constructed.
    std::thread::sleep(Duration::from_millis(200));

    // Game-specific build configuration. The only place LotDK-specific
    // constants enter the worker — every other module is generic.
    let build = lotdk::ACTIVE;
    crate::flog!("INFO", "worker_entry: build = `{}`", build.game_id);

    let probe_module_base = match crate::pe::ModuleInfo::main() {
        Some(m) => m.base,
        None => {
            crate::flog!(
                "ERROR",
                "worker_entry: could not find main module for probe"
            );
            0
        }
    };
    crate::flog!(
        "INFO",
        "worker_entry: probe module_base=0x{:X}",
        probe_module_base
    );
    let probe_result =
        crate::probe::find_fname_to_string(probe_module_base, build.fname_to_string_candidates);
    match probe_result {
        Some(addr) => crate::flog!(
            "INFO",
            "worker_entry: probe SUCCESS — FName::ToString @ 0x{addr:X}"
        ),
        None => crate::flog!(
            "WARN",
            "worker_entry: probe yielded NO winner; reflection will be disabled"
        ),
    }

    let engine: Option<Arc<UeEngine>> = match probe_result {
        Some(fname_to_string) => {
            let guobject_array = probe_module_base + build.guobject_array_rva;
            let process_event = probe_module_base + build.process_event_rva;
            match UeEngine::attach_with_known_addresses(
                fname_to_string,
                guobject_array,
                process_event,
            ) {
                Ok(e) => {
                    crate::flog!(
                        "INFO",
                        "engine ready: fname_to_string=0x{:X} guobject_array=0x{:X} (hardcoded RVAs)",
                        e.fname_to_string,
                        e.guobject_array
                    );
                    log.push(
                        LogLevel::Info,
                        format!(
                            "ue5 reflection ready (probe path): fname_to_string=0x{:X} \
                             guobject_array=0x{:X}",
                            e.fname_to_string, e.guobject_array
                        ),
                    );
                    Some(Arc::new(e))
                }
                Err(err) => {
                    crate::flog!("ERROR", "attach_with_known_addresses failed: {err}");
                    None
                }
            }
        }
        None => {
            // Auto-discovery failed against this build (likely a game patch
            // shifted the RVA). The trainer ships its own discovery — no
            // user action required. The planned in-DLL AOB scan op will
            // re-derive the address from `BuildConfig.fname_to_string_aob`
            // when the hardcoded RVA misses. Until that's wired, the host
            // surfaces a "trainer needs an update" toast via the empty
            // `Welcome.guobject_array` value below.
            crate::flog!(
                "ERROR",
                "auto-discovery: FName::ToString probe found no match against \
                 lotdk::ACTIVE.fname_to_string_candidates. Game build likely \
                 patched; in-DLL AOB fallback will run once that's implemented."
            );
            log.push(
                LogLevel::Error,
                "auto-discovery failed; trainer needs an update for this game build".into(),
            );
            None
        }
    };

    let pid = unsafe { GetCurrentProcessId() };
    let pipe_name = pipe_name_for_pid(pid);
    let pipe_name_w: Vec<u16> = OsStr::new(&pipe_name)
        .encode_wide()
        .chain(Some(0))
        .collect();
    let mut first = true;
    crate::flog!("INFO", "worker_entry: pipe loop start (pipe={pipe_name})");

    // Bail early if shutdown was already requested between locate and the
    // loop start (extremely unlikely, but defensible).
    if crate::SHUTDOWN.load(std::sync::atomic::Ordering::SeqCst) {
        crate::flog!("INFO", "worker_entry: shutdown set before loop; exiting");
        return;
    }

    loop {
        let mut flags = PIPE_ACCESS_DUPLEX;
        if first {
            flags |= FILE_FLAG_FIRST_PIPE_INSTANCE;
            first = false;
        }
        // SAFETY: CreateNamedPipeW with a well-formed wide name + max-instances=1
        // is the documented single-client server pattern.
        // Byte-stream mode. Our protocol uses length-prefixed framing, so we
        // do not want the kernel to chunk reads at message boundaries; that
        // would surface `ERROR_MORE_DATA` (234) when a single ReadFile asks
        // for fewer bytes than the buffered message.
        let pipe = unsafe {
            CreateNamedPipeW(
                PCWSTR(pipe_name_w.as_ptr()),
                flags,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                PIPE_BUFFER_SIZE,
                PIPE_BUFFER_SIZE,
                NMPWAIT_USE_DEFAULT_WAIT,
                None,
            )
        };
        if pipe.is_invalid() {
            log.push(
                LogLevel::Error,
                format!("CreateNamedPipeW({pipe_name}) failed; retrying in 1s"),
            );
            std::thread::sleep(Duration::from_secs(1));
            continue;
        }

        // ConnectNamedPipe blocks until a client connects. ERROR_PIPE_CONNECTED
        // (535) is a benign race-win: the client connected between the create
        // and the connect call. `ERROR_OPERATION_ABORTED` (995) fires when
        // DllMain(DLL_PROCESS_DETACH) calls `CancelSynchronousIo` on us; we
        // treat that as a shutdown signal.
        // SAFETY: pipe is a valid HANDLE.
        let connect_result = unsafe { ConnectNamedPipe(pipe, None) };
        if crate::SHUTDOWN.load(std::sync::atomic::Ordering::SeqCst) {
            crate::flog!(
                "INFO",
                "worker: shutdown signaled during ConnectNamedPipe; exiting"
            );
            // SAFETY: pipe handle valid.
            unsafe {
                let _ = CloseHandle(pipe);
            }
            return;
        }
        let connected = match connect_result {
            Ok(()) => true,
            Err(e) => {
                let code = e.code().0 as u32;
                // 0x800700EF == ERROR_PIPE_CONNECTED wrapped as HRESULT.
                if code == 0x8007_00EF || code == 535 {
                    true
                } else if code == 0x8007_03E3 || code == 995 {
                    // ERROR_OPERATION_ABORTED — DllMain cancelled us.
                    crate::flog!("INFO", "ConnectNamedPipe aborted; exiting");
                    unsafe {
                        let _ = CloseHandle(pipe);
                    }
                    return;
                } else {
                    false
                }
            }
        };
        if !connected {
            log.push(
                LogLevel::Warn,
                "ConnectNamedPipe returned non-fatal error".into(),
            );
            unsafe {
                let _ = CloseHandle(pipe);
            }
            continue;
        }

        crate::flog!("INFO", "client connected; entering serve_client");
        log.push(LogLevel::Info, "client connected".into());
        // Per-client state. Drops at end of serve_client and auto-restores
        // any applied code patches.
        let mut conn = ConnState::new();
        let _ = guarded(|| serve_client(pipe, engine.clone(), log.clone(), &mut conn));
        crate::flog!("INFO", "serve_client returned; tearing down pipe");
        log.push(LogLevel::Info, "client disconnected".into());
        // Explicit drop runs ConnState::restore_all() before the next client
        // connects; auto-revert of patches happens here.
        drop(conn);

        // SAFETY: pipe handle still valid.
        unsafe {
            let _ = FlushFileBuffers(pipe);
            let _ = DisconnectNamedPipe(pipe);
            let _ = CloseHandle(pipe);
        }
    }
}

fn serve_client(
    pipe: HANDLE,
    engine: Option<Arc<UeEngine>>,
    log: Arc<LogRing>,
    conn: &mut ConnState,
) {
    let mut handshake_done = false;
    crate::flog!("INFO", "serve_client: enter");

    loop {
        crate::flog!("DEBUG", "serve_client: read_frame()");
        let req = match read_frame(pipe) {
            Ok(buf) => {
                crate::flog!("DEBUG", "read_frame ok: {} bytes", buf.len());
                match decode::<Request>(&buf) {
                    Ok(r) => {
                        crate::flog!("DEBUG", "decoded request: {r:?}");
                        r
                    }
                    Err(e) => {
                        crate::flog!("WARN", "decode error: {e}");
                        log.push(LogLevel::Warn, format!("decode error: {e}"));
                        let _ = write_msg(pipe, &Response::Error(format!("decode error: {e}")));
                        return;
                    }
                }
            }
            Err(ReadFrameError::Eof) => {
                crate::flog!("INFO", "read_frame EOF");
                return;
            }
            Err(ReadFrameError::Frame(s)) => {
                crate::flog!("WARN", "framing error: {s}");
                log.push(LogLevel::Warn, format!("framing error: {s}"));
                let _ = write_msg(pipe, &Response::Error(format!("framing error: {s}")));
                return;
            }
            Err(ReadFrameError::Io(s)) => {
                crate::flog!("WARN", "pipe IO error: {s}");
                log.push(LogLevel::Warn, format!("pipe IO error: {s}"));
                return;
            }
        };

        // Handshake gate.
        if !handshake_done {
            match &req {
                Request::Hello { client_version } => {
                    crate::flog!(
                        "INFO",
                        "handshake: Hello client_version={client_version} server={PROTOCOL_VERSION}"
                    );
                    if *client_version != PROTOCOL_VERSION {
                        let msg = format!(
                            "version mismatch: server={PROTOCOL_VERSION} client={client_version}"
                        );
                        let _ = write_msg(pipe, &Response::Error(msg));
                        return;
                    }
                    let response = welcome_payload(engine.as_deref());
                    crate::flog!("INFO", "handshake: about to write Welcome");
                    if let Err(e) = write_msg(pipe, &response) {
                        crate::flog!("ERROR", "handshake: write Welcome FAILED: {e}");
                        return;
                    }
                    crate::flog!("INFO", "handshake: Welcome written; awaiting next request");
                    handshake_done = true;
                    continue;
                }
                _ => {
                    crate::flog!("WARN", "pre-handshake non-Hello request: {req:?}");
                    let _ = write_msg(pipe, &Response::Error("hello required".into()));
                    return;
                }
            }
        }

        // Dispatch every other request. The `ConnState` mutation (applied
        // patches map) means we can't move it into a `guarded` closure; we
        // call directly. `guarded()` is only valuable around heavy reads
        // that may panic — none of the op handlers panic on bad input, they
        // return `Response::Error`. If we ever add a handler that does
        // panic-prone work, wrap that handler's body individually.
        crate::flog!("DEBUG", "dispatching request");
        let response = handle_request(req, engine.as_ref(), &log, conn);
        let is_disconnect = matches!(response, Response::DisconnectAck);
        crate::flog!("DEBUG", "writing response (disconnect={is_disconnect})");
        if let Err(e) = write_msg(pipe, &response) {
            crate::flog!("ERROR", "write response FAILED: {e}");
            return;
        }
        if is_disconnect {
            crate::flog!("INFO", "DisconnectAck sent; closing");
            return;
        }
    }
}

fn welcome_payload(engine: Option<&UeEngine>) -> Response {
    let pid = unsafe { GetCurrentProcessId() };
    if let Some(e) = engine {
        Response::Welcome {
            server_version: PROTOCOL_VERSION,
            pid,
            guobject_array: e.guobject_array as u64,
            // FNamePool fields are vestigial — the engine-call decode path
            // (FName::ToString) makes the chunk-walking layout irrelevant.
            // Kept on the wire for protocol-version stability; a later cleanup
            // retires them along with the new memory-op opcodes.
            fname_pool: 0,
            fname_pool_chunks_offset: 0,
            offsets: e.offsets,
            fuobject_item_stride: e.fuobject_item_stride,
            layout_validated: e.layout_validated,
        }
    } else {
        Response::Welcome {
            server_version: PROTOCOL_VERSION,
            pid,
            guobject_array: 0,
            fname_pool: 0,
            fname_pool_chunks_offset: 0,
            offsets: zero_offsets(),
            fuobject_item_stride: 0,
            layout_validated: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn welcome_encodes() {
        let w = welcome_payload(None);
        let frame = encode_framed(&w).expect("encode");
        assert!(frame.len() > 4);
        // First 4 bytes are u32 LE length.
        let len = u32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]);
        assert_eq!(len as usize, frame.len() - 4);
        // Round-trip decodes.
        let _r: Response = postcard::from_bytes(&frame[4..]).expect("decode");
    }
}

fn zero_offsets() -> UeOffsets {
    UeOffsets {
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
    }
}

fn handle_request(
    req: Request,
    engine_arc: Option<&StdArc<UeEngine>>,
    log: &LogRing,
    conn: &mut ConnState,
) -> Response {
    // Derive a borrowed view for the existing arms (they don't need
    // ownership). The Lua arm below clones the Arc into the runtime so the
    // Lua VM can hold the engine alive independent of the request frame.
    let engine: Option<&UeEngine> = engine_arc.map(|a| a.as_ref());
    match req {
        Request::Hello { .. } => Response::Error("hello already received".into()),
        Request::Ping => Response::Pong,
        Request::Disconnect => Response::DisconnectAck,
        Request::SetLogLevel(level) => {
            log.set_level(level);
            Response::Pong
        }
        Request::DrainLog { max_lines } => {
            let cap = max_lines.min(MAX_FRAME_BYTES) as usize;
            Response::LogLines(log.drain(cap))
        }
        Request::WalkObjects { limit } => match engine {
            None => Response::Error("layout not resolved".into()),
            Some(e) => {
                let lim = limit.unwrap_or(0) as usize;
                match e.walk_objects(lim) {
                    Ok(v) => Response::Objects(maybe_truncate(v, lim)),
                    Err(_) => Response::Error("walk_objects read error".into()),
                }
            }
        },
        Request::WalkProperties { class_ptr } => match engine {
            None => Response::Error("layout not resolved".into()),
            Some(e) => match e.walk_properties_cached(class_ptr) {
                Ok(arc) => Response::Properties((*arc).clone()),
                Err(_) => Response::Error("walk_properties read error".into()),
            },
        },
        Request::WalkFunctions { class_ptr } => match engine {
            None => Response::Error("layout not resolved".into()),
            Some(e) => match e.walk_functions_cached(class_ptr) {
                Ok(arc) => Response::Functions((*arc).clone()),
                Err(_) => Response::Error("walk_functions read error".into()),
            },
        },
        Request::ReadProperty { addr, kind } => read_property(engine, addr, kind),
        Request::WriteProperty { addr, kind, value } => write_property(engine, addr, kind, value),

        // -- Single-channel memory ops -------------------------------------
        Request::EnumModules => Response::Modules(enum_modules()),
        Request::ScanModule {
            module_name,
            pattern,
        } => Response::Matches(aob_scan::scan_first(&module_name, &pattern)),
        Request::ScanModuleAll {
            module_name,
            pattern,
        } => Response::Matches(aob_scan::scan_all(&module_name, &pattern)),
        Request::ScanHeapForU64 {
            needle,
            alignment,
            label,
        } => Response::Matches(heap_scan::scan(needle, alignment, &label)),
        Request::CodePatch {
            addr,
            original,
            replacement,
        } => match code_patch::apply(addr as usize, &original, &replacement) {
            Ok(previous) => {
                conn.applied_patches.insert(addr as usize, original);
                Response::PatchOk { previous }
            }
            Err(e) => Response::Error(format!("code_patch: {e}")),
        },
        Request::RestorePatch { addr, original } => {
            match code_patch::restore(addr as usize, &original) {
                Ok(()) => {
                    conn.applied_patches.remove(&(addr as usize));
                    Response::RestoreOk
                }
                Err(e) => Response::Error(format!("restore_patch: {e}")),
            }
        }

        // -- Reflection-promoted feature ops --------------------------------
        Request::FindUObject {
            class_path,
            predicate,
        } => match engine {
            None => Response::Error("layout not resolved".into()),
            Some(e) => match reflection::find_uobject(e, &class_path, &predicate) {
                Some((obj_addr, class_addr)) => Response::FoundObject {
                    obj_addr,
                    class_addr,
                },
                None => Response::NotFound,
            },
        },
        Request::FindAllUObjects {
            class_path,
            predicate,
            max_results,
        } => match engine {
            None => Response::Error("layout not resolved".into()),
            Some(e) => {
                let (matches, truncated) =
                    reflection::find_all_uobjects(e, &class_path, &predicate, max_results as usize);
                Response::FoundObjects { matches, truncated }
            }
        },
        Request::ResolveProperty {
            class_addr,
            property_name,
        } => match engine {
            None => Response::Error("layout not resolved".into()),
            Some(e) => match reflection::resolve_property(e, class_addr, &property_name) {
                Some(resolved) => Response::Resolved(resolved),
                None => Response::NotFound,
            },
        },
        Request::CallUFunction {
            obj_addr,
            class_addr,
            function_name,
            params,
        } => match engine {
            None => Response::Error("layout not resolved".into()),
            Some(e) => {
                match reflection::call_ufunction(e, obj_addr, class_addr, &function_name, &params) {
                    Ok(return_value) => Response::CallOk { return_value },
                    Err(msg) if msg.contains("not found") => Response::NotFound,
                    Err(msg) => Response::Error(format!("call_ufunction: {msg}")),
                }
            }
        },

        // -- In-DLL Lua runtime (protocol v5) -----------------------------
        Request::RunLua { script, name } => conn.lua.run(engine_arc.cloned(), script, name),
        Request::StopLua => conn.lua.stop(),
        Request::LuaStatus => conn.lua.status(),
        Request::DrainLuaOutput { max_lines } => conn.lua.drain_output(max_lines),
    }
}

fn enum_modules() -> Vec<ModuleEntry> {
    crate::pe::enumerate_modules()
        .into_iter()
        .map(|m| ModuleEntry {
            name: m.name,
            base: m.base as u64,
            size: m.size as u64,
            text_offset: m.text_offset as u64,
            text_size: m.text_size as u64,
        })
        .collect()
}

// Silence unused-import warning while only used in the new branches above.
#[allow(dead_code)]
fn _ensure_pattern_wire_used(_: &PatternWire) {}

fn maybe_truncate(mut v: Vec<UeObjectRef>, limit: usize) -> Vec<UeObjectRef> {
    if limit > 0 && v.len() > limit {
        v.truncate(limit);
    }
    v
}

fn read_property(engine: Option<&UeEngine>, addr: u64, kind: PropKind) -> Response {
    // Bound the read by the frame budget.
    let size = kind.size() as usize;
    if size == 0 || size as u32 > MAX_FRAME_BYTES {
        return Response::Error(format!("invalid size {size}"));
    }
    let reader = match engine {
        Some(e) => e.reader,
        None => crate::local_reader::LocalReader::new(),
    };
    let mut buf = vec![0u8; size];
    if reader.read_bytes(addr as usize, &mut buf).is_err() {
        return Response::Error(format!("read failed at 0x{:X}", addr));
    }
    let value = match kind {
        PropKind::I8 => PropValue::I8(i8::from_le_bytes(buf[..1].try_into().unwrap())),
        PropKind::U8 => PropValue::U8(buf[0]),
        PropKind::Bool => PropValue::Bool(buf[0] != 0),
        PropKind::I16 => PropValue::I16(i16::from_le_bytes(buf[..2].try_into().unwrap())),
        PropKind::U16 => PropValue::U16(u16::from_le_bytes(buf[..2].try_into().unwrap())),
        PropKind::I32 => PropValue::I32(i32::from_le_bytes(buf[..4].try_into().unwrap())),
        PropKind::U32 => PropValue::U32(u32::from_le_bytes(buf[..4].try_into().unwrap())),
        PropKind::F32 => PropValue::F32(f32::from_le_bytes(buf[..4].try_into().unwrap())),
        PropKind::I64 => PropValue::I64(i64::from_le_bytes(buf[..8].try_into().unwrap())),
        PropKind::U64 => PropValue::U64(u64::from_le_bytes(buf[..8].try_into().unwrap())),
        PropKind::F64 => PropValue::F64(f64::from_le_bytes(buf[..8].try_into().unwrap())),
        PropKind::Bytes(_) => PropValue::Bytes(buf),
    };
    Response::PropertyValue(value)
}

fn write_property(
    engine: Option<&UeEngine>,
    addr: u64,
    kind: PropKind,
    value: PropValue,
) -> Response {
    if value.kind() != kind {
        return Response::Error(format!(
            "kind mismatch: header={:?} value={:?}",
            kind,
            value.kind()
        ));
    }
    let reader = match engine {
        Some(e) => e.reader,
        None => crate::local_reader::LocalReader::new(),
    };
    let bytes: Vec<u8> = match value {
        PropValue::I8(v) => v.to_le_bytes().to_vec(),
        PropValue::U8(v) => v.to_le_bytes().to_vec(),
        PropValue::Bool(v) => vec![v as u8],
        PropValue::I16(v) => v.to_le_bytes().to_vec(),
        PropValue::U16(v) => v.to_le_bytes().to_vec(),
        PropValue::I32(v) => v.to_le_bytes().to_vec(),
        PropValue::U32(v) => v.to_le_bytes().to_vec(),
        PropValue::F32(v) => v.to_le_bytes().to_vec(),
        PropValue::I64(v) => v.to_le_bytes().to_vec(),
        PropValue::U64(v) => v.to_le_bytes().to_vec(),
        PropValue::F64(v) => v.to_le_bytes().to_vec(),
        PropValue::Bytes(b) => b,
    };
    if bytes.is_empty() {
        return Response::Error("empty write".into());
    }
    if (bytes.len() as u32) > MAX_FRAME_BYTES {
        return Response::Error(format!("write too large: {}", bytes.len()));
    }
    match reader.write_bytes(addr as usize, &bytes) {
        Ok(()) => Response::WriteOk,
        Err(_) => Response::Error(format!("write failed at 0x{:X}", addr)),
    }
}

// ---------- low-level pipe IO ----------------------------------------------

#[derive(Debug)]
enum ReadFrameError {
    Eof,
    Frame(String),
    Io(String),
}

fn read_frame(pipe: HANDLE) -> Result<Vec<u8>, ReadFrameError> {
    let mut prefix = [0u8; 4];
    read_exact(pipe, &mut prefix)?;
    let len = parse_len_prefix(prefix).map_err(|e| ReadFrameError::Frame(e.to_string()))?;
    let mut payload = vec![0u8; len as usize];
    read_exact(pipe, &mut payload)?;
    Ok(payload)
}

fn read_exact(pipe: HANDLE, out: &mut [u8]) -> Result<(), ReadFrameError> {
    let mut total = 0;
    while total < out.len() {
        let mut got: u32 = 0;
        // SAFETY: pipe is a valid HANDLE; out buffer is valid + writable.
        let ok = unsafe {
            ReadFile(
                pipe,
                Some(&mut out[total..]),
                Some(&mut got as *mut u32),
                None,
            )
        };
        match ok {
            Ok(()) => {
                if got == 0 {
                    crate::flog!("DEBUG", "ReadFile returned 0 bytes (EOF)");
                    return Err(ReadFrameError::Eof);
                }
                total += got as usize;
            }
            Err(e) => {
                // ERROR_BROKEN_PIPE (109) / ERROR_PIPE_NOT_CONNECTED (233) =
                // client gone. Treat as EOF.
                let code = e.code().0 as u32;
                crate::flog!("DEBUG", "ReadFile error: code=0x{code:08X}");
                if code == 0x8007_006D || code == 0x8007_00E9 || code == 109 || code == 233 {
                    return Err(ReadFrameError::Eof);
                }
                return Err(ReadFrameError::Io(e.to_string()));
            }
        }
    }
    Ok(())
}

fn write_msg<T: serde::Serialize>(pipe: HANDLE, msg: &T) -> Result<(), String> {
    let buf = encode_framed(msg).map_err(|e| format!("encode: {e}"))?;
    crate::flog!("DEBUG", "write_msg: encoded {} bytes", buf.len());
    write_all(pipe, &buf)
}

fn write_all(pipe: HANDLE, mut buf: &[u8]) -> Result<(), String> {
    while !buf.is_empty() {
        let mut put: u32 = 0;
        // SAFETY: pipe + buf valid for read.
        let ok = unsafe { WriteFile(pipe, Some(buf), Some(&mut put as *mut u32), None) };
        match ok {
            Ok(()) => {
                if put == 0 {
                    crate::flog!("ERROR", "WriteFile returned 0 bytes");
                    return Err("WriteFile returned 0 bytes".into());
                }
                buf = &buf[put as usize..];
            }
            Err(e) => {
                crate::flog!("ERROR", "WriteFile failed: {e}");
                return Err(e.to_string());
            }
        }
    }
    Ok(())
}
