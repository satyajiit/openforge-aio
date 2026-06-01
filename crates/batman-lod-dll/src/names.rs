//! FName decoding via the engine's own `FName::ToString`. The chunk-walking
//! fallback (strategies A/B/C) was retired with the rest of the legacy
//! locator pipeline; the only decode path that remains is the SEH-protected
//! call into `FName::ToString` at the probe-validated address.

use openforge_dll_common::local_reader::ReadError;

use crate::engine::UeEngine;

#[derive(Debug)]
#[allow(dead_code)]
pub enum NameError {
    Read(ReadError),
    /// The engine-call path returned an unexpected failure (SEH fault,
    /// malformed FString, etc.). The string slot is the diagnostic reason.
    DecodeFailed(&'static str),
}

impl From<ReadError> for NameError {
    fn from(e: ReadError) -> Self {
        NameError::Read(e)
    }
}

impl UeEngine {
    /// Decode an FName given `(comparison_index, number)`. Suffixes the canonical
    /// `_<n-1>` form when `number != 0` (matching UE5's stringification).
    pub fn decode_fname(&self, comparison_index: u32, number: u32) -> Result<String, NameError> {
        let base = self.lookup_pool(comparison_index)?;
        Ok(if number != 0 {
            format!("{base}_{}", number - 1)
        } else {
            base
        })
    }

    /// Read an FName head `(comparison_index, number)` from `addr`.
    pub fn read_fname_at(&self, addr: usize) -> Result<(u32, u32), NameError> {
        let mut buf = [0u8; 8];
        self.reader.read_bytes(addr, &mut buf)?;
        let ci = u32::from_le_bytes(buf[0..4].try_into().unwrap());
        let num = u32::from_le_bytes(buf[4..8].try_into().unwrap());
        Ok((ci, num))
    }

    fn lookup_pool(&self, comparison_index: u32) -> Result<String, NameError> {
        if let Some(cached) = self.name_cache.lock().get(&comparison_index) {
            return Ok(cached.clone());
        }
        // `fname_to_string` is non-zero by construction (validated in
        // `attach_with_known_addresses`). The SEH shim inside
        // `call_fname_to_string` traps any in-engine AV / assertion so the
        // worker stays alive even if a wild FName ever made it this far.
        // We deliberately call with `number = 0` and layer the suffix at
        // `decode_fname` because the cache is keyed on `comparison_index`
        // alone.
        let decoded = unsafe { self.call_fname_to_string(comparison_index, 0) };
        let s = decoded.ok_or(NameError::DecodeFailed("call_fname_to_string"))?;
        self.name_cache
            .lock()
            .entry(comparison_index)
            .or_insert(s.clone());
        Ok(s)
    }
}
