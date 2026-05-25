//! Module enumeration + PE `.text` section discovery for an attached process.

use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, MODULEENTRY32W, Module32FirstW, Module32NextW, TH32CS_SNAPMODULE,
    TH32CS_SNAPMODULE32,
};

use crate::error::{Error, Result};
use crate::memory;
use crate::process::{ProcessHandle, from_wchar_buf};

#[derive(Debug, Clone)]
pub struct Module {
    pub name: String,
    pub base: usize,
    pub size: usize,
    /// RVA (relative to `base`) of the `.text` section.
    pub text_offset: usize,
    pub text_size: usize,
}

impl Module {
    pub fn text_base(&self) -> usize {
        self.base + self.text_offset
    }

    pub fn contains(&self, addr: usize) -> bool {
        addr >= self.base && addr < self.base.saturating_add(self.size)
    }
}

pub fn enumerate(pid: u32, proc: &ProcessHandle) -> Result<Vec<Module>> {
    let mut modules = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid)
            .map_err(|_| Error::win32("CreateToolhelp32Snapshot(MODULE)"))?;
        let mut entry = MODULEENTRY32W {
            dwSize: std::mem::size_of::<MODULEENTRY32W>() as u32,
            ..Default::default()
        };
        if Module32FirstW(snap, &mut entry).is_ok() {
            loop {
                let name = from_wchar_buf(&entry.szModule);
                let base = entry.modBaseAddr as usize;
                let size = entry.modBaseSize as usize;
                let (text_offset, text_size) = parse_text_section(proc, base).unwrap_or((0, size));
                modules.push(Module {
                    name,
                    base,
                    size,
                    text_offset,
                    text_size,
                });
                if Module32NextW(snap, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    Ok(modules)
}

fn parse_text_section(proc: &ProcessHandle, base: usize) -> Result<(usize, usize)> {
    // DOS header: PE offset lives at +0x3C
    let mut e_lfanew = [0u8; 4];
    memory::read_bytes(proc, base + 0x3C, &mut e_lfanew)?;
    let pe_off = u32::from_le_bytes(e_lfanew) as usize;
    let pe_addr = base + pe_off;

    let mut sig = [0u8; 4];
    memory::read_bytes(proc, pe_addr, &mut sig)?;
    if &sig != b"PE\0\0" {
        return Err(Error::PeParse {
            module: format!("0x{base:X}"),
            reason: "missing PE signature".into(),
        });
    }

    let coff_addr = pe_addr + 4;
    let mut coff = [0u8; 20];
    memory::read_bytes(proc, coff_addr, &mut coff)?;
    let num_sections = u16::from_le_bytes([coff[2], coff[3]]) as usize;
    let opt_header_size = u16::from_le_bytes([coff[16], coff[17]]) as usize;
    let opt_header_addr = coff_addr + 20;
    let sections_addr = opt_header_addr + opt_header_size;

    for i in 0..num_sections {
        let sect_addr = sections_addr + i * 40;
        let mut sect = [0u8; 40];
        memory::read_bytes(proc, sect_addr, &mut sect)?;
        let name_end = sect[..8].iter().position(|&c| c == 0).unwrap_or(8);
        let name = std::str::from_utf8(&sect[..name_end]).unwrap_or("");
        if name == ".text" {
            let vsize = u32::from_le_bytes([sect[8], sect[9], sect[10], sect[11]]) as usize;
            let vaddr = u32::from_le_bytes([sect[12], sect[13], sect[14], sect[15]]) as usize;
            return Ok((vaddr, vsize));
        }
    }

    Err(Error::PeParse {
        module: format!("0x{base:X}"),
        reason: ".text section not found".into(),
    })
}
