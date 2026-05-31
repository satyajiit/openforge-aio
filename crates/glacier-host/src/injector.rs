//! `CreateRemoteThread(LoadLibraryW)` DLL injection.
//!
//! Game-agnostic: the caller supplies the DLL path; this module derives the
//! file name for idempotency checks. Idempotent — re-running [`Injector::inject`]
//! for a pid that already has the DLL loaded is a no-op.

use std::ffi::c_void;
use std::path::Path;

use tracing::{debug, info, warn};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Diagnostics::Debug::WriteProcessMemory;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, MODULEENTRY32W, Module32FirstW, Module32NextW, TH32CS_SNAPMODULE,
    TH32CS_SNAPMODULE32,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::System::Memory::{
    MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAllocEx, VirtualFreeEx,
};
use windows::Win32::System::Threading::{
    CreateRemoteThread, GetExitCodeThread, INFINITE, LPTHREAD_START_ROUTINE, OpenProcess,
    PROCESS_CREATE_THREAD, PROCESS_QUERY_INFORMATION, PROCESS_VM_OPERATION, PROCESS_VM_READ,
    PROCESS_VM_WRITE, WaitForSingleObject,
};
use windows::core::{PCWSTR, w};

use crate::error::{HostError, Result};

/// One-shot DLL injector. Stateless.
pub struct Injector;

fn dll_file_name_from_path(dll_path: &Path) -> Result<String> {
    match dll_path.file_name().and_then(|s| s.to_str()) {
        Some(name) if !name.is_empty() => Ok(name.to_owned()),
        _ => Err(HostError::DllNotFound(dll_path.to_path_buf())),
    }
}

impl Injector {
    /// Inject `dll_path` into `pid` via `CreateRemoteThread(LoadLibraryW)`.
    /// Idempotent: a no-op if the DLL is already loaded (case-insensitive
    /// file-name match against the target's module list).
    pub fn inject(pid: u32, dll_path: &Path) -> Result<()> {
        if !dll_path.is_file() {
            return Err(HostError::DllNotFound(dll_path.to_path_buf()));
        }
        let dll_file_name = dll_file_name_from_path(dll_path)?;

        match is_dll_loaded(pid, &dll_file_name) {
            Ok(true) => {
                info!(pid, dll = %dll_path.display(), "DLL already loaded; injection skipped");
                return Ok(());
            }
            Ok(false) => {}
            Err(e) => warn!(
                ?e,
                pid, "module enumeration failed; proceeding with injection anyway"
            ),
        }

        // Wide-encode the path. UTF-16, null-terminated.
        let wide: Vec<u16> = dll_path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0u16))
            .collect();
        let wide_bytes: usize = wide.len().saturating_mul(2);
        if wide_bytes == 0 {
            return Err(HostError::InjectionFailed(
                "wide-encoded DLL path is empty".into(),
            ));
        }

        // Resolve LoadLibraryW in *our* process — kernel32 base is shared
        // across the session, so the address is valid as a remote start routine.
        let load_library_w_addr = resolve_load_library_w()?;

        let access = PROCESS_CREATE_THREAD
            | PROCESS_QUERY_INFORMATION
            | PROCESS_VM_OPERATION
            | PROCESS_VM_READ
            | PROCESS_VM_WRITE;
        // SAFETY: documented OR of PROCESS_* rights; handle closed on drop.
        let proc = unsafe { OpenProcess(access, false, pid) }
            .map_err(|_| HostError::last_win32("OpenProcess"))?;
        let _proc = OwnedHandle(proc);

        // SAFETY: handle valid, size non-zero, flags documented.
        let remote_buf = unsafe {
            VirtualAllocEx(
                proc,
                None,
                wide_bytes,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_READWRITE,
            )
        };
        if remote_buf.is_null() {
            return Err(HostError::last_win32("VirtualAllocEx"));
        }
        let alloc_guard = RemoteAlloc {
            proc,
            ptr: remote_buf,
        };
        debug!(
            pid,
            ?remote_buf,
            bytes = wide_bytes,
            "allocated remote buffer"
        );

        let mut written: usize = 0;
        // SAFETY: ptrs and lengths match the allocation; proc is VM_WRITE.
        unsafe {
            WriteProcessMemory(
                proc,
                remote_buf,
                wide.as_ptr() as *const c_void,
                wide_bytes,
                Some(&mut written),
            )
            .map_err(|_| HostError::last_win32("WriteProcessMemory"))?;
        }
        if written != wide_bytes {
            return Err(HostError::InjectionFailed(format!(
                "WriteProcessMemory short write: {written}/{wide_bytes}"
            )));
        }

        // LoadLibraryW(LPCWSTR) -> HMODULE is ABI-compatible with the thread
        // start routine `extern "system" fn(*mut c_void) -> u32` (the HMODULE
        // is truncated to the low 32 bits in the exit code; LPCWSTR is a
        // pointer, same width as *mut c_void on x64).
        // SAFETY: both are `extern "system"`; the truncation is intentional.
        let start_routine: LPTHREAD_START_ROUTINE = Some(unsafe {
            std::mem::transmute::<
                unsafe extern "system" fn(PCWSTR) -> windows::Win32::Foundation::HMODULE,
                unsafe extern "system" fn(*mut c_void) -> u32,
            >(load_library_w_addr)
        });

        // SAFETY: proc has CREATE_THREAD; start_routine is LoadLibraryW;
        // remote_buf is a valid pointer we just wrote a wide string into.
        let thread = unsafe {
            CreateRemoteThread(
                proc,
                None,
                0,
                start_routine,
                Some(remote_buf as *const c_void),
                0,
                None,
            )
        }
        .map_err(|_| HostError::last_win32("CreateRemoteThread"))?;
        let thread_guard = OwnedHandle(thread);

        // SAFETY: thread handle owned; INFINITE documented.
        let wait = unsafe { WaitForSingleObject(thread, INFINITE) };
        if wait != WAIT_OBJECT_0 {
            return Err(HostError::last_win32("WaitForSingleObject"));
        }

        let mut exit_code: u32 = 0;
        // SAFETY: thread handle valid; &mut exit_code is a valid u32 ptr.
        unsafe { GetExitCodeThread(thread, &mut exit_code) }
            .map_err(|_| HostError::last_win32("GetExitCodeThread"))?;

        drop(thread_guard);
        drop(alloc_guard);

        if exit_code == 0 {
            // The exit code is the low 32 bits of the HMODULE; on the rare
            // chance those are zero for a successful load, confirm via the
            // module list before declaring failure.
            if matches!(is_dll_loaded(pid, &dll_file_name), Ok(true)) {
                info!(
                    pid,
                    "LoadLibraryW exit was 0 but module is loaded; treating as success"
                );
                return Ok(());
            }
            return Err(HostError::InjectionFailed(
                "LoadLibraryW returned NULL in target process (DLL failed to load)".into(),
            ));
        }

        info!(pid, dll = %dll_path.display(), exit_code, "injection succeeded");
        Ok(())
    }
}

