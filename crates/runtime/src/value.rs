use serde::{Deserialize, Serialize};

use crate::error::{RuntimeError, RuntimeResult};

/// Format-independent hex / bit-pattern integer deserializers (the `FlexInt`
/// helpers from `docs/ENGINE-FORMAT-ARCHITECTURE.md` §2.3).
///
/// A `heap_scan` fingerprint sentinel like `0xDEADBEEF` is *32/64 bits*, not an
/// arithmetic value: the author means "these bits". Neither TOML nor RON can
/// express that intent for every width natively — TOML rejects `-0x1` and
/// overflows a bare `0xDEADBEEF` into an `i32`; RON range-errors a bare hex
/// token into a narrow field. So the *schema*, not the grammar, owns the fix:
/// a hex-bearing field uses `#[serde(deserialize_with = "hex_bits::as_iNN")]`
/// and the author writes the **quoted string** form (`"0xDEADBEEF"`, `"-0x1"`)
/// in both formats for guaranteed-identical bit-reinterpret semantics.
///
/// The deserializer accepts EITHER:
/// * an integer token (TOML `0x142C7E408`, RON `104`, any decimal) — delegated
///   to the target width's own parse so existing decimals stay byte-stable, or
/// * a string (`"0xDEADBEEF"`, `"-0x1"`, `"0b1010"`, `"0o17"`, `"-17"`, `"42"`)
///   — the magnitude is parsed as `u128`, `wrapping_neg`'d when a leading `-`
///   is present, then cast (`as`) to the target width (pure bit reinterpret).
///
/// Width is chosen by which helper the FIELD names (type-directed) — never
/// inferred — so sign/width can't be silently wrong.
pub mod hex_bits {
    use std::fmt;

    use serde::de::{self, Deserializer, Visitor};

    /// Parse a string magnitude (optional leading `-`, optional `0x`/`0b`/`0o`
    /// radix prefix, optional `_` digit separators) into a `u128`, returning
    /// `(was_negative, magnitude)`.
    fn parse_magnitude<E: de::Error>(raw: &str) -> Result<(bool, u128), E> {
        let s = raw.trim();
        let (neg, rest) = match s.strip_prefix('-') {
            Some(r) => (true, r.trim_start()),
            None => (false, s),
        };
        let rest = rest.strip_prefix('+').unwrap_or(rest);
        let (radix, digits) =
            if let Some(h) = rest.strip_prefix("0x").or_else(|| rest.strip_prefix("0X")) {
                (16u32, h)
            } else if let Some(b) = rest.strip_prefix("0b").or_else(|| rest.strip_prefix("0B")) {
                (2u32, b)
            } else if let Some(o) = rest.strip_prefix("0o").or_else(|| rest.strip_prefix("0O")) {
                (8u32, o)
            } else {
                (10u32, rest)
            };
        let cleaned: String = digits.chars().filter(|c| *c != '_').collect();
        if cleaned.is_empty() {
            return Err(de::Error::custom(format!("empty integer literal `{raw}`")));
        }
        let mag = u128::from_str_radix(&cleaned, radix)
            .map_err(|e| de::Error::custom(format!("invalid integer `{raw}`: {e}")))?;
        Ok((neg, mag))
    }

    /// Reinterpret a parsed `(was_negative, magnitude)` into the target width
    /// `T` via `cast`, applying `wrapping_neg` on the unsigned magnitude when
    /// the literal was negative.
    fn reinterp_str<T, E: de::Error>(raw: &str, cast: impl Fn(u128) -> T) -> Result<T, E> {
        let (neg, mag) = parse_magnitude::<E>(raw)?;
        let bits = if neg { mag.wrapping_neg() } else { mag };
        Ok(cast(bits))
    }

    /// A visitor that accepts an integer token (any of the serde integer
    /// hooks) OR a string, routing each to `cast` for bit reinterpretation.
    struct FlexVisitor<T, F: Fn(u128) -> T> {
        cast: F,
    }

    impl<'de, T, F: Fn(u128) -> T> Visitor<'de> for FlexVisitor<T, F> {
        type Value = T;

        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("an integer or a hex/decimal string like \"0xDEADBEEF\" or \"-0x1\"")
        }

