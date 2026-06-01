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

// ---- trainer-side driver (the `client` feature) --------------------------
//
// Inject the per-game Glacier DLL and drive its named-pipe protocol via a
// `Ctx`-implementing `GlacierSession`. Gated so the in-process DLL — which
// depends on this crate only for the reflection engine below — never compiles
// the Win32 injection machinery.
#[cfg(all(windows, feature = "client"))]
pub mod backend;
#[cfg(all(windows, feature = "client"))]
mod client;
#[cfg(all(windows, feature = "client"))]
mod dll_path;
#[cfg(all(windows, feature = "client"))]
mod error;
#[cfg(all(windows, feature = "client"))]
mod injector;
#[cfg(all(windows, feature = "client"))]
mod pipe;
#[cfg(all(windows, feature = "client"))]
mod session;

#[cfg(all(windows, feature = "client"))]
pub use crate::backend::Glacier2Backend;
#[cfg(all(windows, feature = "client"))]
pub use crate::client::GlacierClient;
#[cfg(all(windows, feature = "client"))]
pub use crate::dll_path::{DLL_PATH_ENV, resolve_dll_path};
#[cfg(all(windows, feature = "client"))]
pub use crate::error::{HostError, Result as HostResult};
#[cfg(all(windows, feature = "client"))]
pub use crate::injector::Injector;
#[cfg(all(windows, feature = "client"))]
pub use crate::session::{DEFAULT_CONNECT_TIMEOUT, GlacierSession};

/// Resolved Glacier handshake metadata, mirrored from
/// `openforge_glacier_protocol::Response::Welcome` for ergonomic host-side
/// access.
#[cfg(all(windows, feature = "client"))]
#[derive(Debug, Clone)]
pub struct Welcome {
    pub server_version: u32,
    pub pid: u32,
    pub module_base: u64,
    pub module_size: u64,
}

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

// ---- Live-instance chain (ZHMModSDK ZEntity.h layout) --------------------
// ZEntityImpl: vtable@0x00, m_pEntityType@0x08 (tagged ptr), id/flags@0x10.
const ENTITY_TYPE_OFF: u64 = 0x08; // ZEntityImpl.m_pEntityType (tagged)
// ZEntityType: m_nBorrowedPointersMask(i32)@0x00, m_pPropertyData@0x08, ...
// m_pPropertyData is a *pointer to* a `TArray<SPropertyData>`, not inline.
const ETYPE_PROPDATA_OFF: u64 = 0x08; // ZEntityType.m_pPropertyData (-> TArray*)
const TARRAY_BEGIN: u64 = 0x00; // TArray { begin, end, allocEnd }
const TARRAY_END: u64 = 0x08;
// SPropertyData entry (stride 0x28). The per-instance byte offset lives ONLY
// here — the SNamedPropertyInfo descriptor carries type/flags/accessors but no
// offset. Match by `m_nPropertyId` (CRC32) via a linear scan (no hash index).
const SPROP_STRIDE: u64 = 0x28;
const SPROP_INFO: u64 = 0x00; // SPropertyInfo* (== &SNamedPropertyInfo + 0x10)
const SPROP_OFFSET: u64 = 0x08; // i64 m_nPropertyOffset
const SPROP_ID: u64 = 0x10; // u32 m_nPropertyId (CRC32)
const SPROP_FLAGS: u64 = 0x14; // u32 m_nPropertyFlags
// SPropertyInfo (reached from SPropertyData.m_pPropertyInfo): type@0x00,
// extraData@0x08, m_Flags@0x10 (E_HAS_GETTER_SETTER bit), accessors after.
const SPROPINFO_TYPE: u64 = 0x00; // STypeID* m_Type
const SPROPINFO_FLAGS: u64 = 0x10; // u32 m_Flags
// SPropertyData.m_pPropertyInfo points at the SPropertyInfo *embedded inside*
// SNamedPropertyInfo at +0x10, so the enclosing descriptor (which carries the
// name char*) is `m_pPropertyInfo - 0x10`.
const SNAMED_PROPINFO_OFF: u64 = 0x10; // offsetof(SNamedPropertyInfo, m_propertyInfo)
/// `EPropertyInfoFlags::E_HAS_GETTER_SETTER`. A raw RPM write is NOT honored
/// for these — the engine's setter is canonical (a Tier-2/in-process concern).
pub const E_HAS_GETTER_SETTER: u32 = 0x10;

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

