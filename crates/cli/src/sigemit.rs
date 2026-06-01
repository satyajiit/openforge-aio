//! Hand-formatted signature stub emitters for `new-feature`.
//!
//! [`signature_stub`] emits a TOML skeleton; [`signature_stub_ron`] emits the
//! SAME field set in valid RON. `new-feature` picks one based on the game's
//! `[engine].config_format`. Both are skeletons the author fills in via
//! `openforge-discover` (no `heap_scan`/needle is pre-seeded).

use crate::cli::{FeatureKind, WriteStrategy};
use crate::date::today_iso;

pub struct StubArgs<'a> {
    pub feature: &'a str,
    pub display_name: &'a str,
    pub description: &'a str,
    pub tier: &'a str,
    pub kind: FeatureKind,
    pub strategy: WriteStrategy,
}

pub fn signature_stub(args: &StubArgs<'_>) -> String {
    let mut s = String::new();
    s.push_str("[meta]\n");
    s.push_str(&format!("feature      = {:?}\n", args.feature));
    s.push_str(&format!("display_name = {:?}\n", args.display_name));
    s.push_str(&format!("description  = {:?}\n", args.description));
    s.push_str(&format!("tier         = {:?}\n", args.tier));
    s.push_str("discovered_in_version = \"\"\n");
    s.push_str("verified_versions = []\n");
    s.push_str(&format!("discovered_on = {:?}\n", today_iso()));

    if !matches!(args.strategy, WriteStrategy::CodePatch) {
        s.push_str("\n[value]\n");
        s.push_str(&format!("type    = \"{}\"\n", kind_label(args.kind)));
        s.push_str("display = \"decimal\"\n");
        s.push_str("endian  = \"little\"\n");
    }

    s.push_str("\n[write]\n");
    match args.strategy {
        WriteStrategy::OneShot => s.push_str("strategy = \"one_shot\"\n"),
        WriteStrategy::Freeze => {
            s.push_str("strategy    = \"freeze\"\n");
            s.push_str("interval_ms = 250\n");
        }
        WriteStrategy::CodePatch => {
            s.push_str("strategy       = \"code_patch\"\n");
            s.push_str("original_bytes = \"\"\n");
            s.push_str("patched_bytes  = \"\"\n");
        }
    }

    s.push_str("\n[locator]\n");
    s.push_str("# Placeholder. Discover the real pattern via:\n");
    s.push_str(&format!(
        "#   openforge-discover --game <game> scan --feature {} --type {} --value <known> --name {:?}\n",
        args.feature,
        kind_label(args.kind),
        args.display_name
    ));
    s.push_str("pattern = \"?? ?? ??\"\n");
    s.push_str("resolve = { kind = \"direct\" }\n");

    s
}

/// RON twin of [`signature_stub`]. Emits the SAME field set in valid RON so a
/// game whose `[engine].config_format` is `Ron` scaffolds `.ron`. The output
/// parses via `openforge_runtime::SignatureSpec::parse_with(.., ConfigFormat::Ron)`.
///
/// RON authoring rules applied here (matching the shipped `.ron` signatures):
/// * `#![enable(implicit_some)]` header so `Option` fields (`step`, `module`,
///   …) can be written bare rather than `Some(..)`.
/// * The top-level struct + each section is a parenthesized `(field: value, …)`.
/// * `WriteSpec` / `ResolveSpec` are internally-tagged, so their tag is a
///   QUOTED string (`strategy: "one_shot"`, `resolve: (kind: "direct")`).
/// * `ValueKind` is a unit variant → a bare identifier (`type: i32`).
pub fn signature_stub_ron(args: &StubArgs<'_>) -> String {
    let mut s = String::new();
    s.push_str("#![enable(implicit_some)]\n(\n");

    s.push_str("    meta: (\n");
    s.push_str(&format!("        feature: {:?},\n", args.feature));
    s.push_str(&format!("        display_name: {:?},\n", args.display_name));
    s.push_str(&format!("        description: {:?},\n", args.description));
    s.push_str(&format!("        tier: {:?},\n", args.tier));
    s.push_str("        discovered_in_version: \"\",\n");
    s.push_str("        verified_versions: [],\n");
    s.push_str(&format!("        discovered_on: {:?},\n", today_iso()));
    s.push_str("    ),\n");

    if !matches!(args.strategy, WriteStrategy::CodePatch) {
        s.push_str("    value: (\n");
        s.push_str(&format!("        type: {},\n", kind_label(args.kind)));
        s.push_str("        display: \"decimal\",\n");
        s.push_str("        endian: little,\n");
        s.push_str("    ),\n");
    }

    match args.strategy {
        WriteStrategy::OneShot => {
            s.push_str("    write: (strategy: \"one_shot\"),\n");
        }
        WriteStrategy::Freeze => {
            s.push_str("    write: (strategy: \"freeze\", interval_ms: 250),\n");
        }
        WriteStrategy::CodePatch => {
            s.push_str(
                "    write: (strategy: \"code_patch\", original_bytes: \"\", patched_bytes: \"\"),\n",
            );
        }
    }

    s.push_str("    // Placeholder locator. Discover the real pattern via:\n");
    s.push_str(&format!(
        "    //   openforge-discover --game <game> scan --feature {} --type {} --value <known> --name {:?}\n",
        args.feature,
        kind_label(args.kind),
        args.display_name
    ));
    s.push_str("    locator: (\n");
    s.push_str("        pattern: \"?? ?? ??\",\n");
    s.push_str("        resolve: (kind: \"direct\"),\n");
    s.push_str("    ),\n");

    s.push_str(")\n");
    s
}

