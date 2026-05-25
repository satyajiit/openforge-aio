//! Reflection-promoted feature ops.
//!
//! `find_uobject` locates a live UObject by class name + an optional name
//! predicate. `resolve_property` turns a `(class_addr, property_name)` pair
//! into the offset + width metadata the host needs to call `ReadProperty` /
//! `WriteProperty` against any instance of that class.
//!
//! ## Why composable in-DLL primitives instead of one-shot read/write opcodes
//!
//! A feature read becomes three cached round-trips after attach:
//! `FindUObject` (live, polled at 250 ms cadence by the host until the
//! object exists), `ResolveProperty` (cached forever — UClass layouts are
//! immutable for the life of the process), `ReadProperty`. The cache hits
//! collapse to ~50 µs per feature read once the world is loaded.
//!
//! ## Object iteration
//!
//! [`find_uobject`] mirrors [`UeEngine::walk_objects`] but skips the
//! [`UeEngine::build_fqn`] call when the candidate's class name doesn't
//! match — `build_fqn` walks 64 outer hops + FName-decodes each, so
//! short-circuiting on `class_name` is ~10× cheaper for the typical
//! poll case ("class not loaded yet"). FQN construction only runs on the
//! handful of class-matching candidates that survive the cheap pre-filter.

use openforge_ue5_protocol::{NamePredicate, PropInfo, PropKind, ResolvedProperty};

use crate::engine::UeEngine;

/// `GUObjectArray` chunk size (UE4/UE5 stock). Matches [`objects::CHUNK_SIZE`].
const CHUNK_SIZE: usize = 65536;

/// Find a live UObject by class name. Returns `(obj_addr, class_addr)` on
/// the first match, or `None` if no instance is loaded.
///
/// `class_path` matches the last segment of the FQN-style class name
/// (case-insensitive). `predicate` further narrows when multiple instances
/// of the class exist (typical for `*PlayerState`-style classes that ship
/// one CDO template plus one or more live actors).
pub fn find_uobject(
    engine: &UeEngine,
    class_path: &str,
    predicate: &NamePredicate,
) -> Option<(u64, u64)> {
    let target = class_path.trim();
    if target.is_empty() {
        return None;
    }

    let mut header = [0u8; 32];
    engine
        .reader
        .read_bytes(engine.guobject_array, &mut header)
        .ok()?;
    let objects_ptr = u64::from_le_bytes(header[0..8].try_into().unwrap()) as usize;
    let num_elements = u32::from_le_bytes(header[20..24].try_into().unwrap()) as usize;
    let num_chunks = u32::from_le_bytes(header[28..32].try_into().unwrap()) as usize;
    if objects_ptr == 0 || num_elements == 0 || num_chunks == 0 {
        return None;
    }

    for chunk_idx in 0..num_chunks {
        let Ok(chunk_addr) = engine.reader.read_ptr(objects_ptr + chunk_idx * 8) else {
            break;
        };
        if chunk_addr == 0 {
            continue;
        }
        for within in 0..CHUNK_SIZE {
            let index = chunk_idx * CHUNK_SIZE + within;
            if index >= num_elements {
                return None;
            }
            let item_addr = chunk_addr + within * engine.fuobject_item_stride as usize;
            let Ok(obj_ptr) = engine.reader.read_ptr(item_addr) else {
                continue;
            };
            if obj_ptr == 0 || !obj_ptr.is_multiple_of(8) || obj_ptr < 0x10000 {
                continue;
            }
            let Ok(class_ptr) = engine.get_object_class(obj_ptr) else {
                continue;
            };
            // Cheap pre-filter: class name only. Skip FQN build if class
            // doesn't match.
            let Ok(class_name) = engine.get_object_name(class_ptr) else {
                continue;
            };
            if !class_name.eq_ignore_ascii_case(target) {
                continue;
            }
            // Class matches — now build FQN and apply predicate.
            if predicate_matches(engine, obj_ptr, predicate) {
                return Some((obj_ptr as u64, class_ptr as u64));
            }
        }
    }
    None
}

