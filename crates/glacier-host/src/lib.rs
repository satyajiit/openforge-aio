//! Glacier 2 reflection bootstrap over an external `Ctx` (no DLL injection).
//!
//! Glacier ships a complete runtime type system (`ZTypeRegistry`). Every type's
//! name is a plain `const char*` in module data, its `IType` metadata lives in a
//! static module table, and the registry's `m_types` hash map keys those types by
//! name. Because nothing here needs in-process code, the whole walk runs from
//! outside the process through [`openforge_core::Ctx`] (`ReadProcessMemory`).
//!
//! Bootstrap (no `.text` signature, update-resilient):
//! 1. Find a type-name string in the module image **by content**.
//! 2. Scan committed RW memory for 8-byte pointers to that string VA — these are
//!    the static `IType.pszTypeName` slot and the registry node's `char*` key.
//! 3. From either, round-trip to the `IType` and validate.
//!
//! Struct offsets are the ZHMModSDK (Hitman WoA) layout. The `IType`/`STypeID`/
//! registry-node offsets AND the `IClassType`/property offsets below are all
//! confirmed live on 007 First Light via `glacier-walk` (e.g. `ZSpatialEntity`
//! decodes `m_mTransform`/`m_bVisible`/… with correct names, CRC32 ids, and
//! types). `glacier-walk --raw` remains available to re-validate on game updates.

use openforge_core::{Ctx, Error, Result};

// ---- IType (the per-type metadata record) --------------------------------
const ITYPE_SIZE: u64 = 0x08; // u16  m_nTypeSize
const ITYPE_ALIGN: u64 = 0x0A; // u8   m_nTypeAlignment
const ITYPE_FLAGS: u64 = 0x0C; // u16  m_nTypeInfoFlags
const ITYPE_NAME: u64 = 0x10; // char* pszTypeName
const ITYPE_TYPEID: u64 = 0x18; // STypeID* typeID
const ITYPE_SIZEOF: u64 = 0x30; // end of IType / start of IClassType extras

// ---- STypeID (indirection handle) ----------------------------------------
const STYPEID_PTYPE: u64 = 0x08; // IType* m_pType

// ---- ZTypeRegistry m_types node (32-byte hash-map node) ------------------
// { u32 len|0x80000000 @0x00, char* name @0x08, STypeID* @0x10, u32 next @0x18 }
const NODE_NAME_OFF: u64 = 0x08;
const NODE_STYPEID_OFF: u64 = 0x10;

// ---- IClassType extras (validated on 007 First Light) --------------------
const ICLASS_NPROPS: u64 = ITYPE_SIZEOF; // u16  m_nProperties  @0x30
const ICLASS_PPROPS: u64 = 0x40; // SNamedPropertyInfo* m_pProperties

// ---- SNamedPropertyInfo (validated on 007 First Light) -------------------
const PROP_STRIDE: u64 = 0x38;
const PROP_NAME: u64 = 0x00; // char* m_pszPropertyName
const PROP_ID: u64 = 0x08; // u32   m_nPropertyID (CRC32)
const PROP_TYPE: u64 = 0x10; // STypeID* m_Type
const PROP_FLAGS: u64 = 0x20; // u32   m_Flags

// ---- ETypeInfoFlags ------------------------------------------------------
pub const TIF_ENTITY: u16 = 0x0001;
pub const TIF_RESOURCE: u16 = 0x0002;
pub const TIF_CLASS: u16 = 0x0004;
pub const TIF_ENUM: u16 = 0x0008;
pub const TIF_CONTAINER: u16 = 0x0010;
pub const TIF_ARRAY: u16 = 0x0020;
pub const TIF_FIXEDARRAY: u16 = 0x0040;
pub const TIF_MAP: u16 = 0x0200;
pub const TIF_PRIMITIVE: u16 = 0x0400;

/// A resolved Glacier reflection type.
#[derive(Debug, Clone)]
pub struct TypeInfo {
    pub name: String,
    /// VA of the (static) `IType` record.
    pub itype_va: u64,
    /// VA of the `STypeID` handle.
    pub stypeid_va: u64,
    /// VA of the type-name string the resolver matched.
    pub name_string_va: u64,
    pub flags: u16,
    pub size: u16,
    pub align: u8,
    /// True if resolved via the static `IType.pszTypeName` slot (module data);
    /// false if resolved via a registry hash-map node (heap).
    pub via_static: bool,
}

/// One property on a (class) type. Offsets are the ZHMModSDK layout hypothesis;
/// treat `crc32` (the name CRC) as the stable identity and the rest as
/// pending-validation until confirmed against this build.
#[derive(Debug, Clone)]
pub struct PropertyInfo {
    pub index: usize,
    pub name: String,
    pub crc32: u32,
    pub flags: u32,
    pub type_name: Option<String>,
}

