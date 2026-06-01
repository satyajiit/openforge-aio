//! Named-pipe transport: a thin `HANDLE` wrapper ([`PipeHandle`]) plus the
//! engine-agnostic, length-prefixed postcard request/response framing
//! ([`Protocol`] + [`send_frame`] / [`recv_frame`]).
//!
//! Wraps `CreateFileW`/`ReadFile`/`WriteFile`/`WaitNamedPipeW` so the per-engine
//! client never mentions a Win32 type except `HANDLE`. Each call returns
//! [`HostCommonError::Win32`] with the call name baked in.
//!
//! ## Framing seam
//!
//! The wire format is identical across engines: a `u32`-LE length prefix
//! followed by a postcard-encoded body. The *encode/decode* themselves stay in
//! each `*-protocol` crate (the in-game DLL shares them), so the [`Protocol`]
//! trait carries them as associated functions and [`send_frame`] drives the
//! exact same `write_all` / `read_exact` sequence the per-engine clients used
//! before this was shared — byte-for-byte unchanged behavior.

use std::time::{Duration, Instant};

use serde::Serialize;
use serde::de::DeserializeOwned;
use tracing::{debug, warn};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_BROKEN_PIPE, ERROR_FILE_NOT_FOUND, ERROR_NO_DATA, ERROR_PIPE_BUSY,
    GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_NONE, OPEN_EXISTING, ReadFile, WriteFile,
};
use windows::Win32::System::Pipes::WaitNamedPipeW;
use windows::core::PCWSTR;

use crate::error::{HostCommonError, Result};

/// Owned pipe `HANDLE` that closes itself on drop.
pub struct PipeHandle(HANDLE);

impl PipeHandle {
    /// `CreateFileW(name, GENERIC_READ | GENERIC_WRITE, 0, NULL, OPEN_EXISTING,
    /// 0, NULL)` with a retry loop that handles `ERROR_PIPE_BUSY` via
    /// `WaitNamedPipeW(250 ms)` and `ERROR_FILE_NOT_FOUND` via a 250 ms sleep.
    /// Total wall-clock budget is bounded by `overall_timeout`.
    pub fn open(name: &str, overall_timeout: Duration) -> Result<Self> {
        let mut wide: Vec<u16> = name.encode_utf16().collect();
        wide.push(0);
        let start = Instant::now();
        let step = Duration::from_millis(250);

        loop {
            // SAFETY: `wide` is null-terminated; the value we pass to
            // CreateFileW is a stable pointer for the duration of the call.
            let result = unsafe {
                CreateFileW(
                    PCWSTR(wide.as_ptr()),
                    (GENERIC_READ | GENERIC_WRITE).0,
                    FILE_SHARE_NONE,
                    None,
                    OPEN_EXISTING,
                    FILE_FLAGS_AND_ATTRIBUTES(0),
                    None,
                )
            };

            match result {
                Ok(handle) if !handle.is_invalid() => {
                    debug!(pipe = name, "pipe opened");
                    // DLL serves in BYTE / BYTE-stream mode; no client-side
                    // mode switch needed. Length-prefixed framing means we
                    // never rely on the kernel for message boundaries.
                    return Ok(PipeHandle(handle));
                }
                Ok(_) | Err(_) => {
                    // Inspect the OS error to decide whether to retry.
                    // SAFETY: GetLastError is always callable.
                    let code = unsafe { GetLastError() };
                    let elapsed = start.elapsed();
                    let remaining = overall_timeout.saturating_sub(elapsed);
                    if remaining.is_zero() {
                        return Err(HostCommonError::Win32 {
                            call: "CreateFileW(pipe)",
                            code: code.0,
                        });
                    }
                    if code == ERROR_PIPE_BUSY {
                        // Wait up to 250 ms for the server side to free up.
                        // SAFETY: `wide` is null-terminated and lives across
                        // the call.
                        let _ = unsafe { WaitNamedPipeW(PCWSTR(wide.as_ptr()), 250) };
                    } else if code == ERROR_FILE_NOT_FOUND {
                        // DLL hasn't created the pipe yet. Back off and retry.
                        std::thread::sleep(step.min(remaining));
                    } else {
                        warn!(?code, "CreateFileW(pipe) failed, retrying");
                        std::thread::sleep(step.min(remaining));
                    }
                }
            }
        }
    }

