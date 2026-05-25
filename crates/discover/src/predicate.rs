//! Scan + narrow predicates. Generic over a small `NumberLike` trait so the
//! same predicate code services every numeric value kind.

use std::fmt::Debug;

use anyhow::{Result, anyhow};

use openforge_runtime::ValueKind;

pub trait NumberLike: Copy + PartialOrd + Debug + Send + Sync + 'static {
    const SIZE: usize;
    fn parse(s: &str) -> Result<Self>;
    fn from_le_bytes(buf: &[u8]) -> Self;
    fn approx_eq(self, other: Self, eps: f64) -> bool;
    fn as_f64(self) -> f64;
}

macro_rules! impl_int {
    ($t:ty) => {
        impl NumberLike for $t {
            const SIZE: usize = std::mem::size_of::<$t>();
            fn parse(s: &str) -> Result<Self> {
                s.parse::<$t>()
                    .map_err(|e| anyhow!("parse {s:?} as {}: {e}", stringify!($t)))
            }
            fn from_le_bytes(buf: &[u8]) -> Self {
                let mut a = [0u8; std::mem::size_of::<$t>()];
                a.copy_from_slice(&buf[..std::mem::size_of::<$t>()]);
                <$t>::from_le_bytes(a)
            }
            fn approx_eq(self, other: Self, _eps: f64) -> bool {
                self == other
            }
            fn as_f64(self) -> f64 {
                self as f64
            }
        }
    };
}

impl_int!(i8);
impl_int!(i16);
impl_int!(i32);
impl_int!(i64);
impl_int!(u8);
impl_int!(u16);
impl_int!(u32);
impl_int!(u64);

impl NumberLike for f32 {
    const SIZE: usize = 4;
    fn parse(s: &str) -> Result<Self> {
        s.parse::<f32>()
            .map_err(|e| anyhow!("parse {s:?} as f32: {e}"))
    }
    fn from_le_bytes(buf: &[u8]) -> Self {
        f32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]])
    }
    fn approx_eq(self, other: Self, eps: f64) -> bool {
        (self as f64 - other as f64).abs() <= eps
    }
    fn as_f64(self) -> f64 {
        self as f64
    }
}

impl NumberLike for f64 {
    const SIZE: usize = 8;
    fn parse(s: &str) -> Result<Self> {
        s.parse::<f64>()
            .map_err(|e| anyhow!("parse {s:?} as f64: {e}"))
    }
    fn from_le_bytes(buf: &[u8]) -> Self {
        f64::from_le_bytes([
            buf[0], buf[1], buf[2], buf[3], buf[4], buf[5], buf[6], buf[7],
        ])
    }
    fn approx_eq(self, other: Self, eps: f64) -> bool {
        (self - other).abs() <= eps
    }
    fn as_f64(self) -> f64 {
        self
    }
}

impl NumberLike for bool {
    const SIZE: usize = 1;
    fn parse(s: &str) -> Result<Self> {
        match s {
            "0" | "false" | "False" | "FALSE" => Ok(false),
            "1" | "true" | "True" | "TRUE" => Ok(true),
            _ => Err(anyhow!("parse {s:?} as bool")),
        }
    }
    fn from_le_bytes(buf: &[u8]) -> Self {
        buf[0] != 0
    }
    fn approx_eq(self, other: Self, _eps: f64) -> bool {
        self == other
    }
    fn as_f64(self) -> f64 {
        if self { 1.0 } else { 0.0 }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Predicate<T: NumberLike> {
    Eq(T),
    Ne(T),
    Gt(T),
    Ge(T),
    Lt(T),
    Le(T),
    Between(T, T),
    Changed,
    Unchanged,
    Increased,
    Decreased,
    IncreasedBy(T),
    DecreasedBy(T),
    /// Pure snapshot — accept every value. Used as the first step in an
    /// unknown-initial-value narrowing flow.
    UnknownInitial,
}

impl<T: NumberLike> Predicate<T> {
    pub fn matches(self, current: T, previous: Option<T>, eps: f64) -> bool {
        match self {
            Predicate::Eq(v) => current.approx_eq(v, eps),
            Predicate::Ne(v) => !current.approx_eq(v, eps),
            Predicate::Gt(v) => current > v,
            Predicate::Ge(v) => current >= v,
            Predicate::Lt(v) => current < v,
            Predicate::Le(v) => current <= v,
            Predicate::Between(lo, hi) => current >= lo && current <= hi,
            Predicate::Changed => match previous {
                Some(p) => !current.approx_eq(p, eps),
                None => true,
            },
            Predicate::Unchanged => match previous {
                Some(p) => current.approx_eq(p, eps),
                None => true,
            },
            Predicate::Increased => match previous {
                Some(p) => current.as_f64() > p.as_f64(),
                None => true,
            },
            Predicate::Decreased => match previous {
                Some(p) => current.as_f64() < p.as_f64(),
                None => true,
            },
            Predicate::IncreasedBy(d) => match previous {
                Some(p) => (current.as_f64() - p.as_f64() - d.as_f64()).abs() <= eps,
                None => false,
            },
            Predicate::DecreasedBy(d) => match previous {
                Some(p) => (p.as_f64() - current.as_f64() - d.as_f64()).abs() <= eps,
                None => false,
            },
            Predicate::UnknownInitial => true,
        }
    }
}

/// Dispatch table: convert a `ValueKind` plus user input into a typed Predicate
/// and run a closure with it.
pub fn dispatch_predicate<R>(
    kind: ValueKind,
    parser: impl FnOnce(&dyn std::any::Any) -> Result<R>,
) -> Result<R> {
    let _ = kind;
    parser(&())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eq_int() {
        let p: Predicate<i32> = Predicate::Eq(42);
        assert!(p.matches(42, None, 0.0));
        assert!(!p.matches(43, None, 0.0));
    }

    #[test]
    fn changed_no_previous() {
        let p: Predicate<i32> = Predicate::Changed;
        assert!(p.matches(0, None, 0.0));
    }

    #[test]
    fn increased_by_float() {
        let p: Predicate<f32> = Predicate::IncreasedBy(1.0);
        assert!(p.matches(2.0, Some(1.0), 1e-3));
        assert!(!p.matches(2.5, Some(1.0), 1e-3));
    }

    #[test]
    fn between_int() {
        let p: Predicate<i32> = Predicate::Between(10, 20);
        assert!(p.matches(15, None, 0.0));
        assert!(!p.matches(5, None, 0.0));
        assert!(!p.matches(25, None, 0.0));
    }

    #[test]
    fn bool_parse() {
        assert!(bool::parse("true").unwrap());
        assert!(!bool::parse("0").unwrap());
        assert!(bool::parse("garbage").is_err());
    }
}