/// One property on a (class) type, decoded from the static `IClassType`
/// descriptor array. `crc32` (the CRC32 of `name`) is the stable identity used
/// to match the per-instance [`ResolvedGlacierField`] at runtime.
#[derive(Debug, Clone)]
pub struct PropertyInfo {
    pub index: usize,
    pub name: String,
    pub crc32: u32,
    pub flags: u32,
    pub type_name: Option<String>,
}

/// A handle to a live Glacier entity instance and its (untagged) `ZEntityType`.
/// `entity_va` is the `ZEntityImpl` base. Per ZHMModSDK `ZEntityRef::GetProperty`
/// the per-instance value base is **`m_pObj` = `entity_va + 8`** (the `ZEntityRef`
/// payload, one pointer-width into the object), so a resolved property's live
/// value lives at `m_pObj + m_nPropertyOffset`. Always go through
/// [`EntityRef::value_addr`] — never add an offset to `entity_va` directly, or
/// you land 8 bytes short and corrupt the adjacent field. Analogous to UE5's
/// `(obj_addr, class_addr)` pair.
#[derive(Debug, Clone, Copy)]
pub struct EntityRef {
    pub entity_va: u64,
    pub entity_type_va: u64,
}

impl EntityRef {
    /// The `ZEntityRef` value base `m_pObj` (`== entity_va + 8`). Property
    /// offsets are added to this, not to `entity_va`.
    pub fn obj_base(&self) -> u64 {
        self.entity_va.wrapping_add(8)
    }

    /// Absolute VA of a resolved property's live value: `m_pObj + offset`.
    pub fn value_addr(&self, offset: i64) -> u64 {
        (self.obj_base() as i64).wrapping_add(offset) as u64
    }
}

/// A property resolved against one live entity's `SPropertyData` array. `offset`
/// is the per-instance byte offset (`m_nPropertyOffset`); the live value lives at
/// `m_pObj + offset` (`= entity_va + 8 + offset`) — use [`EntityRef::value_addr`].
/// `has_getter_setter` gates raw writes: when set, the engine setter is canonical
/// and a raw RPM write is not honored.
#[derive(Debug, Clone)]
pub struct ResolvedGlacierField {
    /// Property name, decoded from the descriptor (`SNamedPropertyInfo`).
    /// `None` if the descriptor pointer was unreadable.
    pub name: Option<String>,
    /// CRC32 of the property name (the lookup key).
    pub crc32: u32,
    /// `SPropertyData.m_nPropertyOffset` — per-instance byte offset, added to the
    /// entity's `m_pObj` (`= entity_va + 8`), not to `entity_va`. See
    /// [`EntityRef::value_addr`].
    pub offset: i64,
    /// `SPropertyData.m_nPropertyFlags` (per-instance flags).
    pub sprop_flags: u32,
    /// `SPropertyInfo.m_Flags` from the descriptor (carries the getter/setter
    /// bit). `None` if the descriptor pointer was unreadable.
    pub descriptor_flags: Option<u32>,
    /// `true` iff `descriptor_flags & E_HAS_GETTER_SETTER`.
    pub has_getter_setter: bool,
    /// Best-effort property type name (decoded from the descriptor's STypeID).
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

    // -- live-instance property resolution ---------------------------------

    /// Read + untag a live `ZEntityImpl`'s `m_pEntityType`, returning the
    /// `ZEntityType` VA. The pointer at `entity_va + 0x08` is tagged in bit0:
    /// clear ⇒ the `ZEntityType*` directly; set ⇒ a self-relative indirection
    /// `*(ZEntityType**)(&m_pEntityType + (raw >> 1))` (arithmetic shift). See
    /// ZHMModSDK `ZEntityImpl::GetType`.
    pub fn entity_type_va(&self, entity_va: u64) -> Result<u64> {
        let slot = entity_va.wrapping_add(ENTITY_TYPE_OFF);
        let raw = self.read_u64(slot)?;
        if raw & 1 == 0 {
            Ok(raw)
        } else {
            let disp = (raw as i64) >> 1;
            let indirect = (slot as i64).wrapping_add(disp) as u64;
            self.read_u64(indirect)
        }
    }

