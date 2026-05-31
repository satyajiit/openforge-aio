//! In-process PE introspection — main-module discovery + module enumeration
//! via Toolhelp32, with per-module `.text` section parsing (used by the AOB
//! scan op to bound its search to executable code, and to build the
//! `openforge_core::Module` list the in-process `Ctx` hands out).

use std::mem;

use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, MODULEENTRY32W, Module32FirstW, Module32NextW, TH32CS_SNAPMODULE,
    TH32CS_SNAPMODULE32,
};
use windows::Win32::System::Threading::GetCurrentProcessId;

/// One enumerated module in our own process. Mirrors `openforge_core::Module`
/// and the wire-side `openforge_glacier_protocol::ModuleEntry` 1:1.
#[derive(Clone, Debug)]
pub struct LoadedModule {
    pub name: String,
    pub base: usize,
    pub size: usize,
    /// RVA of the `.text` section (`0` if the PE has no parseable `.text`).
    pub text_offset: usize,
    pub text_size: usize,
}

impl LoadedModule {
    pub fn text_base(&self) -> usize {
        self.base + self.text_offset
    }
}

/// Enumerate every loaded module in the current process via Toolhelp32, plus
/// each module's `.text` section RVA + size. Toolhelp reports the EXE first,
/// so `out[0]` is the main module.
///
/// `parse_text_section` reads the PE headers in-memory (we're inside the
/// process, so the image is mapped); failures fall back to `(0, 0)` so the
/// module is still reported with module-level bounds but `.text` ops against
/// it find nothing.
pub fn enumerate_modules() -> Vec<LoadedModule> {
    let mut out = Vec::new();
    let pid = unsafe { GetCurrentProcessId() };
    // SAFETY: documented args; snapshot handle is closed below.
    let snap =
        match unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid) } {
            Ok(h) => h,
            Err(_) => return out,
        };
    let mut entry = MODULEENTRY32W {
        dwSize: mem::size_of::<MODULEENTRY32W>() as u32,
        ..Default::default()
    };
    // SAFETY: snap is live, entry has dwSize set.
    if unsafe { Module32FirstW(snap, &mut entry) }.is_ok() {
        loop {
            let name = wide_to_string(&entry.szModule);
            let base = entry.modBaseAddr as usize;
            let size = entry.modBaseSize as usize;
            let (text_offset, text_size) = parse_text_section(base, size).unwrap_or((0, 0));
            out.push(LoadedModule {
                name,
                base,
                size,
                text_offset,
                text_size,
            });
            // SAFETY: snap live, entry valid.
            if unsafe { Module32NextW(snap, &mut entry) }.is_err() {
                break;
            }
        }
    }
    // SAFETY: snap is a handle we own.
    unsafe {
        let _ = CloseHandle(snap);
    }
    out
}

fn wide_to_string(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// Parse the in-memory PE headers at `base` to find the `.text` section.
/// Returns `(text_rva, text_size)`. Every read is bounded by the loader-
/// reported `size`; out-of-bounds returns `None`.
fn parse_text_section(base: usize, size: usize) -> Option<(usize, usize)> {
    // SAFETY: We're parsing in-memory PE bytes. Every read is bounded by
    // `size`. The cdylib runs after the loader has mapped the target image,
    // so the headers are valid; any read that would fall outside the image
    // returns `None`.
    unsafe {
        let img = base as *const u8;
        if size < 0x40 {
            return None;
        }
        let e_lfanew = read_u32_in_image(img, size, 0x3C)? as usize;
        if e_lfanew.checked_add(24)? > size {
            return None;
        }
        let sig = read_u32_in_image(img, size, e_lfanew)?;
        if sig != 0x0000_4550 {
            return None;
        }
        let coff = e_lfanew + 4;
        let num_sections = read_u16_in_image(img, size, coff + 2)? as usize;
        let opt_header_size = read_u16_in_image(img, size, coff + 16)? as usize;
        let sections_off = coff + 20 + opt_header_size;
        if sections_off + 40usize.checked_mul(num_sections)? > size {
            return None;
        }
        for i in 0..num_sections {
            let s = sections_off + i * 40;
            let mut name = [0u8; 8];
            for (k, slot) in name.iter_mut().enumerate() {
                *slot = *img.add(s + k);
            }
            let name_end = name.iter().position(|&c| c == 0).unwrap_or(8);
            let name_str = std::str::from_utf8(&name[..name_end]).unwrap_or("");
            if name_str == ".text" {
                let vsize = read_u32_in_image(img, size, s + 8)? as usize;
                let vaddr = read_u32_in_image(img, size, s + 12)? as usize;
                return Some((vaddr, vsize));
            }
        }
        None
    }
}

#[inline]
unsafe fn read_u32_in_image(img: *const u8, size: usize, off: usize) -> Option<u32> {
    if off + 4 > size {
        return None;
    }
    // SAFETY: bounds checked above.
    let p = unsafe { img.add(off) } as *const u32;
    Some(unsafe { core::ptr::read_unaligned(p) })
}

#[inline]
unsafe fn read_u16_in_image(img: *const u8, size: usize, off: usize) -> Option<u16> {
    if off + 2 > size {
        return None;
    }
    let p = unsafe { img.add(off) } as *const u16;
    Some(unsafe { core::ptr::read_unaligned(p) })
}
