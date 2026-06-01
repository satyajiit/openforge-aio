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
//! Phase 2 ships only [`ConfigFormat::Toml`]; the RON variant lands in a later
//! phase as a new enum arm + `match` branch here, with no caller changes.

use serde::{Deserialize, Serialize};

/// On-disk config format of a signature / manifest source string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigFormat {
    /// TOML — the only format shipped today.
    #[default]
    Toml,
}

impl ConfigFormat {
    /// Map a file extension (no leading dot, any case) to a format.
    /// Returns `None` for unrecognized extensions.
    pub fn from_extension(ext: &str) -> Option<Self> {
        if ext.eq_ignore_ascii_case("toml") {
            Some(ConfigFormat::Toml)
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
        assert_eq!(ConfigFormat::from_extension("ron"), None);
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
    fn default_is_toml() {
        assert_eq!(ConfigFormat::default(), ConfigFormat::Toml);
    }
}
