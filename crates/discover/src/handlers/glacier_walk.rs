//! `openforge-discover glacier-walk` — resolve a Glacier 2 type by name through
//! the live `ZTypeRegistry` (external RPM, no DLL) and enumerate its properties.
//! Turns the validated reflection bootstrap into reusable tooling and doubles as
//! the harness for validating the IClassType/property offsets on a new build.

use std::process::ExitCode;

use anyhow::Result;
use openforge_core::Target;
use openforge_glacier_host::{GlacierReflection, flag_names};

use crate::cli::GlacierWalkArgs;
use crate::context::DiscoverContext;
use crate::term;

pub fn run(ctx: &DiscoverContext, args: &GlacierWalkArgs) -> Result<ExitCode> {
    let candidates: Vec<&str> = ctx
        .manifest
        .game
        .process_names
        .iter()
        .map(String::as_str)
        .collect();
    let target = Target::attach_by_candidates(&candidates)?;
    let base = target.main().base as u64;

    term::header(&format!(
        "glacier-walk: {} → type {:?}",
        target.process_name, args.r#type
    ));
    term::ok(&format!(
        "attached pid {} ({}), module base 0x{base:X}",
        target.pid, target.process_name,
    ));

    let refl = GlacierReflection::new(&target);

    let Some(ty) = refl.resolve_type(&args.r#type)? else {
        term::bullet(format!(
            "type {:?} not found (no name string in module, or no validated IType)",
            args.r#type
        ));
        return Ok(ExitCode::FAILURE);
    };

    term::header("== type ==");
    term::bullet(format!("name        : {}", ty.name));
    term::bullet(format!(
        "IType       : 0x{:X}  ({}+0x{:X})",
        ty.itype_va,
        target.process_name,
        ty.itype_va.saturating_sub(base)
    ));
    term::bullet(format!("STypeID     : 0x{:X}", ty.stypeid_va));
    term::bullet(format!("name string : 0x{:X}", ty.name_string_va));
    term::bullet(format!(
        "flags       : 0x{:04X}  ({})",
        ty.flags,
        flag_names(ty.flags)
    ));
    term::bullet(format!("size/align  : 0x{:X} / {}", ty.size, ty.align));
    term::bullet(format!(
        "resolved via: {}",
        if ty.via_static {
            "static IType slot (module)"
        } else {
            "registry node (heap)"
        }
    ));

    let (nprops, pprops, props) = refl.enumerate_properties(&ty)?;
    term::header("== properties (IClassType layout, validated on First Light) ==");
    term::bullet(format!(
        "m_nProperties=0x{nprops:X}  m_pProperties=0x{pprops:X}"
    ));
    if props.is_empty() {
        term::dim("  (no properties decoded — not a class type, or offsets need correction)");
    } else {
        let n = props.len().min(args.limit);
        for p in props.iter().take(n) {
            let ty_str = p.type_name.as_deref().unwrap_or("?");
            term::bullet(format!(
                "[{:>3}] {:<40} crc=0x{:08X} flags=0x{:X} type={}",
                p.index, p.name, p.crc32, p.flags, ty_str
            ));
        }
        if props.len() > n {
            term::dim(format!("  ... and {} more", props.len() - n));
        }
    }

    if args.raw {
        let bytes = refl.raw_itype(&ty, 0x80)?;
        term::header("== raw IType header (0x80 bytes) ==");
        for (row, chunk) in bytes.chunks(16).enumerate() {
            let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02X}")).collect();
            term::bullet(format!("+0x{:02X}: {}", row * 16, hex.join(" ")));
        }
    }

    Ok(ExitCode::SUCCESS)
}
