//! Synthesize signature TOML files for UE5 reflection targets.
//!
//! These are *free functions* (not methods on `UeEngine`) so the CLI can use
//! either the legacy external `UeEngine` or the DLL-backed `Ue5Session` for
//! the walk phase, then call `emit_*` with whatever inputs they have. The
//! function set deliberately takes `&Target` directly — signature emission
//! still reads the target process from outside (vtable byte, `.text`
//! uniqueness check). That stays cross-process even in the DLL flow.

use std::fs;

use anyhow::{Result, bail};
use openforge_core::{Ctx, Target};
use openforge_ue5_protocol::PropInfo;
use tracing::info;

use crate::context::DiscoverContext;

/// Emit a heap_scan-locator signature TOML for a single class property.
///
/// `instance_ptr` is a **live UObject instance** of the class — we read its
/// vtable as the discrimination value, because every instance of a class
/// shares the same vtable but distinct from other classes. Reading the
/// vtable off the meta-object pointer is wrong: it points to the vtable of
/// `UClass`, not the user class.
///
/// `layout_validated` must be true (the resolved UE5 layout was probed
/// against a known UStruct). If false, the function refuses rather than
/// emit a TOML that might be silently bad.
pub fn emit_property_signature(
    target: &Target,
    ctx: &DiscoverContext,
    feature_id: &str,
    class_name: &str,
    instance_ptr: u64,
    prop: &PropInfo,
    layout_validated: bool,
) -> Result<()> {
    if !layout_validated {
        bail!(
            "Layout offsets were not validated for this process; refusing to emit signature for {}.",
            feature_id
        );
    }

    // Read instance vtable (first 8 bytes of any live UObject instance).
    let mut vtable_buf = [0u8; 8];
    target.read_bytes(instance_ptr as usize, &mut vtable_buf)?;
    let vtable = u64::from_le_bytes(vtable_buf) as usize;

    if vtable == 0 {
        bail!("Could not read instance vtable for {}", class_name);
    }

    // Module ranges for validator
    let main_module = target.main();
    let min_val = main_module.base;
    let max_val = main_module.base + main_module.size;

    // Map property type to ValueKind
    let val_kind = match prop.kind.as_str() {
        "IntProperty" => openforge_runtime::ValueKind::I32,
        "Int64Property" => openforge_runtime::ValueKind::I64,
        "FloatProperty" => openforge_runtime::ValueKind::F32,
        "DoubleProperty" => openforge_runtime::ValueKind::F64,
        "BoolProperty" => openforge_runtime::ValueKind::Bool,
        "ByteProperty" => openforge_runtime::ValueKind::U8,
        _ => openforge_runtime::ValueKind::I32, // Fallback
    };

    let mut s = String::new();
    s.push_str("[meta]\n");
    s.push_str(&format!("feature      = \"{}\"\n", feature_id));
    s.push_str(&format!(
        "display_name = \"{}\"\n",
        feature_id.replace('_', " ")
    ));
    s.push_str(&format!(
        "tagline      = \"Reflection-derived signature for class {}\"\n",
        class_name
    ));
    s.push_str("tier         = \"general\"\n");
    if let Some(v) = ctx.manifest.game.supported_versions.first() {
        s.push_str(&format!("discovered_in_version = \"{}\"\n", v));
    }
    s.push_str(&format!(
        "verified_versions = [{}]\n",
        ctx.manifest
            .game
            .supported_versions
            .iter()
            .map(|v| format!("\"{}\"", v))
            .collect::<Vec<_>>()
            .join(", ")
    ));

    s.push_str("\n[value]\n");
    s.push_str(&format!("type    = \"{}\"\n", val_kind.label()));
    s.push_str("display = \"decimal\"\n");
    s.push_str("endian  = \"little\"\n");

    s.push_str("\n[write]\n");
    s.push_str("strategy = \"one_shot\"\n");

    s.push_str("\n[heap_scan]\n");
    s.push_str("value_type   = \"u64\"\n");
    s.push_str(&format!("value        = {}\n", vtable));
    s.push_str(&format!("field_offset = {}\n", prop.offset));
    s.push_str("alignment    = 8\n");

    s.push_str("\n[[heap_scan.validators]]\n");
    s.push_str("field_type      = \"u64\"\n");
    s.push_str(&format!("relative_offset = -{}\n", prop.offset));
    s.push_str(&format!("min             = {}\n", min_val));
    s.push_str(&format!("max             = {}\n", max_val));

    let path = ctx.signature_path(feature_id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, &s)?;

    info!("Emitted property signature TOML to {}", path.display());
    Ok(())
}