/// Decode `m_nTypeInfoFlags` into a human string.
pub fn flag_names(flags: u16) -> String {
    let mut v = Vec::new();
    for (bit, name) in [
        (TIF_ENTITY, "Entity"),
        (TIF_RESOURCE, "Resource"),
        (TIF_CLASS, "Class"),
        (TIF_ENUM, "Enum"),
        (TIF_CONTAINER, "Container"),
        (TIF_ARRAY, "Array"),
        (TIF_FIXEDARRAY, "FixedArray"),
        (TIF_MAP, "Map"),
        (TIF_PRIMITIVE, "Primitive"),
    ] {
        if flags & bit != 0 {
            v.push(name);
        }
    }
    if v.is_empty() {
        format!("0x{flags:04X}")
    } else {
        v.join("|")
    }
}

/// Reflection walker bound to a live `Ctx` (e.g. a `Target` over RPM).
pub struct GlacierReflection<'a> {
    ctx: &'a dyn Ctx,
}

impl<'a> GlacierReflection<'a> {
    pub fn new(ctx: &'a dyn Ctx) -> Self {
        Self { ctx }
    }

    fn read_u64(&self, addr: u64) -> Result<u64> {
        let mut b = [0u8; 8];
        self.ctx.read_bytes(addr as usize, &mut b)?;
        Ok(u64::from_le_bytes(b))
    }

    fn read_u32(&self, addr: u64) -> Result<u32> {
        let mut b = [0u8; 4];
        self.ctx.read_bytes(addr as usize, &mut b)?;
        Ok(u32::from_le_bytes(b))
    }

    fn read_u16(&self, addr: u64) -> Result<u16> {
        let mut b = [0u8; 2];
        self.ctx.read_bytes(addr as usize, &mut b)?;
        Ok(u16::from_le_bytes(b))
    }

    fn read_u8(&self, addr: u64) -> Result<u8> {
        let mut b = [0u8; 1];
        self.ctx.read_bytes(addr as usize, &mut b)?;
        Ok(b[0])
    }

    /// Read a NUL-terminated C string, tolerating reads that would cross into
    /// unmapped memory by trying progressively smaller windows.
    fn read_cstr(&self, addr: u64) -> Result<String> {
        if addr == 0 {
            return Ok(String::new());
        }
        for cap in [128usize, 48, 16, 8] {
            let mut buf = vec![0u8; cap];
            if self.ctx.read_bytes(addr as usize, &mut buf).is_ok() {
                let end = buf.iter().position(|&b| b == 0).unwrap_or(cap);
                return Ok(String::from_utf8_lossy(&buf[..end]).into_owned());
            }
        }
        Err(Error::Custom(format!(
            "read_cstr: unreadable at 0x{addr:X}"
        )))
    }

    /// Locate a type-name string in the main module image by content. Returns
    /// the VA of the start of the NUL-terminated string, or `None`.
    pub fn find_type_string(&self, name: &str) -> Result<Option<u64>> {
        let m = self.ctx.main_module();
        let base = m.base as u64;
        let size = m.size as u64;
        let mut needle = name.as_bytes().to_vec();
        needle.push(0); // require NUL terminator — a registry type name is a C string
        let overlap = needle.len() as u64;
        let chunk: u64 = 1 << 20; // 1 MiB

        let mut off: u64 = 0;
        while off < size {
            let win_end = (off + chunk + overlap).min(size);
            let win_len = (win_end - off) as usize;
            let mut buf = vec![0u8; win_len];
            // Tolerate unreadable pages: skip the chunk on failure.
            if self.ctx.read_bytes((base + off) as usize, &mut buf).is_ok()
                && let Some(p) = find_subslice(&buf, &needle)
            {
                // Drop the trailing NUL from the reported VA span.
                return Ok(Some(base + off + p as u64));
            }
            off += chunk;
        }
        Ok(None)
    }

    /// Validate that `itype_va` is a real `IType` for the given string, and read
    /// its fields. Round-trips through `STypeID.m_pType` to reject false hits.
    fn try_itype(
        &self,
        itype_va: u64,
        string_va: u64,
        name: &str,
        via_static: bool,
    ) -> Result<Option<TypeInfo>> {
        // pszTypeName must point back at the matched string.
        if self.read_u64(itype_va + ITYPE_NAME).unwrap_or(0) != string_va {
            return Ok(None);
        }
        let stypeid_va = self.read_u64(itype_va + ITYPE_TYPEID).unwrap_or(0);
        if stypeid_va == 0 {
            return Ok(None);
        }
        // STypeID.m_pType must round-trip back to this IType.
        if self.read_u64(stypeid_va + STYPEID_PTYPE).unwrap_or(0) != itype_va {
            return Ok(None);
        }
        Ok(Some(TypeInfo {
            name: name.to_string(),
            itype_va,
            stypeid_va,
            name_string_va: string_va,
            flags: self.read_u16(itype_va + ITYPE_FLAGS).unwrap_or(0),
            size: self.read_u16(itype_va + ITYPE_SIZE).unwrap_or(0),
            align: self.read_u8(itype_va + ITYPE_ALIGN).unwrap_or(0),
            via_static,
        }))
    }

