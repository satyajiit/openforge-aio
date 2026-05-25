//! Enumerate the target process's writable, non-executable memory pages.
//!
//! Used as the haystack for first scans. Excludes executable code pages, guard
//! pages, free / reserved regions, and uncommitted ranges.

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;

    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Memory::{
        MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE, PAGE_EXECUTE_READ,
        PAGE_EXECUTE_READWRITE, PAGE_EXECUTE_WRITECOPY, PAGE_GUARD, PAGE_NOACCESS, PAGE_READWRITE,
        PAGE_WRITECOPY, VirtualQueryEx,
    };

    #[derive(Debug, Clone, Copy)]
    pub struct MemoryRegion {
        pub base: usize,
        pub size: usize,
    }

    pub fn enumerate_rw(handle: HANDLE) -> Vec<MemoryRegion> {
        let mut out = Vec::new();
        let mut addr: usize = 0;
        let mbi_size = std::mem::size_of::<MEMORY_BASIC_INFORMATION>();
        loop {
            let mut mbi = MEMORY_BASIC_INFORMATION::default();
            let written =
                unsafe { VirtualQueryEx(handle, Some(addr as *const c_void), &mut mbi, mbi_size) };
            if written == 0 {
                break;
            }
            let base = mbi.BaseAddress as usize;
            let region_size = mbi.RegionSize;
            if region_size == 0 {
                break;
            }

            let is_committed = mbi.State == MEM_COMMIT;
            let prot = mbi.Protect.0;
            let is_rw = prot == PAGE_READWRITE.0 || prot == PAGE_WRITECOPY.0;
            let is_exec = prot
                & (PAGE_EXECUTE.0
                    | PAGE_EXECUTE_READ.0
                    | PAGE_EXECUTE_READWRITE.0
                    | PAGE_EXECUTE_WRITECOPY.0)
                != 0;
            let is_guard = prot & PAGE_GUARD.0 != 0;
            let is_no_access = prot == PAGE_NOACCESS.0;

            if is_committed && is_rw && !is_exec && !is_guard && !is_no_access {
                out.push(MemoryRegion {
                    base,
                    size: region_size,
                });
            }

            let next = base.saturating_add(region_size);
            if next <= addr {
                break;
            }
            addr = next;
        }
        out
    }
}

#[cfg(not(windows))]
mod imp {
    #[derive(Debug, Clone, Copy)]
    pub struct MemoryRegion {
        pub base: usize,
        pub size: usize,
    }

    pub fn enumerate_rw(_handle: ()) -> Vec<MemoryRegion> {
        Vec::new()
    }
}

pub use imp::*;