/// Count exact matches of `bytes` within the main module's .text section.
/// Used to verify uniqueness of synthesized AOBs.
fn count_text_matches(target: &Target, bytes: &[u8]) -> Result<usize> {
    let main_module = target.main();
    let mut buf = vec![0u8; main_module.text_size];
    target.read_bytes(main_module.text_base(), &mut buf)?;
    if bytes.is_empty() || buf.len() < bytes.len() {
        return Ok(0);
    }
    let plen = bytes.len();
    let end = buf.len() - plen + 1;
    let mut count = 0usize;
    for i in 0..end {
        if buf[i] == bytes[0] && &buf[i..i + plen] == bytes {
            count += 1;
        }
    }
    Ok(count)
}

/// Emit a code_patch-locator signature TOML for a UFunction's native entry.
///
/// Synthesizes an AOB from the function's prologue bytes, widening the
/// window until the AOB is unique within `.text`. Refuses to emit when no
/// unique window exists at 64 bytes (rather than write a broken signature).
pub fn emit_ufunction_signature(
    target: &Target,
    ctx: &DiscoverContext,
    feature_id: &str,
    class_name: &str,
    func_name: &str,
    func_addr: u64,
    layout_validated: bool,
) -> Result<()> {
    if !layout_validated {
        bail!(
            "Layout offsets were not validated for this process; refusing to emit signature for {}.",
            feature_id
        );
    }

    // Try widening windows until we find a unique AOB. Every function shares
    // the same ABI prologue, so 16 bytes is rarely enough — widen and recount.
    let windows: &[usize] = &[16, 24, 32, 48, 64];
    let mut chosen: Option<(Vec<u8>, usize)> = None;
    let mut last_count = 0usize;

    for &w in windows {
        let mut buf = vec![0u8; w];
        target.read_bytes(func_addr as usize, &mut buf)?;
        let count = count_text_matches(target, &buf)?;
        last_count = count;
        if count == 1 {
            chosen = Some((buf, w));
            break;
        }
    }

    let (entry_bytes, window) = match chosen {
        Some(c) => c,
        None => bail!(
            "Synthesized AOB for {}::{} (func at 0x{:X}) was not unique even at 64 bytes; \
             last match count = {}. Refusing to write a broken signature.",
            class_name,
            func_name,
            func_addr,
            last_count
        ),
    };

    let aob_pattern = entry_bytes
        .iter()
        .map(|b| format!("{:02X}", b))
        .collect::<Vec<_>>()
        .join(" ");

    let mut s = String::new();
    s.push_str("[meta]\n");
    s.push_str(&format!("feature      = \"{}\"\n", feature_id));
    s.push_str(&format!(
        "display_name = \"{}\"\n",
        feature_id.replace('_', " ")
    ));
    s.push_str(&format!(
        "tagline      = \"God Mode via TakeDamage early RET (class {})\"\n",
        class_name
    ));
    s.push_str("tier         = \"combat\"\n");
    if let Some(v) = ctx.manifest.game.supported_versions.first() {
        s.push_str(&format!("discovered_in_version = \"{}\"\n", v));
    }

    s.push_str("\n[write]\n");
    s.push_str("strategy       = \"code_patch\"\n");
    let original_str = entry_bytes[0..6]
        .iter()
        .map(|b| format!("{:02X}", b))
        .collect::<Vec<_>>()
        .join(" ");
    s.push_str(&format!("original_bytes = \"{}\"\n", original_str));
    s.push_str("patched_bytes  = \"C3 90 90 90 90 90\"\n");

    s.push_str("\n[locator]\n");
    s.push_str(&format!("pattern = \"{}\"\n", aob_pattern));
    s.push_str("resolve = { kind = \"direct\" }\n");

    let path = ctx.signature_path(feature_id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, &s)?;

    info!(
        "Emitted UFunction signature TOML to {} (unique at {}-byte window)",
        path.display(),
        window
    );
    Ok(())
}