        fn visit_i64<E: de::Error>(self, v: i64) -> Result<T, E> {
            Ok((self.cast)(v as u128))
        }
        fn visit_i128<E: de::Error>(self, v: i128) -> Result<T, E> {
            Ok((self.cast)(v as u128))
        }
        fn visit_u64<E: de::Error>(self, v: u64) -> Result<T, E> {
            Ok((self.cast)(v as u128))
        }
        fn visit_u128<E: de::Error>(self, v: u128) -> Result<T, E> {
            Ok((self.cast)(v))
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<T, E> {
            reinterp_str::<T, E>(v, &self.cast)
        }
        fn visit_string<E: de::Error>(self, v: String) -> Result<T, E> {
            reinterp_str::<T, E>(&v, &self.cast)
        }
    }

    fn reinterp<'de, D, T, F>(d: D, cast: F) -> Result<T, D::Error>
    where
        D: Deserializer<'de>,
        F: Fn(u128) -> T,
    {
        d.deserialize_any(FlexVisitor { cast })
    }

    pub fn as_i32<'de, D: Deserializer<'de>>(d: D) -> Result<i32, D::Error> {
        reinterp(d, |u| u as i32)
    }
    pub fn as_u32<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
        reinterp(d, |u| u as u32)
    }
    pub fn as_i64<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
        reinterp(d, |u| u as i64)
    }
    pub fn as_u64<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
        reinterp(d, |u| u as u64)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ValueKind {
    Bool,
    U8,
    U16,
    U32,
    U64,
    I8,
    I16,
    I32,
    I64,
    F32,
    F64,
}

