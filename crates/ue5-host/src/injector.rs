//! `CreateRemoteThread(LoadLibraryW)` DLL injection.
//!
//! Game-agnostic: the caller supplies the DLL path; this module derives the
//! file name from that path and uses it for idempotency checks. The host
//! crate does not bake in any specific game's DLL name.
//!
//! Idempotent: re-running [`Injector::inject`] for a pid that already has the
//! DLL loaded is a no-op (returns `Ok(())`). We discover that by enumerating
//! the target's loaded modules with `CreateToolhelp32Snapshot(TH32CS_SNAPMODULE
//! | TH32CS_SNAPMODULE32)` before doing any work.

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

/// One-shot DLL injector. Stateless — kept as a unit type for a tidy public
/// API (`Injector::inject(...)`) and to leave room for future state without a
/// breaking change.
pub struct Injector;

/// Extract the file-name component from a DLL path, returning an error if the
/// path has no file name or the name isn't valid UTF-8. The host's
/// idempotency check (`is_dll_loaded`) matches against this string against
/// `MODULEENTRY32W.szModule`.
fn dll_file_name_from_path(dll_path: &Path) -> Result<String> {
    match dll_path.file_name().and_then(|s| s.to_str()) {
        Some(name) if !name.is_empty() => Ok(name.to_owned()),
        _ => Err(HostError::DllNotFound(dll_path.to_path_buf())),
    }
}