/// Resolve `LoadLibraryW` in our own process.
fn resolve_load_library_w()
-> Result<unsafe extern "system" fn(PCWSTR) -> windows::Win32::Foundation::HMODULE> {
    // SAFETY: `w!` produces a static, null-terminated wide literal.
    let h_kernel32 = unsafe { GetModuleHandleW(w!("kernel32.dll")) }
        .map_err(|_| HostError::last_win32("GetModuleHandleW(kernel32.dll)"))?;
    // SAFETY: literal is null-terminated; HMODULE is valid.
    let proc = unsafe { GetProcAddress(h_kernel32, windows::core::s!("LoadLibraryW")) };
    let Some(proc) = proc else {
        return Err(HostError::last_win32("GetProcAddress(LoadLibraryW)"));
    };
    // SAFETY: resolved "LoadLibraryW" from kernel32; signature matches on x64.
    let typed: unsafe extern "system" fn(PCWSTR) -> windows::Win32::Foundation::HMODULE =
        unsafe { std::mem::transmute(proc) };
    Ok(typed)
}

/// True iff `target_pid` has a loaded module whose file-name matches
/// `dll_file_name` (case-insensitive).
pub(crate) fn is_dll_loaded(target_pid: u32, dll_file_name: &str) -> Result<bool> {
    // SAFETY: documented args; snapshot closed via OwnedHandle.
    let snap =
        unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, target_pid) }
            .map_err(|_| HostError::last_win32("CreateToolhelp32Snapshot(MODULE)"))?;
    let _snap = OwnedHandle(snap);

    let mut entry = MODULEENTRY32W {
        dwSize: std::mem::size_of::<MODULEENTRY32W>() as u32,
        ..Default::default()
    };
    // SAFETY: snapshot live; entry has dwSize set.
    if unsafe { Module32FirstW(snap, &mut entry) }.is_err() {
        return Ok(false);
    }
    loop {
        let name = wide_to_string(&entry.szModule);
        if name.eq_ignore_ascii_case(dll_file_name) {
            return Ok(true);
        }
        // SAFETY: snapshot still live; entry still valid.
        if unsafe { Module32NextW(snap, &mut entry) }.is_err() {
            return Ok(false);
        }
    }
}

fn wide_to_string(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// Drop guard: closes a HANDLE on drop.
struct OwnedHandle(HANDLE);
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: we own this handle.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

/// Drop guard: `VirtualFreeEx`'s the remote allocation when scoped out.
struct RemoteAlloc {
    proc: HANDLE,
    ptr: *mut c_void,
}
impl Drop for RemoteAlloc {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            // SAFETY: `proc` is a valid VM_OPERATION handle; `ptr` came from
            // VirtualAllocEx in the same process.
            unsafe {
                let _ = VirtualFreeEx(self.proc, self.ptr, 0, MEM_RELEASE);
            }
        }
    }
}

use std::os::windows::ffi::OsStrExt;
