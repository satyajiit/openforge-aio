//! Config-format abstraction seam.
//!
//! Today every game signature and manifest is authored in TOML, but the
//! engine-format roadmap (see `docs/ENGINE-FORMAT-ARCHITECTURE.md` §5) calls
//! for a second on-disk format (RON) to be addable as a one-line registration.
//!
//! This module introduces that seam without changing any behavior:
//!
//! * [`ConfigFormat`] tags which format a chunk of source text is written in.
//! * [`parse_str`] is the single runtime entry point — it dispatches to the
//!   right deserializer for a [`ConfigFormat`]. The runtime parse paths
//!   ([`crate::signature::SignatureSpec::parse_with`],
//!   [`crate::manifest::GameManifest::parse_with`],
//!   [`crate::feature::DeclarativeFeature::from_source`]) all route through it.
//! * [`SpecFormat`] + [`TomlFormat`] document the per-format contract. The
//!   trait has a generic method (so it is **not** object-safe — never use
//!   `dyn SpecFormat`); the runtime uses the free function [`parse_str`]
//!   rather than dynamic dispatch.
//!
//! Phase 6 adds [`ConfigFormat::Ron`] as a fully-working second format: a new
//! enum arm, a [`parse_str`] match branch, and a [`RonFormat`] trait impl — no
//! caller changes. TOML stays the default and every shipped `.toml` flows
//! through the unchanged `toml::from_str` path.

use serde::{Deserialize, Serialize};

/// On-disk config format of a signature / manifest source string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigFormat {
    /// TOML — the default; every manifest and most signatures.
    #[default]
    Toml,
    /// RON (Rusty Object Notation) — additive second signature format. Uses
    /// the SAME `serde` derives as TOML (see `docs/ENGINE-FORMAT-ARCHITECTURE.md`
    /// §2/§5), so it is a true `ron::from_str::<SignatureSpec>` drop-in.
    Ron,
}

impl ConfigFormat {
    /// Map a file extension (no leading dot, any case) to a format.
    /// Returns `None` for unrecognized extensions.
    pub fn from_extension(ext: &str) -> Option<Self> {
        if ext.eq_ignore_ascii_case("toml") {
            Some(ConfigFormat::Toml)
        } else if ext.eq_ignore_ascii_case("ron") {
            Some(ConfigFormat::Ron)
        } else {
            None
        }
    }
}

/// Per-format parsing contract.
///
/// NOTE: [`SpecFormat::parse_value`] is generic, which makes this trait **not**
/// object-safe — do not attempt `dyn SpecFormat`. The runtime dispatches via
/// the free function [`parse_str`] instead. This trait exists to document the
/// contract each format implementation must satisfy (and to be unit-tested).
pub trait SpecFormat: Send + Sync {
    /// Which [`ConfigFormat`] this implementation handles.
    fn format(&self) -> ConfigFormat;
    /// File extensions (no leading dot, lowercase) this format claims.
    fn extensions(&self) -> &'static [&'static str];
    /// Deserialize `src` into `T`, returning a human-readable error string.
    fn parse_value<T: serde::de::DeserializeOwned>(&self, src: &str) -> Result<T, String>;
}

/// The TOML format implementation.
pub struct TomlFormat;

impl SpecFormat for TomlFormat {
    fn format(&self) -> ConfigFormat {
        ConfigFormat::Toml
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["toml"]
    }

    fn parse_value<T: serde::de::DeserializeOwned>(&self, src: &str) -> Result<T, String> {
        toml::from_str(src).map_err(|e| e.to_string())
    }
}

/// The RON format implementation. Mirrors [`TomlFormat`]; both target the
/// identical `serde` derives, so a `.ron` signature deserializes into the same
/// [`crate::signature::SignatureSpec`] as its `.toml` equivalent.
pub struct RonFormat;

impl SpecFormat for RonFormat {
    fn format(&self) -> ConfigFormat {
        ConfigFormat::Ron
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["ron"]
    }

    fn parse_value<T: serde::de::DeserializeOwned>(&self, src: &str) -> Result<T, String> {
        ron::from_str(src).map_err(|e| e.to_string())
    }
}

