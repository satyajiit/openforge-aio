//! Round-trip writer for signature TOMLs.
//!
//! Hand-formatted (rather than `toml::to_string`) so we can guarantee section
//! ordering — meta → value → write → locator → pointer_chain — and inline-table
//! style on the locator's `resolve`.

use std::fmt::Write;

use openforge_runtime::{Value, ValueKind};

#[derive(Debug, Clone)]
pub struct SignatureDoc {
    pub meta: MetaSection,
    pub value: Option<ValueSection>,
    pub write: WriteSection,
    pub locator: LocatorSection,
    pub pointer_chain: Option<PointerChainSection>,
}

#[derive(Debug, Clone)]
pub struct MetaSection {
    pub feature: String,
    pub display_name: String,
    pub description: Option<String>,
    pub tier: String,
    pub discovered_in_version: Option<String>,
    pub verified_versions: Vec<String>,
    pub discovered_on: Option<String>,
    pub discovered_by: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ValueSection {
    pub kind: ValueKind,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub display: String,
    pub endian: String,
}

#[derive(Debug, Clone)]
pub enum WriteSection {
    OneShot,
    Freeze {
        interval_ms: u32,
    },
    CodePatch {
        original_bytes: String,
        patched_bytes: String,
    },
}

#[derive(Debug, Clone)]
pub struct LocatorSection {
    pub pattern: String,
    pub resolve_kind: String,
    pub instruction_offset: Option<usize>,
    pub operand_size: Option<usize>,
    pub delta: Option<i64>,
    pub module: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PointerChainSection {
    pub hops: Vec<HopSection>,
}

#[derive(Debug, Clone)]
pub struct HopSection {
    pub deref: bool,
    pub offset: i64,
}

impl SignatureDoc {
    pub fn to_toml(&self) -> String {
        let mut s = String::new();
        // [meta]
        let _ = writeln!(s, "[meta]");
        let _ = writeln!(s, "feature      = {}", toml_str(&self.meta.feature));
        let _ = writeln!(s, "display_name = {}", toml_str(&self.meta.display_name));
        if let Some(d) = &self.meta.description {
            let _ = writeln!(s, "description  = {}", toml_str(d));
        }
        let _ = writeln!(s, "tier         = {}", toml_str(&self.meta.tier));
        if let Some(v) = &self.meta.discovered_in_version {
            let _ = writeln!(s, "discovered_in_version = {}", toml_str(v));
        }
        let _ = writeln!(
            s,
            "verified_versions = [{}]",
            self.meta
                .verified_versions
                .iter()
                .map(|v| toml_str(v))
                .collect::<Vec<_>>()
                .join(", ")
        );
        if let Some(d) = &self.meta.discovered_on {
            let _ = writeln!(s, "discovered_on = {}", toml_str(d));
        }
        if let Some(d) = &self.meta.discovered_by {
            let _ = writeln!(s, "discovered_by = {}", toml_str(d));
        }

        if let Some(v) = &self.value {
            let _ = writeln!(s);
            let _ = writeln!(s, "[value]");
            let _ = writeln!(s, "type    = {}", toml_str(v.kind.label()));
            if let Some(m) = v.min {
                let _ = writeln!(s, "min     = {}", format_num(m));
            }
            if let Some(m) = v.max {
                let _ = writeln!(s, "max     = {}", format_num(m));
            }
            let _ = writeln!(s, "display = {}", toml_str(&v.display));
            let _ = writeln!(s, "endian  = {}", toml_str(&v.endian));
        }

        let _ = writeln!(s);
        let _ = writeln!(s, "[write]");
        match &self.write {
            WriteSection::OneShot => {
                let _ = writeln!(s, "strategy = \"one_shot\"");
            }
            WriteSection::Freeze { interval_ms } => {
                let _ = writeln!(s, "strategy    = \"freeze\"");
                let _ = writeln!(s, "interval_ms = {interval_ms}");
            }
            WriteSection::CodePatch {
                original_bytes,
                patched_bytes,
            } => {
                let _ = writeln!(s, "strategy       = \"code_patch\"");
                let _ = writeln!(s, "original_bytes = {}", toml_str(original_bytes));
                let _ = writeln!(s, "patched_bytes  = {}", toml_str(patched_bytes));
            }
        }

        let _ = writeln!(s);
        let _ = writeln!(s, "[locator]");
        let _ = writeln!(s, "pattern = {}", toml_str(&self.locator.pattern));
        match self.locator.resolve_kind.as_str() {
            "rip_relative" => {
                let off = self.locator.instruction_offset.unwrap_or(0);
                let sz = self.locator.operand_size.unwrap_or(4);
                let _ = writeln!(
                    s,
                    "resolve = {{ kind = \"rip_relative\", instruction_offset = {off}, operand_size = {sz} }}"
                );
            }
            "offset" => {
                let d = self.locator.delta.unwrap_or(0);
                let _ = writeln!(s, "resolve = {{ kind = \"offset\", delta = {d} }}");
            }
            _ => {
                let _ = writeln!(s, "resolve = {{ kind = \"direct\" }}");
            }
        }
        if let Some(m) = &self.locator.module {
            let _ = writeln!(s, "module  = {}", toml_str(m));
        }

        if let Some(chain) = &self.pointer_chain {
            for hop in &chain.hops {
                let _ = writeln!(s);
                let _ = writeln!(s, "[[pointer_chain.hops]]");
                let _ = writeln!(s, "deref  = {}", hop.deref);
                let _ = writeln!(s, "offset = 0x{:X}", hop.offset);
            }
        }

        s
    }
}

fn toml_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn format_num(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e18 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

#[allow(dead_code)]
pub fn placeholder_value(kind: ValueKind) -> Value {
    match kind {
        ValueKind::Bool => Value::Bool(false),
        ValueKind::U8 => Value::U8(0),
        ValueKind::U16 => Value::U16(0),
        ValueKind::U32 => Value::U32(0),
        ValueKind::U64 => Value::U64(0),
        ValueKind::I8 => Value::I8(0),
        ValueKind::I16 => Value::I16(0),
        ValueKind::I32 => Value::I32(0),
        ValueKind::I64 => Value::I64(0),
        ValueKind::F32 => Value::F32(0.0),
        ValueKind::F64 => Value::F64(0.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_basic_one_shot() {
        let doc = SignatureDoc {
            meta: MetaSection {
                feature: "studs".into(),
                display_name: "Studs".into(),
                description: Some("Primary LEGO currency".into()),
                tier: "currency".into(),
                discovered_in_version: Some("1.0.0.1".into()),
                verified_versions: vec!["1.0.0.1".into()],
                discovered_on: Some("2026-05-23".into()),
                discovered_by: None,
            },
            value: Some(ValueSection {
                kind: ValueKind::I32,
                min: Some(0.0),
                max: Some(999_999_999.0),
                display: "thousands".into(),
                endian: "little".into(),
            }),
            write: WriteSection::OneShot,
            locator: LocatorSection {
                pattern: "48 8B 05 ?? ?? ?? ?? 89 88 ?? ?? ?? ?? 48 85 C0".into(),
                resolve_kind: "rip_relative".into(),
                instruction_offset: Some(3),
                operand_size: Some(4),
                delta: None,
                module: None,
            },
            pointer_chain: None,
        };
        let s = doc.to_toml();
        // Round-trip through openforge_runtime parser.
        let parsed = openforge_runtime::SignatureSpec::parse(&s).expect("parse round-trip");
        assert_eq!(parsed.meta.feature, "studs");
        assert_eq!(
            parsed.locator.expect("emit produces a locator").pattern,
            doc.locator.pattern
        );
    }
}