    /// Bounds of a `ZEntityType`'s `SPropertyData` array as `(begin_va, count)`.
    /// `m_pPropertyData` is a *pointer to* a `TArray { begin, end, allocEnd }`,
    /// so we deref it then size from `(end - begin) / stride`. Returns a zero
    /// count for an absent array or an implausible span (wrong layout / freed).
    ///
    /// The plain `{begin, end}` sizing is valid only because `sizeof(SPropertyData)`
    /// is `0x28` (> 16), so Glacier's `TArray` never inlines these elements and
    /// always heap-allocates. Do NOT reuse this for a `TArray` of a ≤16-byte
    /// element without handling the inline-storage path.
    fn property_data_span(&self, entity_type_va: u64) -> Result<(u64, u64)> {
        let arr = self.read_u64(entity_type_va.wrapping_add(ETYPE_PROPDATA_OFF))?;
        if arr == 0 {
            return Ok((0, 0));
        }
        let begin = self.read_u64(arr.wrapping_add(TARRAY_BEGIN))?;
        let end = self.read_u64(arr.wrapping_add(TARRAY_END))?;
        if begin == 0 || end <= begin {
            return Ok((begin, 0));
        }
        let span = end - begin;
        if span % SPROP_STRIDE != 0 || span > SPROP_STRIDE * 100_000 {
            return Ok((begin, 0));
        }
        Ok((begin, span / SPROP_STRIDE))
    }

    /// Decode one `SPropertyData` entry into a [`ResolvedGlacierField`],
    /// reaching through `m_pPropertyInfo` for the name, getter/setter flag, and
    /// property type name.
    fn decode_sproperty(&self, entry_va: u64) -> Result<ResolvedGlacierField> {
        let crc32 = self.read_u32(entry_va.wrapping_add(SPROP_ID))?;
        let offset = self.read_u64(entry_va.wrapping_add(SPROP_OFFSET))? as i64;
        let sprop_flags = self.read_u32(entry_va.wrapping_add(SPROP_FLAGS))?;
        let info = self
            .read_u64(entry_va.wrapping_add(SPROP_INFO))
            .unwrap_or(0);
        let (name, descriptor_flags, type_name) = if info != 0 {
            let descriptor_flags = self.read_u32(info.wrapping_add(SPROPINFO_FLAGS)).ok();
            let type_name = self
                .read_u64(info.wrapping_add(SPROPINFO_TYPE))
                .ok()
                .and_then(|sid| self.type_name_of_stypeid(sid));
            let named = info.wrapping_sub(SNAMED_PROPINFO_OFF);
            let name = self
                .read_u64(named.wrapping_add(PROP_NAME))
                .ok()
                .and_then(|p| self.read_cstr(p).ok())
                .filter(|s| !s.is_empty());
            (name, descriptor_flags, type_name)
        } else {
            (None, None, None)
        };
        let has_getter_setter = descriptor_flags.is_some_and(|f| f & E_HAS_GETTER_SETTER != 0);
        Ok(ResolvedGlacierField {
            name,
            crc32,
            offset,
            sprop_flags,
            descriptor_flags,
            has_getter_setter,
            type_name,
        })
    }

    /// Enumerate every per-instance property on a live entity (decoded from its
    /// `ZEntityType`'s `SPropertyData` array). Returns the [`EntityRef`] handle
    /// alongside the decoded fields.
    pub fn instance_properties(
        &self,
        entity_va: u64,
    ) -> Result<(EntityRef, Vec<ResolvedGlacierField>)> {
        let entity_type_va = self.entity_type_va(entity_va)?;
        let (begin, count) = self.property_data_span(entity_type_va)?;
        let mut out = Vec::with_capacity(count as usize);
        for i in 0..count {
            if let Ok(f) = self.decode_sproperty(begin.wrapping_add(i * SPROP_STRIDE)) {
                out.push(f);
            }
        }
        Ok((
            EntityRef {
                entity_va,
                entity_type_va,
            },
            out,
        ))
    }