/// Multi-result variant of [`find_uobject`]: returns every live UObject whose
/// UClass name matches `class_path` (case-insensitive) and whose FQN passes
/// `predicate`. Stops collecting at `max_results` (returns `truncated=true`
/// in that case) so the response stays under the framer's 256 KB cap.
///
/// Drives "iterate-and-write" features like one-hit-kill, which needs every
/// live enemy's HealthAttributeSet, not just the first.
pub fn find_all_uobjects(
    engine: &UeEngine,
    class_path: &str,
    predicate: &NamePredicate,
    max_results: usize,
) -> (Vec<openforge_ue5_protocol::FoundObjectEntry>, bool) {
    use openforge_ue5_protocol::FoundObjectEntry;
    let target = class_path.trim();
    let mut out: Vec<FoundObjectEntry> = Vec::new();
    if target.is_empty() || max_results == 0 {
        return (out, false);
    }

    let mut header = [0u8; 32];
    if engine
        .reader
        .read_bytes(engine.guobject_array, &mut header)
        .is_err()
    {
        return (out, false);
    }
    let objects_ptr = u64::from_le_bytes(header[0..8].try_into().unwrap()) as usize;
    let num_elements = u32::from_le_bytes(header[20..24].try_into().unwrap()) as usize;
    let num_chunks = u32::from_le_bytes(header[28..32].try_into().unwrap()) as usize;
    if objects_ptr == 0 || num_elements == 0 || num_chunks == 0 {
        return (out, false);
    }

    let mut truncated = false;
    'outer: for chunk_idx in 0..num_chunks {
        let Ok(chunk_addr) = engine.reader.read_ptr(objects_ptr + chunk_idx * 8) else {
            break;
        };
        if chunk_addr == 0 {
            continue;
        }
        for within in 0..CHUNK_SIZE {
            let index = chunk_idx * CHUNK_SIZE + within;
            if index >= num_elements {
                break 'outer;
            }
            let item_addr = chunk_addr + within * engine.fuobject_item_stride as usize;
            let Ok(obj_ptr) = engine.reader.read_ptr(item_addr) else {
                continue;
            };
            if obj_ptr == 0 || !obj_ptr.is_multiple_of(8) || obj_ptr < 0x10000 {
                continue;
            }
            let Ok(class_ptr) = engine.get_object_class(obj_ptr) else {
                continue;
            };
            let Ok(class_name) = engine.get_object_name(class_ptr) else {
                continue;
            };
            if !class_name.eq_ignore_ascii_case(target) {
                continue;
            }
            if predicate_matches(engine, obj_ptr, predicate) {
                if out.len() >= max_results {
                    truncated = true;
                    break 'outer;
                }
                let fqn = engine.build_fqn(obj_ptr);
                out.push(FoundObjectEntry {
                    obj_addr: obj_ptr as u64,
                    class_addr: class_ptr as u64,
                    fqn,
                });
            }
        }
    }
    (out, truncated)
}

fn predicate_matches(engine: &UeEngine, obj_ptr: usize, predicate: &NamePredicate) -> bool {
    match predicate {
        NamePredicate::Any => true,
        NamePredicate::Exact(s) => engine
            .get_object_name(obj_ptr)
            .map(|n| n.eq_ignore_ascii_case(s))
            .unwrap_or(false),
        NamePredicate::Contains(s) => {
            let fqn = engine.build_fqn(obj_ptr);
            fqn.to_ascii_lowercase().contains(&s.to_ascii_lowercase())
        }
        NamePredicate::FqnPrefix(s) => {
            let fqn = engine.build_fqn(obj_ptr);
            fqn.to_ascii_lowercase()
                .starts_with(&s.to_ascii_lowercase())
        }
    }
}

/// Resolve a single FProperty on `class_addr` (or any super in its chain).
/// `walk_properties_cached` already caches per-class results in the engine,
/// so the iteration here is over a small in-memory vec.
pub fn resolve_property(
    engine: &UeEngine,
    class_addr: u64,
    property_name: &str,
) -> Option<ResolvedProperty> {
    let needle = property_name.trim();
    if needle.is_empty() {
        return None;
    }
    let props = engine.walk_properties_cached(class_addr).ok()?;
    for p in props.iter() {
        if p.name.eq_ignore_ascii_case(needle) {
            return Some(ResolvedProperty {
                offset: p.offset,
                size: p.size,
                kind: prop_kind_from_field(p),
                defined_in_class: p.defined_in_class.clone(),
            });
        }
    }
    None
}

/// Map a [`PropInfo`]'s `FFieldClass` name to the wire [`PropKind`]. Unknown
/// types fall through to `Bytes(size)` so callers can still issue a raw
/// read/write; the host wrapper surfaces the original `kind` string so a
/// feature TOML can opt into byte-mode explicitly.
pub(crate) fn prop_kind_from_field(p: &PropInfo) -> PropKind {
    match p.kind.as_str() {
        "Int8Property" => PropKind::I8,
        "Int16Property" => PropKind::I16,
        "IntProperty" => PropKind::I32,
        "Int64Property" => PropKind::I64,
        // UE5 ByteProperty is the unsigned 8-bit (also used for TEnumAsByte).
        "ByteProperty" | "UInt8Property" => PropKind::U8,
        "UInt16Property" => PropKind::U16,
        "UInt32Property" => PropKind::U32,
        "UInt64Property" => PropKind::U64,
        "FloatProperty" => PropKind::F32,
        "DoubleProperty" => PropKind::F64,
        "BoolProperty" => PropKind::Bool,
        // ObjectProperty / SoftObjectProperty / NameProperty / StrProperty
        // / StructProperty etc. — all fall through to raw bytes for now.
        // Future opcodes can promote these to typed reads (FName decode,
        // FString decode, struct-by-name etc.).
        _ => PropKind::Bytes(p.size),
    }
}

