use crate::ctx::Ctx;
use crate::error::{Error, Result};

/// How to turn an AOB match address into the live target address.
#[derive(Debug, Clone, Copy)]
pub enum Resolve {
    /// The match address itself is the target.
    Direct,

    /// `target = match_addr + delta` (signed).
    Offset { delta: i64 },

    /// x64 RIP-relative addressing:
    /// `target = (match_addr + instruction_offset + operand_size) + sign_extend(displacement)`
    /// where `displacement` is read from `match_addr + instruction_offset`.
    RipRelative {
        instruction_offset: usize,
        operand_size: usize,
    },
}

impl Resolve {
    pub fn apply(&self, ctx: &dyn Ctx, match_addr: usize) -> Result<usize> {
        match *self {
            Resolve::Direct => Ok(match_addr),
            Resolve::Offset { delta } => Ok((match_addr as i64).wrapping_add(delta) as usize),
            Resolve::RipRelative {
                instruction_offset,
                operand_size,
            } => {
                let disp_addr = match_addr + instruction_offset;
                let mut buf = [0u8; 8];
                let slice = &mut buf[..operand_size];
                ctx.read_bytes(disp_addr, slice)?;
                let disp: i64 = match operand_size {
                    1 => buf[0] as i8 as i64,
                    2 => i16::from_le_bytes([buf[0], buf[1]]) as i64,
                    4 => i32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as i64,
                    n => {
                        return Err(Error::BadPattern(format!(
                            "rip_relative operand_size must be 1, 2, or 4; got {n}"
                        )));
                    }
                };
                let next_rip = match_addr + instruction_offset + operand_size;
                Ok((next_rip as i64 + disp) as usize)
            }
        }
    }
}