    /// Resolve a single named property on a live entity to its per-instance
    /// byte offset. Matches `CRC32(name)` against the `SPropertyData` array via
    /// a linear scan — the engine's own `FindProperty` is also linear, with no
    /// hash index.
    pub fn resolve_instance_property(
        &self,
        entity_va: u64,
        name: &str,
    ) -> Result<Option<ResolvedGlacierField>> {
        let target = crc32_property_id(name);
        let entity_type_va = self.entity_type_va(entity_va)?;
        let (begin, count) = self.property_data_span(entity_type_va)?;
        for i in 0..count {
            let entry = begin.wrapping_add(i * SPROP_STRIDE);
            if self.read_u32(entry.wrapping_add(SPROP_ID)).unwrap_or(0) == target {
                return Ok(Some(self.decode_sproperty(entry)?));
            }
        }
        Ok(None)
    }

    /// Read `len` bytes at an absolute VA. Used by the validation CLI to dump a
    /// resolved field's live value without baking a width into the engine.
    pub fn read_raw(&self, addr: u64, len: usize) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; len];
        self.ctx.read_bytes(addr as usize, &mut buf)?;
        Ok(buf)
    }

    /// In-process entity-discovery primitive: does the live entity at
    /// `entity_va` carry a property whose CRC32 id equals `crc`?
    ///
    /// Validates the `ZEntityImpl` → `ZEntityType` → `SPropertyData` chain and
    /// linearly scans the per-instance property ids. Fault-isolated: a bad
    /// candidate pointer collapses to `Ok(false)` (the span guard returns a
    /// zero count) or `Err` (an unreadable chain link) — never a crash — so it
    /// is safe to call across millions of heap-scan candidates. The first
    /// successful read of `m_pEntityType` already rejects the overwhelming
    /// majority of non-entity candidates.
    pub fn entity_has_property(&self, entity_va: u64, crc: u32) -> Result<bool> {
        let entity_type_va = self.entity_type_va(entity_va)?;
        let (begin, count) = self.property_data_span(entity_type_va)?;
        for i in 0..count {
            let entry = begin.wrapping_add(i * SPROP_STRIDE);
            if self.read_u32(entry.wrapping_add(SPROP_ID)).unwrap_or(0) == crc {
                return Ok(true);
            }
        }
        Ok(false)
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

/// CRC32 (IEEE 802.3 / zlib: reflected, poly `0xEDB88320`, init/final
/// `0xFFFFFFFF`) of a property or pin name — the exact hash Glacier uses for
/// `SPropertyData.m_nPropertyId` and pin ids. (Type *names* use a different
/// hash, FNV-1a-64-lowercased, in the registry; only properties/pins use this.)
/// Verified against live 007 First Light hashes — see the test below.
pub fn crc32_property_id(name: &str) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in name.as_bytes() {
        crc ^= b as u32;
        for _ in 0..8 {
            // Branchless reflected step: subtract the low bit to build a mask.
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::crc32_property_id;

    /// Locks the property-name hash to the values read live off 007 First
    /// Light's reflection tables (via `glacier-walk`). If this ever fails on a
    /// new build, the engine changed its hashing — re-derive before trusting
    /// `resolve_instance_property`.
    #[test]
    fn crc32_matches_live_first_light_hashes() {
        for (name, expect) in [
            ("m_infiniteAmmo", 0x2BD1CEC3u32),
            ("m_humanoid", 0xA18C1668),
            ("m_bActive", 0x267A966C),
            ("m_maxhealth", 0xA92A6D3C),
            ("m_isUnkillable", 0x51C3CF42),
            ("m_maximumAmmunitionWhenSpawned", 0x7048DBB5),
        ] {
            assert_eq!(crc32_property_id(name), expect, "crc mismatch for {name}");
        }
    }
}