fn kind_label(k: FeatureKind) -> &'static str {
    match k {
        FeatureKind::I8 => "i8",
        FeatureKind::I16 => "i16",
        FeatureKind::I32 => "i32",
        FeatureKind::I64 => "i64",
        FeatureKind::U8 => "u8",
        FeatureKind::U16 => "u16",
        FeatureKind::U32 => "u32",
        FeatureKind::U64 => "u64",
        FeatureKind::F32 => "f32",
        FeatureKind::F64 => "f64",
        FeatureKind::Bool => "bool",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_shot_stub_parses() {
        let s = signature_stub(&StubArgs {
            feature: "studs",
            display_name: "Studs",
            description: "<describe this cheat>",
            tier: "currency",
            kind: FeatureKind::I32,
            strategy: WriteStrategy::OneShot,
        });
        let parsed = openforge_runtime::SignatureSpec::parse(&s).expect("parse");
        assert_eq!(parsed.meta.feature, "studs");
        assert_eq!(
            parsed.value.as_ref().unwrap().kind,
            openforge_runtime::ValueKind::I32
        );
    }

    #[test]
    fn code_patch_stub_parses_without_value_section() {
        let s = signature_stub(&StubArgs {
            feature: "god_mode",
            display_name: "God Mode",
            description: "Disable damage",
            tier: "combat",
            kind: FeatureKind::Bool,
            strategy: WriteStrategy::CodePatch,
        });
        let parsed = openforge_runtime::SignatureSpec::parse(&s).expect("parse");
        assert_eq!(parsed.meta.feature, "god_mode");
        assert!(parsed.value.is_none());
        assert!(matches!(
            parsed.write,
            openforge_runtime::WriteSpec::CodePatch { .. }
        ));
    }

    fn parse_ron(s: &str) -> openforge_runtime::SignatureSpec {
        openforge_runtime::SignatureSpec::parse_with(s, openforge_runtime::ConfigFormat::Ron)
            .expect("ron stub must parse")
    }

    #[test]
    fn one_shot_ron_stub_parses() {
        let s = signature_stub_ron(&StubArgs {
            feature: "studs",
            display_name: "Studs",
            description: "<describe this cheat>",
            tier: "currency",
            kind: FeatureKind::I32,
            strategy: WriteStrategy::OneShot,
        });
        let parsed = parse_ron(&s);
        parsed.validate().expect("validate ron one_shot stub");
        assert_eq!(parsed.meta.feature, "studs");
        assert_eq!(
            parsed.value.as_ref().unwrap().kind,
            openforge_runtime::ValueKind::I32
        );
        assert!(matches!(
            parsed.write,
            openforge_runtime::WriteSpec::OneShot
        ));
    }

    #[test]
    fn freeze_ron_stub_parses() {
        let s = signature_stub_ron(&StubArgs {
            feature: "lock_health",
            display_name: "Lock Health",
            description: "Freeze health",
            tier: "combat",
            kind: FeatureKind::F32,
            strategy: WriteStrategy::Freeze,
        });
        let parsed = parse_ron(&s);
        parsed.validate().expect("validate ron freeze stub");
        assert_eq!(parsed.meta.feature, "lock_health");
        assert!(matches!(
            parsed.write,
            openforge_runtime::WriteSpec::Freeze {
                interval_ms: 250,
                ..
            }
        ));
    }

    #[test]
    fn code_patch_ron_stub_parses_without_value_section() {
        let s = signature_stub_ron(&StubArgs {
            feature: "god_mode",
            display_name: "God Mode",
            description: "Disable damage",
            tier: "combat",
            kind: FeatureKind::Bool,
            strategy: WriteStrategy::CodePatch,
        });
        let parsed = parse_ron(&s);
        parsed.validate().expect("validate ron code_patch stub");
        assert_eq!(parsed.meta.feature, "god_mode");
        assert!(parsed.value.is_none());
        assert!(matches!(
            parsed.write,
            openforge_runtime::WriteSpec::CodePatch { .. }
        ));
    }
}
