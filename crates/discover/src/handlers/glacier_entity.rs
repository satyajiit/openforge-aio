//! `openforge-discover glacier-entity` — walk a LIVE Glacier 2 entity instance.
//!
//! Given a `ZEntityImpl` VA, decode its (tagged) `m_pEntityType`, then its
//! `ZEntityType`'s `SPropertyData` array, exposing each property's per-instance
//! byte offset. With `--prop` it resolves a single named property by CRC32 and
//! dumps the live value at both candidate value-bases (`entity` and `entity+8`)
//! so the instance-layer offset chain can be validated end-to-end against a
//! known field. External RPM, no DLL.

use std::process::ExitCode;

use anyhow::Result;
use openforge_core::Target;
use openforge_glacier_host::{E_HAS_GETTER_SETTER, GlacierReflection, ResolvedGlacierField};

use crate::cli::GlacierEntityArgs;
use crate::context::DiscoverContext;
use crate::handlers::pick::parse_hex_addr;
use crate::term;

pub fn run(ctx: &DiscoverContext, args: &GlacierEntityArgs) -> Result<ExitCode> {
    let entity_va = parse_hex_addr(&args.address)? as u64;
    let candidates: Vec<&str> = ctx
        .manifest
        .game
        .process_names
        .iter()
        .map(String::as_str)
        .collect();
    let target = Target::attach_by_candidates(&candidates)?;
    let refl = GlacierReflection::new(&target);

    term::header(&format!(
        "glacier-entity: {} @ 0x{entity_va:X}",
        target.process_name
    ));
    term::ok(&format!(
        "attached pid {} ({})",
        target.pid, target.process_name
    ));

    // Raw vs untagged m_pEntityType — shows whether the tag bit was set.
    let raw_slot = refl
        .read_raw(entity_va.wrapping_add(0x08), 8)
        .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
        .unwrap_or(0);
    let entity_type_va = match refl.entity_type_va(entity_va) {
        Ok(v) => v,
        Err(e) => {
            term::bullet(format!("could not read m_pEntityType: {e}"));
            return Ok(ExitCode::FAILURE);
        }
    };
    term::header("== entity ==");
    term::bullet(format!(
        "m_pEntityType (raw) : 0x{raw_slot:X}  (tag bit {})",
        raw_slot & 1
    ));
    term::bullet(format!("ZEntityType (untag) : 0x{entity_type_va:X}"));

    let (ent, props) = match refl.instance_properties(entity_va) {
        Ok(v) => v,
        Err(e) => {
            term::bullet(format!("property-data walk failed: {e}"));
            return Ok(ExitCode::FAILURE);
        }
    };
    term::bullet(format!("SPropertyData count : {}", props.len()));
    if props.is_empty() {
        term::dim(
            "  (no properties — not a live ZEntityImpl, freed, or the instance \
             layout differs on this build)",
        );
        return Ok(ExitCode::FAILURE);
    }

    if let Some(name) = &args.prop {
        let Some(field) = refl.resolve_instance_property(entity_va, name)? else {
            term::bullet(format!(
                "property {name:?} (crc=0x{:08X}) not found among {} entries",
                openforge_glacier_host::crc32_property_id(name),
                props.len()
            ));
            return Ok(ExitCode::FAILURE);
        };
        print_field(&field);
        // Dump the live value at both candidate value-bases. The CANONICAL base
        // is m_pObj + offset (m_pObj == entity + 8, per ZHMModSDK
        // ZEntityRef::GetProperty — see EntityRef::value_addr); entity + offset
        // is shown only as a sanity contrast. Whichever yields a sane value for
        // a known field confirms the base on this build.
        for (label, addr) in [
            ("m_pObj + offset (canonical)", ent.value_addr(field.offset)),
            (
                "entity + offset (contrast)",
                (ent.entity_va as i64).wrapping_add(field.offset) as u64,
            ),
        ] {
            match refl.read_raw(addr, args.width) {
                Ok(bytes) => {
                    let hex: Vec<String> = bytes.iter().map(|b| format!("{b:02X}")).collect();
                    let as_u32 = if bytes.len() >= 4 {
                        u32::from_le_bytes(bytes[..4].try_into().unwrap())
                    } else {
                        0
                    };
                    let as_f32 = f32::from_bits(as_u32);
                    term::bullet(format!(
                        "[{label:<19}] 0x{addr:X}: {}  (u32={as_u32} / f32={as_f32})",
                        hex.join(" ")
                    ));
                }
                Err(e) => term::bullet(format!("[{label:<19}] 0x{addr:X}: <unreadable: {e}>")),
            }
        }
    } else {
        term::header("== properties (per-instance SPropertyData) ==");
        let n = props.len().min(args.limit);
        for (i, f) in props.iter().take(n).enumerate() {
            let name = f.name.as_deref().unwrap_or("?");
            let ty = f.type_name.as_deref().unwrap_or("?");
            let gs = if f.has_getter_setter {
                " [getter/setter]"
            } else {
                ""
            };
            term::bullet(format!(
                "[{i:>3}] +0x{:<5X} crc=0x{:08X} {:<40} {}{gs}",
                f.offset, f.crc32, name, ty
            ));
        }
        if props.len() > n {
            term::dim(format!("  ... and {} more", props.len() - n));
        }
    }

    Ok(ExitCode::SUCCESS)
}

fn print_field(f: &ResolvedGlacierField) {
    term::header("== property ==");
    term::bullet(format!(
        "name           : {}",
        f.name.as_deref().unwrap_or("?")
    ));
    term::bullet(format!("crc32          : 0x{:08X}", f.crc32));
    term::bullet(format!("offset         : +0x{:X} ({})", f.offset, f.offset));
    term::bullet(format!("sprop_flags    : 0x{:X}", f.sprop_flags));
    match f.descriptor_flags {
        Some(d) => term::bullet(format!(
            "descriptor flag: 0x{d:X}  (getter/setter={})",
            d & E_HAS_GETTER_SETTER != 0
        )),
        None => term::bullet("descriptor flag: <unreadable>".to_string()),
    }
    term::bullet(format!(
        "type           : {}",
        f.type_name.as_deref().unwrap_or("?")
    ));
}