/// Offset from a UFunction's base to its `ParmsSize` field (`u16`).
/// Source: UE4SS log — `UFunction::ParmsSize = 0xB6`. Stable for this UE5
/// build; if a future game shifts it, surface as a BuildConfig field
/// rather than hardcoding here.
const UFUNCTION_PARMS_SIZE_OFFSET: usize = 0xB6;

/// Call a UFunction by name on `(obj_addr, class_addr)` via the engine's
/// `ProcessEvent` dispatcher. Returns the post-call parameter buffer
/// (out-parameters and the return slot are updated in place by the
/// engine).
///
/// ## Parameter buffer sizing
///
/// `params` is the caller's serialized argument blob. The actual buffer
/// passed to `ProcessEvent` is sized to `max(params.len(), ParmsSize)` and
/// zero-padded if shorter — UE5 expects exactly `ParmsSize` bytes and
/// reads/writes within that window regardless of how much the caller
/// sent. Truncation of caller-supplied data would silently lose
/// stack-passed pointers, so we never truncate; oversized `params` is a
/// hard error.
///
/// ## Safety
///
/// Raw FFI call into engine code. No SEH wrapper here — a crash from a
/// genuinely invalid `obj_addr` or `ufunction` pointer would take the
/// game down. Callers (the runtime's reflection-feature path) validate
/// both before this fires by going through `find_uobject` first. If we
/// add user-typed input later (Lua scripts, REPL), an SEH shim around
/// the call site is the right answer.
pub fn call_ufunction(
    engine: &UeEngine,
    obj_addr: u64,
    class_addr: u64,
    function_name: &str,
    params: &[u8],
) -> Result<Vec<u8>, String> {
    if engine.process_event == 0 {
        return Err("ProcessEvent address not configured for this build".into());
    }
    let needle = function_name.trim();
    if needle.is_empty() {
        return Err("function_name is empty".into());
    }
    if obj_addr == 0 {
        return Err("obj_addr is null".into());
    }
    if class_addr == 0 {
        return Err("class_addr is null".into());
    }

    let funcs = engine
        .walk_functions_cached(class_addr)
        .map_err(|e| format!("walk_functions failed: {e:?}"))?;
    let func = funcs
        .iter()
        .find(|f| f.name.eq_ignore_ascii_case(needle))
        .ok_or_else(|| {
            format!("UFunction `{needle}` not found on class chain (0x{class_addr:X})")
        })?;
    let ufunction_addr = func.addr as usize;
    if ufunction_addr == 0 {
        return Err(format!("UFunction `{needle}` has null address"));
    }

    let mut size_buf = [0u8; 2];
    engine
        .reader
        .read_bytes(ufunction_addr + UFUNCTION_PARMS_SIZE_OFFSET, &mut size_buf)
        .map_err(|e| format!("read ParmsSize failed: {e:?}"))?;
    let parms_size = u16::from_le_bytes(size_buf) as usize;

    if params.len() > parms_size && parms_size > 0 {
        return Err(format!(
            "params blob too large: caller sent {} bytes, UFunction `{needle}` \
             expects {parms_size} (truncation would silently drop arguments)",
            params.len()
        ));
    }

    // Allocate sized buffer; copy caller's bytes; zero-pad the rest.
    let buf_size = parms_size.max(params.len()).max(1);
    let mut buf: Vec<u8> = vec![0u8; buf_size];
    if !params.is_empty() {
        buf[..params.len()].copy_from_slice(params);
    }

    // FFI: `void ProcessEvent(UObject*, UFunction*, void*)`.
    type ProcessEventFn = unsafe extern "system" fn(
        *mut core::ffi::c_void,
        *mut core::ffi::c_void,
        *mut core::ffi::c_void,
    );
    let pe: ProcessEventFn = unsafe { core::mem::transmute(engine.process_event) };
    crate::flog!(
        "DEBUG",
        "ProcessEvent obj=0x{:X} fn=`{needle}` ufunction=0x{:X} parms_size={}",
        obj_addr,
        ufunction_addr,
        parms_size
    );
    unsafe {
        pe(
            obj_addr as *mut _,
            ufunction_addr as *mut _,
            buf.as_mut_ptr() as *mut _,
        );
    }

    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prop_kind_mapping_covers_common_uclass_field_types() {
        let mk = |kind: &str, size: u32| PropInfo {
            name: "x".into(),
            offset: 0,
            size,
            kind: kind.into(),
            defined_in_class: "".into(),
        };
        assert_eq!(prop_kind_from_field(&mk("IntProperty", 4)), PropKind::I32);
        assert_eq!(prop_kind_from_field(&mk("FloatProperty", 4)), PropKind::F32);
        assert_eq!(prop_kind_from_field(&mk("BoolProperty", 1)), PropKind::Bool);
        assert_eq!(prop_kind_from_field(&mk("ByteProperty", 1)), PropKind::U8);
        // Unknown → Bytes(size).
        assert_eq!(
            prop_kind_from_field(&mk("StructProperty", 16)),
            PropKind::Bytes(16)
        );
    }
}