    /// Resolve a type by name: string → pointers → IType (validated).
    pub fn resolve_type(&self, name: &str) -> Result<Option<TypeInfo>> {
        let Some(string_va) = self.find_type_string(name)? else {
            return Ok(None);
        };
        let hits = self.ctx.scan_heap_for_u64(string_va, 8)?;
        let m = self.ctx.main_module();
        let mod_lo = m.base as u64;
        let mod_hi = (m.base + m.size) as u64;
        let in_module = |a: u64| a >= mod_lo && a < mod_hi;

        // Prefer the static IType.pszTypeName slot (module data) for a
        // module-relative, resilient IType address.
        for &slot in hits.iter().filter(|&&s| in_module(s as u64)) {
            let slot = slot as u64;
            if slot < ITYPE_NAME {
                continue;
            }
            if let Some(ti) = self.try_itype(slot - ITYPE_NAME, string_va, name, true)? {
                return Ok(Some(ti));
            }
        }
        // Fall back to a registry node's char* slot (heap): STypeID at slot+0x08.
        for &slot in hits.iter().filter(|&&s| !in_module(s as u64)) {
            let slot = slot as u64;
            let stypeid = self.read_u64(slot + (NODE_STYPEID_OFF - NODE_NAME_OFF))?;
            if stypeid == 0 {
                continue;
            }
            let itype = self.read_u64(stypeid + STYPEID_PTYPE).unwrap_or(0);
            if itype != 0
                && let Some(ti) = self.try_itype(itype, string_va, name, false)?
            {
                return Ok(Some(ti));
            }
        }
        Ok(None)
    }

    /// Resolve a `STypeID*` to its type name (best effort).
    fn type_name_of_stypeid(&self, stypeid_va: u64) -> Option<String> {
        if stypeid_va == 0 {
            return None;
        }
        let itype = self.read_u64(stypeid_va + STYPEID_PTYPE).ok()?;
        if itype == 0 {
            return None;
        }
        let name_ptr = self.read_u64(itype + ITYPE_NAME).ok()?;
        self.read_cstr(name_ptr).ok().filter(|s| !s.is_empty())
    }

    /// Enumerate a class type's properties using the IClassType layout
    /// hypothesis. Returns the raw `(count, props_array_va)` alongside the
    /// decoded entries so callers can flag implausible results.
    pub fn enumerate_properties(&self, ty: &TypeInfo) -> Result<(u16, u64, Vec<PropertyInfo>)> {
        let nprops = self.read_u16(ty.itype_va + ICLASS_NPROPS).unwrap_or(0);
        let pprops = self.read_u64(ty.itype_va + ICLASS_PPROPS).unwrap_or(0);
        let mut out = Vec::new();
        if pprops == 0 || nprops == 0 || nprops > 4096 {
            return Ok((nprops, pprops, out));
        }
        for i in 0..nprops as u64 {
            let e = pprops + i * PROP_STRIDE;
            let name_ptr = self.read_u64(e + PROP_NAME).unwrap_or(0);
            let name = self.read_cstr(name_ptr).unwrap_or_default();
            let crc32 = self.read_u32(e + PROP_ID).unwrap_or(0);
            let flags = self.read_u32(e + PROP_FLAGS).unwrap_or(0);
            let type_name = self
                .read_u64(e + PROP_TYPE)
                .ok()
                .and_then(|sid| self.type_name_of_stypeid(sid));
            out.push(PropertyInfo {
                index: i as usize,
                name,
                crc32,
                flags,
                type_name,
            });
        }
        Ok((nprops, pprops, out))
    }

    /// Raw bytes of the `IType`/`IClassType` header, for offset validation.
    pub fn raw_itype(&self, ty: &TypeInfo, len: usize) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; len];
        self.ctx.read_bytes(ty.itype_va as usize, &mut buf)?;
        Ok(buf)
    }
}

/// Naive forward substring search (first-byte gated). Needle is short, hay is a
/// ~1 MiB chunk — fast enough and dependency-free.
fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    let first = needle[0];
    let last_start = hay.len() - needle.len();
    let mut i = 0;
    while i <= last_start {
        if hay[i] == first && &hay[i..i + needle.len()] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}
