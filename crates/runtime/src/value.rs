use serde::{Deserialize, Serialize};

use crate::error::{RuntimeError, RuntimeResult};

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