    /// Write `buf` in full. Returns `Disconnected` if the server hangs up
    /// mid-write.
    pub fn write_all(&self, buf: &[u8]) -> Result<()> {
        let mut remaining = buf;
        while !remaining.is_empty() {
            let mut written: u32 = 0;
            // SAFETY: handle valid; lpBuffer is a slice; `lpNumberOfBytesWritten`
            // is a valid u32 ptr.
            let res = unsafe { WriteFile(self.0, Some(remaining), Some(&mut written), None) };
            match res {
                Ok(()) => {
                    if written == 0 {
                        return Err(HostCommonError::Disconnected);
                    }
                    remaining = &remaining[written as usize..];
                }
                Err(_) => {
                    // SAFETY: GetLastError is always callable.
                    let code = unsafe { GetLastError() };
                    if code == ERROR_BROKEN_PIPE || code == ERROR_NO_DATA {
                        return Err(HostCommonError::Disconnected);
                    }
                    return Err(HostCommonError::Win32 {
                        call: "WriteFile(pipe)",
                        code: code.0,
                    });
                }
            }
        }
        Ok(())
    }

    /// Read exactly `buf.len()` bytes. Returns `Disconnected` if the pipe
    /// is closed before the read completes.
    pub fn read_exact(&self, buf: &mut [u8]) -> Result<()> {
        let mut filled: usize = 0;
        while filled < buf.len() {
            let mut got: u32 = 0;
            // SAFETY: handle valid; tail slice is non-empty; `&mut got` is a
            // valid u32 ptr.
            let res = unsafe { ReadFile(self.0, Some(&mut buf[filled..]), Some(&mut got), None) };
            match res {
                Ok(()) => {
                    if got == 0 {
                        return Err(HostCommonError::Disconnected);
                    }
                    filled += got as usize;
                }
                Err(_) => {
                    // SAFETY: GetLastError is always callable.
                    let code = unsafe { GetLastError() };
                    if code == ERROR_BROKEN_PIPE {
                        return Err(HostCommonError::Disconnected);
                    }
                    return Err(HostCommonError::Win32 {
                        call: "ReadFile(pipe)",
                        code: code.0,
                    });
                }
            }
        }
        Ok(())
    }

    /// The raw Win32 `HANDLE`. Rarely needed (the framed transport covers the
    /// common path); kept for diagnostics / future direct-IO callers.
    #[allow(dead_code)]
    pub fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for PipeHandle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: we own this handle.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

// SAFETY: a Win32 pipe HANDLE is a kernel integer that can move across
// threads. Each engine client puts its `PipeHandle` behind a `&mut self` API
// (caller is single-threaded per client), so no aliasing exists.
unsafe impl Send for PipeHandle {}

/// One engine's wire protocol: the `Request`/`Response` pair plus the
/// length-prefixed postcard framing. The encode/decode live in the engine's
/// `*-protocol` crate (shared with the in-game DLL), so they are supplied here
/// as associated functions rather than reimplemented — keeping
/// [`send_frame`]'s behavior byte-for-byte identical to the pre-dedup clients.
pub trait Protocol {
    /// The request enum sent to the DLL.
    type Request: Serialize;
    /// The response enum received from the DLL.
    type Response: DeserializeOwned;

    /// Encode `req` into a full frame: `u32`-LE length prefix + postcard body.
    /// Implementors delegate to their `*-protocol` crate's `encode_framed`.
    fn encode_request(req: &Self::Request) -> std::result::Result<Vec<u8>, postcard::Error>;

    /// Parse a `u32`-LE length prefix into the body length, applying the
    /// protocol's frame-size bounds. Implementors delegate to their
    /// `*-protocol` crate's `parse_len_prefix`, mapping its frame error into
    /// [`HostCommonError::FrameEmpty`] / [`HostCommonError::FrameOversize`].
    fn parse_len(prefix: [u8; 4]) -> Result<u32>;

    /// Decode a postcard response body. Implementors delegate to
    /// `postcard::from_bytes`.
    fn decode_response(body: &[u8]) -> std::result::Result<Self::Response, postcard::Error>;
}

/// Send one `req` and read exactly one response over `pipe`, using `P`'s
/// length-prefixed postcard framing.
///
/// This is the exact sequence every engine client ran inline before the
/// dedup: encode the framed request, `write_all`, read the 4-byte prefix,
/// `parse_len`, read the body, decode. Behavior is unchanged.
pub fn send_frame<P: Protocol>(pipe: &PipeHandle, req: &P::Request) -> Result<P::Response> {
    let frame = P::encode_request(req)?;
    debug!(bytes = frame.len(), "tx frame");
    pipe.write_all(&frame)?;
    recv_frame::<P>(pipe)
}

/// Read exactly one `P::Response` frame off `pipe` (4-byte LE prefix + body).
/// Split out so a handshake that writes a raw frame can reuse the read half.
pub fn recv_frame<P: Protocol>(pipe: &PipeHandle) -> Result<P::Response> {
    let mut prefix = [0u8; 4];
    pipe.read_exact(&mut prefix)?;
    let len = P::parse_len(prefix)? as usize;
    let mut body = vec![0u8; len];
    pipe.read_exact(&mut body)?;
    debug!(bytes = body.len(), "rx frame");
    Ok(P::decode_response(&body)?)
}
