//! Pipe-server worker thread. Builds an in-process reflection Ctx, then spins
//! a single-client named-pipe loop serving Glacier memory + reflection ops
//! until the process exits.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::sync::Arc;
use std::time::Duration;

use openforge_core::Pattern;
use openforge_glacier_protocol::{
    LogLevel, MAX_FRAME_BYTES, ModuleEntry, PROTOCOL_VERSION, Request, Response, decode,
    encode_framed, parse_len_prefix, pipe_name_for_pid,
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

use openforge_dll_common::local_reader::LocalReader;
use openforge_dll_common::panic_guard::guarded;
use openforge_dll_common::pe;

use crate::local_ctx::LocalCtx;
use crate::log_ring::LogRing;
use crate::{freeze, reflection, scan};

const PIPE_BUFFER_SIZE: u32 = 64 * 1024;

/// Entry point for the worker thread spawned from `DllMain`.
pub fn worker_entry(log: Arc<LogRing>) {
    crate::flog!("INFO", "worker_entry start");
    // Let the loader settle so we don't enumerate modules mid-load.
    std::thread::sleep(Duration::from_millis(200));

    // Build the in-process reflection Ctx once. Memory ops work without it
    // (they hit LocalReader / fresh enumeration directly); reflection ops
    // require it and report a clear error if it's missing.
    let ctx = LocalCtx::new();
    match &ctx {
        Some(c) => {
            use openforge_core::Ctx;
            let m = c.main_module();
            crate::flog!(
                "INFO",
                "local ctx ready: main module {} base=0x{:X} size=0x{:X}",
                m.name,
                m.base,
                m.size
            );
            log.push(
                LogLevel::Info,
                format!(
                    "glacier reflection ctx ready: {} @ 0x{:X} (+0x{:X})",
                    m.name, m.base, m.size
                ),
            );
        }
        None => {
            crate::flog!(
                "ERROR",
                "LocalCtx::new() returned None — module enumeration failed; reflection disabled"
            );
            log.push(
                LogLevel::Error,
                "module enumeration failed; reflection ops will error".into(),
            );
        }
    }

    let pid = unsafe { GetCurrentProcessId() };
    let pipe_name = pipe_name_for_pid(pid);
    let pipe_name_w: Vec<u16> = OsStr::new(&pipe_name)
        .encode_wide()
        .chain(Some(0))
        .collect();
    let mut first = true;
    crate::flog!("INFO", "worker_entry: pipe loop start (pipe={pipe_name})");

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
        // Byte-stream mode: our protocol uses length-prefixed framing, so we
        // don't want the kernel chunking reads at message boundaries.
        // SAFETY: well-formed wide name + max-instances=1 single-client server.
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

        // SAFETY: pipe is a valid HANDLE.
        let connect_result = unsafe { ConnectNamedPipe(pipe, None) };
        if crate::SHUTDOWN.load(std::sync::atomic::Ordering::SeqCst) {
            crate::flog!("INFO", "worker: shutdown during ConnectNamedPipe; exiting");
            unsafe {
                let _ = CloseHandle(pipe);
            }
            return;
        }
        let connected = match connect_result {
            Ok(()) => true,
            Err(e) => {
                let code = e.code().0 as u32;
                // ERROR_PIPE_CONNECTED (535) — benign race-win.
                if code == 0x8007_00EF || code == 535 {
                    true
                } else if code == 0x8007_03E3 || code == 995 {
                    // ERROR_OPERATION_ABORTED — DllMain cancelled our IO.
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
        let _ = guarded(|| serve_client(pipe, ctx.as_ref(), &log));
        crate::flog!("INFO", "serve_client returned; tearing down pipe");
        log.push(LogLevel::Info, "client disconnected".into());

        // SAFETY: pipe handle still valid.
        unsafe {
            let _ = FlushFileBuffers(pipe);
            let _ = DisconnectNamedPipe(pipe);
            let _ = CloseHandle(pipe);
        }
    }
}

fn serve_client(pipe: HANDLE, ctx: Option<&LocalCtx>, log: &LogRing) {
    let mut handshake_done = false;
    loop {
        let req = match read_frame(pipe) {
            Ok(buf) => match decode::<Request>(&buf) {
                Ok(r) => r,
                Err(e) => {
                    let _ = write_msg(pipe, &Response::Error(format!("decode error: {e}")));
                    return;
                }
            },
            Err(ReadFrameError::Eof) => return,
            Err(ReadFrameError::Frame(s)) => {
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
                        "handshake: Hello client={client_version} server={PROTOCOL_VERSION}"
                    );
                    if *client_version != PROTOCOL_VERSION {
                        let _ = write_msg(
                            pipe,
                            &Response::Error(format!(
                                "version mismatch: server={PROTOCOL_VERSION} client={client_version}"
                            )),
                        );
                        return;
                    }
                    if write_msg(pipe, &welcome_payload(ctx)).is_err() {
                        return;
                    }
                    handshake_done = true;
                    continue;
                }
                _ => {
                    let _ = write_msg(pipe, &Response::Error("hello required".into()));
                    return;
                }
            }
        }

        let response = handle_request(req, ctx, log);
        let is_disconnect = matches!(response, Response::DisconnectAck);
        if write_msg(pipe, &response).is_err() {
            return;
        }
        if is_disconnect {
            return;
        }
    }
}

fn welcome_payload(ctx: Option<&LocalCtx>) -> Response {
    use openforge_core::Ctx;
    let pid = unsafe { GetCurrentProcessId() };
    let (module_base, module_size) = match ctx {
        Some(c) => {
            let m = c.main_module();
            (m.base as u64, m.size as u64)
        }
        None => (0, 0),
    };
    Response::Welcome {
        server_version: PROTOCOL_VERSION,
        pid,
        module_base,
        module_size,
    }
}

fn handle_request(req: Request, ctx: Option<&LocalCtx>, log: &LogRing) -> Response {
    match req {
        Request::Hello { .. } => Response::Error("hello already received".into()),
        Request::Ping => Response::Pong,
        Request::Disconnect => Response::DisconnectAck,
        Request::SetLogLevel(level) => {
            log.set_level(level);
            Response::LogLines(Vec::new())
        }
        Request::DrainLog { max_lines } => {
            let cap = if max_lines == 0 {
                usize::MAX
            } else {
                max_lines as usize
            };
            Response::LogLines(log.drain(cap))
        }

        // -- memory ops (Ctx-independent) ----------------------------------
        Request::ReadBytes { addr, len } => read_bytes(addr, len),
        Request::WriteBytes { addr, bytes } => write_bytes(addr, bytes),
        Request::EnumModules => Response::Modules(enum_modules()),
        Request::ScanModule {
            module_name,
            pattern,
        } => match Pattern::from_wire(&pattern.bytes, &pattern.mask) {
            Ok(p) => Response::Matches(scan::aob(&module_name, &p, true)),
            Err(e) => Response::Error(format!("bad pattern: {e}")),
        },
        Request::ScanModuleAll {
            module_name,
            pattern,
        } => match Pattern::from_wire(&pattern.bytes, &pattern.mask) {
            Ok(p) => Response::Matches(scan::aob(&module_name, &p, false)),
            Err(e) => Response::Error(format!("bad pattern: {e}")),
        },
        Request::ScanHeapForU64 {
            needle,
            alignment,
            label,
        } => Response::Matches(scan::heap_for_u64(needle, alignment, &label)),

        // -- reflection ops (require the in-process Ctx) -------------------
        Request::ResolveType { name } => with_ctx(ctx, |c| reflection::resolve_type(c, &name)),
        Request::EnumerateTypeProperties { name } => {
            with_ctx(ctx, |c| reflection::enumerate_type_properties(c, &name))
        }
        Request::InstanceProperties { entity_va } => {
            with_ctx(ctx, |c| reflection::instance_properties(c, entity_va))
        }
        Request::ResolveInstanceProperty { entity_va, name } => with_ctx(ctx, |c| {
            reflection::resolve_instance_property(c, entity_va, &name)
        }),
        Request::SetProperty {
            entity_va,
            property,
            value,
        } => with_ctx(ctx, |c| {
            reflection::set_property(c, entity_va, &property, &value)
        }),
        Request::FindEntitiesWithProperty {
            property,
            max_results,
        } => with_ctx(ctx, |c| {
            reflection::find_entities_with_property(c, &property, max_results)
        }),

        // -- engine-call actuation (protocol v3) ---------------------------
        Request::FireNode {
            node_va,
            inputs,
            fire,
        } => with_ctx(ctx, |c| reflection::fire_node(c, node_va, &inputs, &fire)),

        // Remaining v3 ops are reserved in the wire protocol but not yet
        // served by the DLL; report a clear error rather than silently
        // mishandling them.
        Request::GameThreadCall { .. } => {
            Response::Error("GameThreadCall not yet implemented".into())
        }
        Request::FindInstancesOfType { .. } => {
            Response::Error("FindInstancesOfType not yet implemented".into())
        }
        Request::RegisterFreeze { .. } => {
            Response::Error("RegisterFreeze not yet implemented".into())
        }
        Request::UnregisterFreeze { .. } => {
            Response::Error("UnregisterFreeze not yet implemented".into())
        }
        Request::SetPropertyEngine { .. } => {
            Response::Error("SetPropertyEngine not yet implemented".into())
        }

        // -- dynamic guarded freeze (protocol v4) --------------------------
        //
        // Ctx-independent (raw mem only): the freeze registry + thread own the
        // writes, so these route like ReadBytes/WriteBytes, not via with_ctx.
        Request::StartFreeze {
            box_va,
            write_offset,
            source_offset,
            value,
            value_kind,
            guard_min,
            guard_max,
        } => freeze::start_freeze(
            box_va,
            write_offset,
            source_offset,
            value,
            value_kind,
            guard_min,
            guard_max,
        ),
        Request::StopFreeze { handle } => freeze::stop_freeze(handle),
        Request::QueryFreezeStats { handle } => freeze::query_stats(handle),
    }
}

/// Run a reflection handler that needs the in-process Ctx, or report that the
/// Ctx is unavailable (module enumeration failed at startup).
fn with_ctx(ctx: Option<&LocalCtx>, f: impl FnOnce(&LocalCtx) -> Response) -> Response {
    match ctx {
        Some(c) => f(c),
        None => Response::Error(
            "local reflection ctx unavailable (module enumeration failed at DLL init)".into(),
        ),
    }
}

fn read_bytes(addr: u64, len: u32) -> Response {
    if len == 0 {
        return Response::Bytes(Vec::new());
    }
    if len > MAX_FRAME_BYTES {
        return Response::Error(format!("read too large: {len} (max {MAX_FRAME_BYTES})"));
    }
    let mut buf = vec![0u8; len as usize];
    match LocalReader::new().read_bytes(addr as usize, &mut buf) {
        Ok(()) => Response::Bytes(buf),
        Err(e) => Response::Error(format!("read at 0x{addr:X}: {e:?}")),
    }
}

fn write_bytes(addr: u64, bytes: Vec<u8>) -> Response {
    if bytes.is_empty() {
        return Response::Error("empty write".into());
    }
    if (bytes.len() as u32) > MAX_FRAME_BYTES {
        return Response::Error(format!("write too large: {}", bytes.len()));
    }
    match LocalReader::new().write_bytes(addr as usize, &bytes) {
        Ok(()) => Response::WriteOk,
        Err(e) => Response::Error(format!("write at 0x{addr:X}: {e:?}")),
    }
}

fn enum_modules() -> Vec<ModuleEntry> {
    pe::enumerate_modules()
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
                    return Err(ReadFrameError::Eof);
                }
                total += got as usize;
            }
            Err(e) => {
                // ERROR_BROKEN_PIPE (109) / ERROR_PIPE_NOT_CONNECTED (233) =
                // client gone. Treat as EOF.
                let code = e.code().0 as u32;
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
                    return Err("WriteFile returned 0 bytes".into());
                }
                buf = &buf[put as usize..];
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}
