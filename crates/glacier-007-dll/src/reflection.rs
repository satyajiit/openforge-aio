//! Glacier reflection op handlers.
//!
//! Each handler builds an `openforge_glacier_host::GlacierReflection` over the
//! in-process [`crate::local_ctx::LocalCtx`] and turns its result into a wire
//! [`Response`]. The structural decode (type resolution, static property
//! enumeration, the live `ZEntityImpl` → `ZEntityType` → `SPropertyData`
//! instance walk, CRC32 property matching, and the `m_pObj = entity + 8` value
//! base) all live in the shared host crate — this module is the thin wire
//! adapter plus the guarded raw write.

use openforge_core::Ctx;
use openforge_glacier_host::{
    EntityRef, GlacierReflection, PropertyInfo, ResolvedGlacierField, TypeInfo, crc32_property_id,
};
use openforge_glacier_protocol::{
    GlacierField, GlacierType, GlacierTypeProp, GlacierValue, Response,
};

use crate::scan;

// ---- wire conversions ------------------------------------------------------

fn type_to_wire(t: &TypeInfo) -> GlacierType {
    GlacierType {
        name: t.name.clone(),
        itype_va: t.itype_va,
        stypeid_va: t.stypeid_va,
        name_string_va: t.name_string_va,
        flags: t.flags,
        size: t.size,
        align: t.align,
        via_static: t.via_static,
    }
}

fn type_prop_to_wire(p: &PropertyInfo) -> GlacierTypeProp {
    GlacierTypeProp {
        index: p.index as u32,
        name: p.name.clone(),
        crc32: p.crc32,
        flags: p.flags,
        type_name: p.type_name.clone(),
    }
}

fn field_to_wire(f: &ResolvedGlacierField) -> GlacierField {
    GlacierField {
        name: f.name.clone(),
        crc32: f.crc32,
        offset: f.offset,
        sprop_flags: f.sprop_flags,
        descriptor_flags: f.descriptor_flags,
        has_getter_setter: f.has_getter_setter,
        type_name: f.type_name.clone(),
    }
}

// ---- handlers --------------------------------------------------------------

/// `ResolveType { name }` → `ResolvedType` | `NotFound` | `Error`.
pub fn resolve_type(ctx: &dyn Ctx, name: &str) -> Response {
    let refl = GlacierReflection::new(ctx);
    match refl.resolve_type(name) {
        Ok(Some(t)) => Response::ResolvedType(type_to_wire(&t)),
        Ok(None) => Response::NotFound,
        Err(e) => Response::Error(format!("resolve_type({name:?}): {e}")),
    }
}

/// `EnumerateTypeProperties { name }` → `TypeProperties` | `NotFound` |
/// `Error`. A type that resolves but has no class-property array yields an
/// empty `TypeProperties` (not `NotFound`); only an unresolved type name is
/// `NotFound`.
pub fn enumerate_type_properties(ctx: &dyn Ctx, name: &str) -> Response {
    let refl = GlacierReflection::new(ctx);
    let ty = match refl.resolve_type(name) {
        Ok(Some(t)) => t,
        Ok(None) => return Response::NotFound,
        Err(e) => return Response::Error(format!("resolve_type({name:?}): {e}")),
    };
    match refl.enumerate_properties(&ty) {
        Ok((_n, _arr, props)) => {
            Response::TypeProperties(props.iter().map(type_prop_to_wire).collect())
        }
        Err(e) => Response::Error(format!("enumerate_properties({name:?}): {e}")),
    }
}

/// `InstanceProperties { entity_va }` → `InstanceProps` | `Error`.
pub fn instance_properties(ctx: &dyn Ctx, entity_va: u64) -> Response {
    let refl = GlacierReflection::new(ctx);
    match refl.instance_properties(entity_va) {
        Ok((ent, fields)) => Response::InstanceProps {
            entity_va: ent.entity_va,
            obj_base: ent.obj_base(),
            fields: fields.iter().map(field_to_wire).collect(),
        },
        Err(e) => Response::Error(format!("instance_properties(0x{entity_va:X}): {e}")),
    }
}