impl ValueKind {
    pub fn size(self) -> usize {
        match self {
            ValueKind::Bool | ValueKind::U8 | ValueKind::I8 => 1,
            ValueKind::U16 | ValueKind::I16 => 2,
            ValueKind::U32 | ValueKind::I32 | ValueKind::F32 => 4,
            ValueKind::U64 | ValueKind::I64 | ValueKind::F64 => 8,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ValueKind::Bool => "bool",
            ValueKind::U8 => "u8",
            ValueKind::U16 => "u16",
            ValueKind::U32 => "u32",
            ValueKind::U64 => "u64",
            ValueKind::I8 => "i8",
            ValueKind::I16 => "i16",
            ValueKind::I32 => "i32",
            ValueKind::I64 => "i64",
            ValueKind::F32 => "f32",
            ValueKind::F64 => "f64",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v", rename_all = "lowercase")]
pub enum Value {
    Bool(bool),
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    I8(i8),
    I16(i16),
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
}

impl Value {
    pub fn kind(&self) -> ValueKind {
        match self {
            Value::Bool(_) => ValueKind::Bool,
            Value::U8(_) => ValueKind::U8,
            Value::U16(_) => ValueKind::U16,
            Value::U32(_) => ValueKind::U32,
            Value::U64(_) => ValueKind::U64,
            Value::I8(_) => ValueKind::I8,
            Value::I16(_) => ValueKind::I16,
            Value::I32(_) => ValueKind::I32,
            Value::I64(_) => ValueKind::I64,
            Value::F32(_) => ValueKind::F32,
            Value::F64(_) => ValueKind::F64,
        }
    }

    pub fn as_f64(&self) -> f64 {
        match *self {
            Value::Bool(b) => b as i32 as f64,
            Value::U8(v) => v as f64,
            Value::U16(v) => v as f64,
            Value::U32(v) => v as f64,
            Value::U64(v) => v as f64,
            Value::I8(v) => v as f64,
            Value::I16(v) => v as f64,
            Value::I32(v) => v as f64,
            Value::I64(v) => v as f64,
            Value::F32(v) => v as f64,
            Value::F64(v) => v,
        }
    }

    pub fn to_le_bytes(&self) -> Vec<u8> {
        match *self {
            Value::Bool(b) => vec![if b { 1 } else { 0 }],
            Value::U8(v) => v.to_le_bytes().to_vec(),
            Value::U16(v) => v.to_le_bytes().to_vec(),
            Value::U32(v) => v.to_le_bytes().to_vec(),
            Value::U64(v) => v.to_le_bytes().to_vec(),
            Value::I8(v) => v.to_le_bytes().to_vec(),
            Value::I16(v) => v.to_le_bytes().to_vec(),
            Value::I32(v) => v.to_le_bytes().to_vec(),
            Value::I64(v) => v.to_le_bytes().to_vec(),
            Value::F32(v) => v.to_le_bytes().to_vec(),
            Value::F64(v) => v.to_le_bytes().to_vec(),
        }
    }

    pub fn from_le_bytes(kind: ValueKind, buf: &[u8]) -> RuntimeResult<Value> {
        if buf.len() < kind.size() {
            return Err(RuntimeError::KindMismatch {
                expected: format!("{} bytes for {:?}", kind.size(), kind),
                got: format!("{} bytes", buf.len()),
            });
        }
        let v = match kind {
            ValueKind::Bool => Value::Bool(buf[0] != 0),
            ValueKind::U8 => Value::U8(buf[0]),
            ValueKind::U16 => Value::U16(u16::from_le_bytes([buf[0], buf[1]])),
            ValueKind::U32 => Value::U32(u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]])),
            ValueKind::U64 => Value::U64(u64::from_le_bytes([
                buf[0], buf[1], buf[2], buf[3], buf[4], buf[5], buf[6], buf[7],
            ])),
            ValueKind::I8 => Value::I8(buf[0] as i8),
            ValueKind::I16 => Value::I16(i16::from_le_bytes([buf[0], buf[1]])),
            ValueKind::I32 => Value::I32(i32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]])),
            ValueKind::I64 => Value::I64(i64::from_le_bytes([
                buf[0], buf[1], buf[2], buf[3], buf[4], buf[5], buf[6], buf[7],
            ])),
            ValueKind::F32 => Value::F32(f32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]])),
            ValueKind::F64 => Value::F64(f64::from_le_bytes([
                buf[0], buf[1], buf[2], buf[3], buf[4], buf[5], buf[6], buf[7],
            ])),
        };
        Ok(v)
    }

    /// Coerce to the requested kind. Used to accept numeric input from the UI
    /// that may have arrived as a generic JSON number.
    pub fn coerce(self, kind: ValueKind) -> RuntimeResult<Value> {
        if self.kind() == kind {
            return Ok(self);
        }
        let n = self.as_f64();
        let v = match kind {
            ValueKind::Bool => Value::Bool(n != 0.0),
            ValueKind::U8 => Value::U8(n as u8),
            ValueKind::U16 => Value::U16(n as u16),
            ValueKind::U32 => Value::U32(n as u32),
            ValueKind::U64 => Value::U64(n as u64),
            ValueKind::I8 => Value::I8(n as i8),
            ValueKind::I16 => Value::I16(n as i16),
            ValueKind::I32 => Value::I32(n as i32),
            ValueKind::I64 => Value::I64(n as i64),
            ValueKind::F32 => Value::F32(n as f32),
            ValueKind::F64 => Value::F64(n),
        };
        Ok(v)
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ValueRange {
    pub min: Option<f64>,
    pub max: Option<f64>,
}

impl ValueRange {
    pub fn check(&self, v: f64) -> RuntimeResult<()> {
        if let Some(lo) = self.min
            && v < lo
        {
            return Err(RuntimeError::OutOfRange {
                value: v,
                min: self.min,
                max: self.max,
            });
        }
        if let Some(hi) = self.max
            && v > hi
        {
            return Err(RuntimeError::OutOfRange {
                value: v,
                min: self.min,
                max: self.max,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod ron_probe {
    //! STEP 0 — empirical RON capability probe (de-risk gate for Phase 6).
    //! Verifies, against the real `ron` crate, that (a) the `hex_bits`
    //! `deserialize_any` helpers route string-vs-int correctly under RON and
    //! (b) every F6 tagged-enum representation round-trips from RON into the
    //! EXISTING structs. Run: `cargo test -p openforge-runtime --lib ron_probe`.
    use serde::Deserialize;

    // ---- (a) hex_bits via deserialize_with through RON --------------------

    #[derive(Debug, Deserialize)]
    struct HexI64 {
        #[serde(deserialize_with = "super::hex_bits::as_i64")]
        v: i64,
    }
    #[derive(Debug, Deserialize)]
    struct HexI32 {
        #[serde(deserialize_with = "super::hex_bits::as_i32")]
        v: i32,
    }
    #[derive(Debug, Deserialize)]
    struct HexU32 {
        #[serde(deserialize_with = "super::hex_bits::as_u32")]
        v: u32,
    }

    #[test]
    fn probe_a_hex_bits_ron_string_i64() {
        // 0xDEADBEEF = 3_735_928_559 — fits positive in i64.
        let h: HexI64 = ron::from_str(r#"(v: "0xDEADBEEF")"#).expect("ron parse i64 hex str");
        assert_eq!(h.v, 3_735_928_559_i64);
    }

    #[test]
    fn probe_a_hex_bits_ron_int_i64() {
        let h: HexI64 = ron::from_str("(v: 104)").expect("ron parse i64 int");
        assert_eq!(h.v, 104);
    }

    #[test]
    fn probe_a_hex_bits_ron_string_i32_reinterpret() {
        // 0xDEADBEEF cast as i32 = -559_038_737 (bit reinterpret).
        let h: HexI32 = ron::from_str(r#"(v: "0xDEADBEEF")"#).expect("ron parse i32 hex str");
        assert_eq!(h.v, -559_038_737_i32);
    }

    #[test]
    fn probe_a_hex_bits_ron_negative_hex_u32() {
        // -0x1 as u32 = 0xFFFF_FFFF.
        let h: HexU32 = ron::from_str(r#"(v: "-0x1")"#).expect("ron parse u32 neg hex");
        assert_eq!(h.v, 0xFFFF_FFFF_u32);
    }

    #[test]
    fn probe_a_hex_bits_toml_int_byte_stable() {
        // The retrofit: a bare TOML 0x integer token must still parse via the
        // SAME deserializer (delegates through the integer visitor hook).
        let h: HexI64 = toml::from_str("v = 0x142C7E408").expect("toml parse i64 bare hex int");
        assert_eq!(h.v, 0x1_42C7_E408_i64);
    }

    #[test]
    fn probe_a_hex_bits_toml_quoted_str() {
        let h: HexI64 = toml::from_str(r#"v = "0x142C7E408""#).expect("toml parse i64 quoted hex");
        assert_eq!(h.v, 0x1_42C7_E408_i64);
    }

    #[test]
    fn probe_a_hex_bits_negative_decimal_i32() {
        let h: HexI32 = ron::from_str(r#"(v: "-17")"#).expect("ron parse i32 neg dec str");
        assert_eq!(h.v, -17);
        let h2: HexI32 = ron::from_str("(v: -17)").expect("ron parse i32 neg dec int");
        assert_eq!(h2.v, -17);
    }

    // ---- (b) F6 tagged enums from RON into the EXISTING structs ------------

    #[test]
    fn probe_b_writespec_internally_tagged() {
        use crate::signature::WriteSpec;
        let one: WriteSpec =
            ron::from_str(r#"(strategy: "one_shot")"#).expect("ron WriteSpec one_shot");
        assert!(matches!(one, WriteSpec::OneShot));

        let fr: WriteSpec = ron::from_str(
            r#"(strategy: "freeze", interval_ms: 16, freeze_value: None, freeze_copy_offset: Some(104))"#,
        )
        .expect("ron WriteSpec freeze");
        match fr {
            WriteSpec::Freeze {
                interval_ms,
                freeze_value,
                freeze_copy_offset,
            } => {
                assert_eq!(interval_ms, 16);
                assert_eq!(freeze_value, None);
                assert_eq!(freeze_copy_offset, Some(104));
            }
            other => panic!("expected Freeze, got {other:?}"),
        }
    }

    #[test]
    fn probe_b_controlspec_internally_tagged() {
        use crate::signature::ControlSpec;
        let c: ControlSpec =
            ron::from_str(r#"(kind: "input", step: Some(0.05))"#).expect("ron ControlSpec input");
        match c {
            ControlSpec::Input { step, presets } => {
                assert_eq!(step, Some(0.05));
                assert!(presets.is_empty());
            }
            other => panic!("expected Input, got {other:?}"),
        }
    }

    #[test]
    fn probe_b_predicatespec_adjacently_tagged() {
        use crate::signature::PredicateSpec;
        // NUANCE (empirically established): RON deserializes an *adjacently*
        // tagged enum only when the tag is a BARE RON identifier matching the
        // serde-renamed variant name (`kind: fqn_prefix`), NOT a quoted string
        // (`kind: "fqn_prefix"` → `Expected identifier`). Internally-tagged
        // enums (WriteSpec/ControlSpec above) accept the quoted-string tag.
        // Authors of `.ron` reflection predicates must use the bare form.
        let p: PredicateSpec = ron::from_str(r#"(kind: fqn_prefix, value: "/Engine/Transient/")"#)
            .expect("ron PredicateSpec fqn_prefix (bare identifier tag)");
        assert!(matches!(p, PredicateSpec::FqnPrefix(ref s) if s == "/Engine/Transient/"));
    }

    #[test]
    fn probe_b_preset_untagged() {
        use crate::signature::Preset;
        let bare: Preset = ron::from_str("0.5").expect("ron Preset bare");
        assert_eq!(bare.value(), 0.5);

        let labeled: Preset =
            ron::from_str(r#"(label: "Tame", value: 0.7)"#).expect("ron Preset labeled");
        match labeled {
            Preset::Labeled { ref label, value } => {
                assert_eq!(label, "Tame");
                assert_eq!(value, 0.7);
            }
            other => panic!("expected Labeled, got {other:?}"),
        }
    }
}
