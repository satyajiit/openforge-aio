//! Process enumeration and handle management (Windows).

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;

use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    INFINITE, OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_VM_OPERATION,
    PROCESS_VM_READ, PROCESS_VM_WRITE, WaitForSingleObject,
};

use crate::error::{Error, Result};

#[derive(Debug, Clone)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct ProcessSnapshot {
    pub entries: Vec<ProcessInfo>,
}

impl ProcessSnapshot {
    pub fn capture() -> Result<Self> {
        let mut entries = Vec::new();
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
                .map_err(|_| Error::win32("CreateToolhelp32Snapshot(PROCESS)"))?;
            let mut entry = PROCESSENTRY32W {
                dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
                ..Default::default()
            };
            if Process32FirstW(snap, &mut entry).is_ok() {
                loop {
                    let name = from_wchar_buf(&entry.szExeFile);
                    entries.push(ProcessInfo {
                        pid: entry.th32ProcessID,
                        name,
                    });
                    if Process32NextW(snap, &mut entry).is_err() {
                        break;
                    }
                }
            }
            let _ = CloseHandle(snap);
        }
        Ok(Self { entries })
    }

    pub fn find_by_name(&self, name: &str) -> Option<&ProcessInfo> {
        self.entries
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))
    }

    pub fn find_by_candidates(&self, candidates: &[&str]) -> Option<&ProcessInfo> {
        candidates.iter().find_map(|c| self.find_by_name(c))
    }
}

/// Owned process handle. Closes the handle on drop.
pub struct ProcessHandle {
    handle: HANDLE,
}

impl ProcessHandle {
    pub fn open(pid: u32) -> Result<Self> {
        let access =
            PROCESS_VM_READ | PROCESS_VM_WRITE | PROCESS_VM_OPERATION | PROCESS_QUERY_INFORMATION;
        let handle =
            unsafe { OpenProcess(access, false, pid) }.map_err(|_| Error::win32("OpenProcess"))?;
        Ok(Self { handle })
    }

    pub fn raw(&self) -> HANDLE {
        self.handle
    }
}

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

// SAFETY: A Win32 HANDLE is a kernel-managed integer. It can be moved across
// threads and used from any thread that owns it. We never share `&mut HANDLE`
// across threads from this type.
unsafe impl Send for ProcessHandle {}
unsafe impl Sync for ProcessHandle {}

/// Block the calling thread until the given process exits.
///
/// Opens a fresh `SYNCHRONIZE`-only handle (independent of the trainer's
/// existing read/write handle) and waits on it with `INFINITE`. Returns `Ok(())`
/// when the process dies, and `Err` only if the handle can't be opened (most
/// likely because the process is already gone — treat that as "exited"
/// upstream).
///
/// MUST be called from a blocking context (e.g. `spawn_blocking`). The wait
/// holds an OS thread for the lifetime of the target process.
pub fn wait_for_pid_exit(pid: u32) -> Result<()> {
    let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) }
        .map_err(|_| Error::win32("OpenProcess(SYNCHRONIZE)"))?;
    let rc = unsafe { WaitForSingleObject(handle, INFINITE) };
    unsafe {
        let _ = CloseHandle(handle);
    }
    if rc == WAIT_OBJECT_0 {
        Ok(())
    } else {
        Err(Error::win32("WaitForSingleObject"))
    }
}

pub(crate) fn from_wchar_buf(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    OsString::from_wide(&buf[..end])
        .to_string_lossy()
        .into_owned()
}