/// `ResolveInstanceProperty { entity_va, name }` → `ResolvedField` |
/// `NotFound` | `Error`.
pub fn resolve_instance_property(ctx: &dyn Ctx, entity_va: u64, name: &str) -> Response {
    let refl = GlacierReflection::new(ctx);
    match refl.resolve_instance_property(entity_va, name) {
        Ok(Some(f)) => Response::ResolvedField(field_to_wire(&f)),
        Ok(None) => Response::NotFound,
        Err(e) => Response::Error(format!(
            "resolve_instance_property(0x{entity_va:X}, {name:?}): {e}"
        )),
    }
}

/// `SetProperty { entity_va, property, value }` → `WriteOk` | `NotFound` |
/// `Error`.
///
/// Resolves the property on the live instance, refuses a raw write when the
/// descriptor carries `E_HAS_GETTER_SETTER` (the engine's setter is canonical
/// for those — a later game-thread-routed op), then stamps the value's
/// little-endian image at the canonical `m_pObj + offset` value address.
pub fn set_property(
    ctx: &dyn Ctx,
    entity_va: u64,
    property: &str,
    value: &GlacierValue,
) -> Response {
    let refl = GlacierReflection::new(ctx);
    let field = match refl.resolve_instance_property(entity_va, property) {
        Ok(Some(f)) => f,
        Ok(None) => return Response::NotFound,
        Err(e) => {
            return Response::Error(format!(
                "set_property: resolve(0x{entity_va:X}, {property:?}): {e}"
            ));
        }
    };
    if field.has_getter_setter {
        return Response::Error(format!(
            "property {property:?} has a getter/setter (descriptor_flags={:?}); a raw write is \
             not honored by the engine — needs the engine setter (game-thread op, not yet wired)",
            field.descriptor_flags
        ));
    }
    // Build the value base through the canonical helper so the +8 m_pObj
    // indirection is never re-derived by hand. entity_type_va also validates
    // the entity is a live ZEntityImpl before we write.
    let entity_type_va = match refl.entity_type_va(entity_va) {
        Ok(v) => v,
        Err(e) => {
            return Response::Error(format!("set_property: entity_type(0x{entity_va:X}): {e}"));
        }
    };
    let ent = EntityRef {
        entity_va,
        entity_type_va,
    };
    let addr = ent.value_addr(field.offset);
    let bytes = value.to_le_bytes();
    match ctx.write_bytes(addr as usize, &bytes) {
        Ok(()) => {
            crate::flog!(
                "INFO",
                "set_property: {property:?} (crc=0x{:08X}) @ 0x{addr:X} = {value:?}",
                field.crc32
            );
            Response::WriteOk
        }
        Err(e) => Response::Error(format!("set_property: write at 0x{addr:X}: {e}")),
    }
}

/// `FindEntitiesWithProperty { property, max_results }` → `Entities`.
///
/// Heap-scans for `ZEntityImpl`-shaped objects (vtable into the main module)
/// whose per-instance `SPropertyData` array carries `property` (CRC32 match).
/// The whole walk is in-process and fault-isolated.
pub fn find_entities_with_property(ctx: &dyn Ctx, property: &str, max_results: u32) -> Response {
    let crc = crc32_property_id(property);
    let m = ctx.main_module();
    let lo = m.base as u64;
    let hi = lo.saturating_add(m.size as u64);
    let max = max_results as usize;
    crate::flog!(
        "INFO",
        "find_entities_with_property: {property:?} crc=0x{crc:08X} max={max} module=[0x{lo:X},0x{hi:X})"
    );

    let refl = GlacierReflection::new(ctx);
    let mut out: Vec<u64> = Vec::new();
    scan::for_each_candidate_object(lo, hi, |cand| {
        if max != 0 && out.len() >= max {
            return false;
        }
        if refl.entity_has_property(cand, crc).unwrap_or(false) {
            out.push(cand);
        }
        true
    });
    crate::flog!(
        "INFO",
        "find_entities_with_property: {property:?} → {} hits",
        out.len()
    );
    Response::Entities(out)
}