impl Injector {
    /// Inject `dll_path` into the process identified by `pid` using
    /// `CreateRemoteThread(LoadLibraryW)`.
    ///
    /// Idempotent: if the DLL named `dll_path.file_name()` is already loaded
    /// into the target (case-insensitive match), this returns `Ok(())`
    /// without re-injecting.
    pub fn inject(pid: u32, dll_path: &Path) -> Result<()> {
        if !dll_path.is_file() {
            return Err(HostError::DllNotFound(dll_path.to_path_buf()));
        }
        let dll_file_name = dll_file_name_from_path(dll_path)?;

        // Fast path: already loaded?
        match is_dll_loaded(pid, &dll_file_name) {
            Ok(true) => {
                info!(pid, dll = %dll_path.display(), "DLL already loaded; injection skipped");
                return Ok(());
            }
            Ok(false) => {}
            Err(e) => {
                // Module enumeration is best-effort. If it fails we still try
                // to inject — worst case the second LoadLibraryW returns the
                // existing HMODULE and we no-op.
                warn!(
                    ?e,
                    pid, "module enumeration failed; proceeding with injection anyway"
                );
            }
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

        // Resolve LoadLibraryW in *our* process. The kernel32 base is the same
        // across processes in the same Windows session, so the address is
        // valid for the remote `CreateRemoteThread` start routine.
        let load_library_w_addr = resolve_load_library_w()?;

        // Open the target with the rights we need for VM ops + a remote
        // thread.
        let access = PROCESS_CREATE_THREAD
            | PROCESS_QUERY_INFORMATION
            | PROCESS_VM_OPERATION
            | PROCESS_VM_READ
            | PROCESS_VM_WRITE;
        // SAFETY: pid is a u32 from the caller; `access` is a documented
        // OR of PROCESS_* rights; bInheritHandle=false; handle is closed on
        // drop of `_proc`.
        let proc = unsafe { OpenProcess(access, false, pid) }
            .map_err(|_| HostError::last_win32("OpenProcess"))?;
        let _proc = OwnedHandle(proc);

        // Allocate enough room for the wide-encoded path in the target.
        // SAFETY: handle is valid, size is non-zero, flags are documented.
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

        // Write the path into the remote allocation.
        let mut written: usize = 0;
        // SAFETY: ptrs and lengths match the allocation; `proc` is a valid
        // PROCESS_VM_WRITE handle.
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

        // Build the LPTHREAD_START_ROUTINE pointer. `LoadLibraryW` has the
        // signature `HMODULE LoadLibraryW(LPCWSTR)`. The ABI is compatible
        // with `unsafe extern "system" fn(*mut c_void) -> u32` (HMODULE is
        // truncated to the low 32 bits in the thread exit code; LPCWSTR is a
        // pointer, same width as `*mut c_void` on x64).
        // SAFETY: We transmute a function pointer of equivalent calling
        // convention. Both are `extern "system"`. The truncation in the
        // return type is intentional and documented at the call site.
        let start_routine: LPTHREAD_START_ROUTINE = Some(unsafe {
            std::mem::transmute::<
                unsafe extern "system" fn(PCWSTR) -> windows::Win32::Foundation::HMODULE,
                unsafe extern "system" fn(*mut c_void) -> u32,
            >(load_library_w_addr)
        });

        // Spawn the remote thread, passing the remote buffer as its parameter.
        // SAFETY: `proc` has PROCESS_CREATE_THREAD; `start_routine` is a
        // valid `LoadLibraryW`; `remote_buf` is a valid pointer in the
        // target's address space we just wrote a null-terminated wide string
        // into.
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

        // Wait for LoadLibraryW to complete in the target.
        // SAFETY: thread handle owned, INFINITE is documented.
        let wait = unsafe { WaitForSingleObject(thread, INFINITE) };
        if wait != WAIT_OBJECT_0 {
            return Err(HostError::last_win32("WaitForSingleObject"));
        }

        // GetExitCodeThread returns the low 32 bits of the HMODULE on x64.
        // A non-zero value means LoadLibraryW returned a non-null HMODULE
        // (success). A zero value means LoadLibraryW returned NULL (failure).
        // We cannot recover the full 64-bit HMODULE from this — that's a
        // documented limitation of CreateRemoteThread(LoadLibraryW).
        let mut exit_code: u32 = 0;
        // SAFETY: thread handle is valid; `&mut exit_code` is a valid u32 ptr.
        unsafe { GetExitCodeThread(thread, &mut exit_code) }
            .map_err(|_| HostError::last_win32("GetExitCodeThread"))?;

        // Drop guards explicitly so we close handles / free memory in order.
        drop(thread_guard);
        drop(alloc_guard);

        if exit_code == 0 {
            // Double-check via the module list before declaring failure —
            // some images load by side-effect of dependencies and the exit
            // code semantics can be subtle on Win11 with very large HMODULEs
            // whose low 32 bits happen to be zero (extremely unlikely but
            // worth catching).
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

impl Injector {
    /// Try to eject the current DLL image and then inject `dll_path`. Useful
    /// for dev iteration when you've rebuilt the DLL and need the target
    /// process to pick up the new image without a game restart.
    ///
    /// If the eject reports `false` (worker thread holding refcount), this
    /// returns `Err(HostError::InjectionFailed("..."))` rather than silently
    /// reusing the stale image — the caller is expected to restart the game.
    pub fn reinject(pid: u32, dll_path: &Path) -> Result<()> {
        let dll_file_name = dll_file_name_from_path(dll_path)?;
        match Self::eject(pid, &dll_file_name) {
            Ok(true) => {
                info!(pid, "old DLL ejected; injecting new image");
                Self::inject(pid, dll_path)
            }
            Ok(false) => Err(HostError::InjectionFailed(
                "eject ran but the DLL is still loaded (worker thread holding refcount); \
                 restart the game and re-run with `inject`"
                    .into(),
            )),
            Err(e) => Err(e),
        }
    }
}

impl Injector {
    /// Attempt to unload the DLL named `dll_file_name` from the target
    /// process via `CreateRemoteThread(FreeLibrary, hmodule)`.
    ///
    /// **Important caveat**: `FreeLibrary` only unloads the DLL when its
    /// reference count drops to zero. Because the DLL spawns a worker thread
    /// that loops forever inside its own image, that thread keeps the
    /// reference count at >=1 and the DLL stays loaded. This method still
    /// runs (the remote `FreeLibrary` call succeeds — the worker just keeps
    /// running). The intent is to let dev tooling try a clean unload first
    /// before falling back to "ask the user to restart the game".
    ///
    /// Returns:
    /// * `Ok(true)`  — `FreeLibrary` ran and `is_dll_loaded` now reports false.
    ///   The DLL is actually gone and a fresh `inject` will load a new image.
    /// * `Ok(false)` — `FreeLibrary` ran but the module is still listed
    ///   (worker thread or other refcount keeping it alive). A new `inject`
    ///   will be skipped by `is_dll_loaded`.
    /// * `Err(_)`    — Win32 / handle failure during the remote-thread dance.
    pub fn eject(pid: u32, dll_file_name: &str) -> Result<bool> {
        let hmod = match find_module_handle(pid, dll_file_name)? {
            Some(h) => h,
            None => return Ok(true), // already not loaded
        };

        let free_library_addr = resolve_free_library()?;

        let access = PROCESS_CREATE_THREAD
            | PROCESS_QUERY_INFORMATION
            | PROCESS_VM_OPERATION
            | PROCESS_VM_READ
            | PROCESS_VM_WRITE;
        // SAFETY: documented OR of PROCESS_* rights; closed via guard below.
        let proc = unsafe { OpenProcess(access, false, pid) }
            .map_err(|_| HostError::last_win32("OpenProcess(eject)"))?;
        let _proc = OwnedHandle(proc);

        // FreeLibrary signature: `BOOL FreeLibrary(HMODULE)`. ABI-compatible
        // with `unsafe extern "system" fn(*mut c_void) -> u32`.
        // SAFETY: same calling convention; HMODULE is pointer-width.
        let start_routine: LPTHREAD_START_ROUTINE = Some(unsafe {
            std::mem::transmute::<
                unsafe extern "system" fn(
                    windows::Win32::Foundation::HMODULE,
                ) -> windows::core::BOOL,
                unsafe extern "system" fn(*mut c_void) -> u32,
            >(free_library_addr)
        });

        // SAFETY: proc has PROCESS_CREATE_THREAD; start_routine is FreeLibrary;
        // hmod.0 is the HMODULE in the target's address space.
        let thread = unsafe {
            CreateRemoteThread(
                proc,
                None,
                0,
                start_routine,
                Some(hmod.0 as *const c_void),
                0,
                None,
            )
        }
        .map_err(|_| HostError::last_win32("CreateRemoteThread(FreeLibrary)"))?;
        let _thread_guard = OwnedHandle(thread);

        // SAFETY: thread handle valid; INFINITE is documented.
        let wait = unsafe { WaitForSingleObject(thread, INFINITE) };
        if wait != WAIT_OBJECT_0 {
            return Err(HostError::last_win32("WaitForSingleObject(eject)"));
        }
        // Pacing: the loader updates the module list lazily after FreeLibrary
        // returns from the remote thread. Sleep briefly so a subsequent
        // `is_dll_loaded` check sees the post-unload state.
        std::thread::sleep(std::time::Duration::from_millis(100));

        let still_loaded = is_dll_loaded(pid, dll_file_name).unwrap_or(true);
        if still_loaded {
            warn!(
                pid,
                "FreeLibrary completed but DLL still listed (likely worker thread holding refcount)"
            );
            Ok(false)
        } else {
            info!(pid, "DLL ejected from target");
            Ok(true)
        }
    }
}

/// Look up the HMODULE base address of `dll_file_name` in the target process,
/// reading it out of the Toolhelp module snapshot. Returns `Ok(None)` if the
/// DLL isn't loaded.
fn find_module_handle(
    target_pid: u32,
    dll_file_name: &str,
) -> Result<Option<windows::Win32::Foundation::HMODULE>> {
    // SAFETY: documented args; snapshot handle is closed via OwnedHandle.
    let snap =
        unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, target_pid) }
            .map_err(|_| HostError::last_win32("CreateToolhelp32Snapshot(MODULE)"))?;
    let _snap = OwnedHandle(snap);

    let mut entry = MODULEENTRY32W {
        dwSize: std::mem::size_of::<MODULEENTRY32W>() as u32,
        ..Default::default()
    };
    // SAFETY: snapshot live, entry has dwSize set.
    if unsafe { Module32FirstW(snap, &mut entry) }.is_err() {
        return Ok(None);
    }
    loop {
        let name = wide_to_string(&entry.szModule);
        if name.eq_ignore_ascii_case(dll_file_name) {
            return Ok(Some(windows::Win32::Foundation::HMODULE(
                entry.modBaseAddr as *mut c_void,
            )));
        }
        // SAFETY: snapshot still live, entry still valid.
        if unsafe { Module32NextW(snap, &mut entry) }.is_err() {
            return Ok(None);
        }
    }
}

fn resolve_free_library()
-> Result<unsafe extern "system" fn(windows::Win32::Foundation::HMODULE) -> windows::core::BOOL> {
    // SAFETY: `w!` produces a static null-terminated wide literal.
    let h_kernel32 = unsafe { GetModuleHandleW(w!("kernel32.dll")) }
        .map_err(|_| HostError::last_win32("GetModuleHandleW(kernel32.dll)"))?;
    // SAFETY: literal is null-terminated; HMODULE is valid.
    let proc = unsafe { GetProcAddress(h_kernel32, windows::core::s!("FreeLibrary")) };
    let Some(proc) = proc else {
        return Err(HostError::last_win32("GetProcAddress(FreeLibrary)"));
    };
    // SAFETY: resolved symbol "FreeLibrary" from kernel32; signature matches.
    Ok(unsafe {
        std::mem::transmute::<
            unsafe extern "system" fn() -> isize,
            unsafe extern "system" fn(windows::Win32::Foundation::HMODULE) -> windows::core::BOOL,
        >(proc)
    })
}

/// Look up `LoadLibraryW` in our own process. The kernel32 base address is
/// shared across all processes in a Windows session, so the resolved pointer
/// is directly usable as a remote thread start routine.
fn resolve_load_library_w()
-> Result<unsafe extern "system" fn(PCWSTR) -> windows::Win32::Foundation::HMODULE> {
    // SAFETY: `w!` produces a static, null-terminated wide literal.
    let h_kernel32 = unsafe { GetModuleHandleW(w!("kernel32.dll")) }
        .map_err(|_| HostError::last_win32("GetModuleHandleW(kernel32.dll)"))?;
    // SAFETY: literal is null-terminated; HMODULE is valid (we just got it).
    let proc = unsafe { GetProcAddress(h_kernel32, windows::core::s!("LoadLibraryW")) };
    let Some(proc) = proc else {
        return Err(HostError::last_win32("GetProcAddress(LoadLibraryW)"));
    };
    // SAFETY: We resolved the symbol named "LoadLibraryW" from kernel32.dll;
    // the function pointer type matches its public signature on x64.
    let typed: unsafe extern "system" fn(PCWSTR) -> windows::Win32::Foundation::HMODULE =
        unsafe { std::mem::transmute(proc) };
    Ok(typed)
}

/// True iff `target_pid` has a loaded module whose file-name matches
/// `dll_file_name` (case-insensitive). Walks `MODULEENTRY32W` from a
/// `CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32)` so we
/// see both native (x64) and Wow64 modules — irrelevant for UE5 games (always
/// x64) but cheap insurance.
pub(crate) fn is_dll_loaded(target_pid: u32, dll_file_name: &str) -> Result<bool> {
    // SAFETY: documented args; snapshot handle is closed via OwnedHandle.
    let snap =
        unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, target_pid) }
            .map_err(|_| HostError::last_win32("CreateToolhelp32Snapshot(MODULE)"))?;
    let _snap = OwnedHandle(snap);

    let mut entry = MODULEENTRY32W {
        dwSize: std::mem::size_of::<MODULEENTRY32W>() as u32,
        ..Default::default()
    };
    // SAFETY: snapshot handle is live; entry has dwSize set.
    if unsafe { Module32FirstW(snap, &mut entry) }.is_err() {
        // Empty snapshot — treat as "not loaded" but don't error: pid may
        // be exiting.
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

/// Drop guard: closes a HANDLE on drop. Used for both process and thread
/// handles in this module.
struct OwnedHandle(HANDLE);
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: we own this handle (returned by an API that gave us
            // ownership); CloseHandle is the documented disposal API.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}
impl std::ops::Deref for OwnedHandle {
    type Target = HANDLE;
    fn deref(&self) -> &HANDLE {
        &self.0
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
            // SAFETY: `proc` is a valid PROCESS_VM_OPERATION handle; `ptr` is
            // the value we got from VirtualAllocEx in the same process.
            unsafe {
                let _ = VirtualFreeEx(self.proc, self.ptr, 0, MEM_RELEASE);
            }
        }
    }
}

// `encode_wide` lives on `OsStrExt`.
use std::os::windows::ffi::OsStrExt;
