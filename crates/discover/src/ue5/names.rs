//! Production-ready, cached FName decoder with wide-character and suffix support.
//!
//! The name cache lives on `UeEngine` (interior-mutex'd `HashMap<u32, String>`).
//! Engine instances are short-lived per CLI invocation, so cross-run sharing is
//! intentionally not provided.

use anyhow::{Result, bail};
use openforge_core::Ctx;

use crate::ue5::locate::UeEngine;

impl UeEngine {
    pub fn decode_fname(&self, comparison_index: u32, number: u32) -> Result<String> {
        let base_name = self.lookup_pool(comparison_index)?;
        if number != 0 {
            Ok(format!("{}_{}", base_name, number - 1))
        } else {
            Ok(base_name)
        }
    }

    fn lookup_pool(&self, comparison_index: u32) -> Result<String> {
        {
            let cache = self.name_cache.lock().unwrap();
            if let Some(cached) = cache.get(&comparison_index) {
                return Ok(cached.clone());
            }
        }

        let chunk_index = (comparison_index >> 16) as usize;
        let byte_offset = ((comparison_index & 0xFFFF) << 1) as usize;

        // FNamePool structure (UE5; layout drifts across minor versions):
        // +0x00: Mutex/Lock (8 bytes)
        // +0x?? : CurrentNumElements / CurrentNumChunks counters
        // +chunks_offset: Chunks (array of 8-byte pointers, max 8192)
        //
        // `self.fname_pool_chunks_offset` was probed and locked in by
        // `locate::find_fname_pool` (one of {0x08, 0x10, 0x18, 0x20}).
        let mut chunks_header = [0u8; 8];
        self.process.read_bytes(
            self.fname_pool + self.fname_pool_chunks_offset,
            &mut chunks_header,
        )?;
        let chunks_addr = u64::from_le_bytes(chunks_header) as usize;

        let mut chunk_base_buf = [0u8; 8];
        self.process
            .read_bytes(chunks_addr + chunk_index * 8, &mut chunk_base_buf)?;
        let chunk_base = u64::from_le_bytes(chunk_base_buf) as usize;
        if chunk_base == 0 {
            bail!("FNamePool chunk pointer is null at index {}", chunk_index);
        }

        let entry_addr = chunk_base + byte_offset;
        let mut header_buf = [0u8; 2];
        self.process.read_bytes(entry_addr, &mut header_buf)?;
        let header = u16::from_le_bytes(header_buf);

        // Header format (UE5):
        // Bit 0: IsWide
        // Bits 6..16: Len (10 bits) -- (header >> 6) & 0x3FF
        let is_wide = (header & 1) != 0;
        let len = ((header >> 6) & 0x3FF) as usize;

        if len == 0 || len > 1024 {
            bail!(
                "FName entry at 0x{:X} has invalid length {}",
                entry_addr,
                len
            );
        }

        let name = if is_wide {
            let mut buf = vec![0u8; len * 2];
            self.process.read_bytes(entry_addr + 2, &mut buf)?;
            let words: Vec<u16> = buf
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            String::from_utf16_lossy(&words)
        } else {
            let mut buf = vec![0u8; len];
            self.process.read_bytes(entry_addr + 2, &mut buf)?;
            String::from_utf8_lossy(&buf).into_owned()
        };

        let mut cache = self.name_cache.lock().unwrap();
        cache.insert(comparison_index, name.clone());
        Ok(name)
    }

    pub fn read_fname_at(&self, addr: usize) -> Result<(u32, u32)> {
        let mut buf = [0u8; 8];
        self.process.read_bytes(addr, &mut buf)?;
        let comparison_index = u32::from_le_bytes(buf[0..4].try_into().unwrap());
        let number = u32::from_le_bytes(buf[4..8].try_into().unwrap());
        Ok((comparison_index, number))
    }
}