/// Runtime parse entry point: deserialize `src` into `T` using `fmt`.
///
/// This is the single dispatch point every runtime parse path funnels
/// through. Adding a format is a new `match` arm here.
pub fn parse_str<T: serde::de::DeserializeOwned>(
    src: &str,
    fmt: ConfigFormat,
) -> Result<T, String> {
    match fmt {
        ConfigFormat::Toml => toml::from_str(src).map_err(|e| e.to_string()),
        ConfigFormat::Ron => ron::from_str(src).map_err(|e| e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_str_toml_ok() {
        let v: toml::Value = parse_str("a = 1", ConfigFormat::Toml).expect("toml parse");
        assert_eq!(v.get("a").and_then(|x| x.as_integer()), Some(1));
    }

    #[test]
    fn parse_str_toml_err_is_string() {
        let r: Result<toml::Value, String> = parse_str("a = = =", ConfigFormat::Toml);
        assert!(r.is_err());
    }

    #[test]
    fn from_extension_case_insensitive() {
        assert_eq!(
            ConfigFormat::from_extension("TOML"),
            Some(ConfigFormat::Toml)
        );
        assert_eq!(
            ConfigFormat::from_extension("toml"),
            Some(ConfigFormat::Toml)
        );
        assert_eq!(ConfigFormat::from_extension("ron"), Some(ConfigFormat::Ron));
        assert_eq!(ConfigFormat::from_extension("RON"), Some(ConfigFormat::Ron));
        assert_eq!(ConfigFormat::from_extension("json"), None);
    }

    #[test]
    fn toml_format_trait_impl() {
        let f = TomlFormat;
        assert_eq!(f.format(), ConfigFormat::Toml);
        assert_eq!(f.extensions(), &["toml"]);
        let v: toml::Value = f.parse_value("a = 1").expect("toml parse via trait");
        assert_eq!(v.get("a").and_then(|x| x.as_integer()), Some(1));
    }

    #[test]
    fn ron_format_trait_impl() {
        let f = RonFormat;
        assert_eq!(f.format(), ConfigFormat::Ron);
        assert_eq!(f.extensions(), &["ron"]);
        #[derive(serde::Deserialize)]
        struct Holder {
            a: i32,
        }
        let v: Holder = f.parse_value("(a: 1)").expect("ron parse via trait");
        assert_eq!(v.a, 1);
    }

    #[test]
    fn parse_str_ron_signature_spec() {
        use crate::signature::{SignatureSpec, WriteSpec};
        // A minimal, valid heap_scan one_shot signature authored in RON,
        // exercising the hex-needle string (R3) + the value section. The
        // `#![enable(implicit_some)]` extension lets `Option<T>` fields be
        // written bare (`value: (...)` instead of `value: Some((...))`),
        // keeping the on-disk shape close to the TOML it mirrors.
        let src = r#"#![enable(implicit_some)]
        (
            meta: ( feature: "probe_sig", display_name: "Probe Sig" ),
            value: ( type: i32 ),
            write: ( strategy: "one_shot" ),
            heap_scan: ( value_type: u64, value: "0x142C7E408", field_offset: 104 ),
        )"#;
        let spec: SignatureSpec = parse_str(src, ConfigFormat::Ron).expect("ron SignatureSpec");
        spec.validate().expect("validate ron signature");
        assert_eq!(spec.meta.feature, "probe_sig");
        assert!(matches!(spec.write, WriteSpec::OneShot));
        let hs = spec.heap_scan.expect("heap_scan present");
        assert_eq!(hs.value, 0x1_42C7_E408_i64);
        assert_eq!(hs.field_offset, 104);
    }

    #[test]
    fn parse_str_ron_negative_hex_needle() {
        use crate::signature::SignatureSpec;
        // R3 negative bit-pattern coverage through the full RON signature path:
        // `value: "-0x1"` => hex_bits::as_i64 => -1 (0xFFFF_FFFF_FFFF_FFFF).
        // Mirrors crates/games/_template/signatures/hex_demo.ron.
        let src = r#"#![enable(implicit_some)]
        (
            meta: ( feature: "neg_hex", display_name: "Neg Hex" ),
            value: ( type: i64 ),
            write: ( strategy: "one_shot" ),
            heap_scan: ( value_type: i64, value: "-0x1", field_offset: 0, alignment: 8 ),
        )"#;
        let spec: SignatureSpec = parse_str(src, ConfigFormat::Ron).expect("ron neg-hex sig");
        spec.validate().expect("validate neg-hex sig");
        assert_eq!(spec.heap_scan.expect("heap_scan").value, -1_i64);
    }

    #[test]
    fn default_is_toml() {
        assert_eq!(ConfigFormat::default(), ConfigFormat::Toml);
    }
}
