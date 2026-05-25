//! AOB (array-of-bytes) signature synthesis.
//!
//! Given a byte sequence that begins at the writing/load instruction, decode
//! x86-64 instructions and wildcard volatile fields (immediate operands and
//! displacements) while keeping the stable opcode + ModR/M bytes. The first
//! instruction's RIP-relative displacement (if any) determines the `resolve`
//! metadata.
//!
//! We use `iced-x86` to verify each instruction decodes and to determine its
//! length, but we parse ModR/M ourselves to identify displacement bytes —
//! that path is reliable and small.

use anyhow::{Result, anyhow};
use iced_x86::{Decoder, DecoderOptions, Instruction, OpKind};
use openforge_core::Pattern;
use openforge_core::ctx::Ctx;

#[derive(Debug, Clone)]
pub struct AobSynthesis {
    pub pattern: String,
    pub resolve_kind: AobResolveKind,
    pub instruction_offset: usize,
    pub operand_size: usize,
    pub instructions_included: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AobResolveKind {
    Direct,
    RipRelative,
}

pub fn build_pattern(bytes: &[u8], n_instr: usize, ip: u64) -> Result<AobSynthesis> {
    if bytes.is_empty() {
        return Err(anyhow!("no bytes to decode"));
    }
    let mut decoder = Decoder::with_ip(64, bytes, ip, DecoderOptions::NONE);
    let mut accumulated_len = 0usize;
    let mut wildcards = vec![false; bytes.len()];
    let mut first_disp_abs_offset: Option<usize> = None;
    let mut first_disp_size: usize = 0;
    let mut first_is_rip_rel = false;
    let mut decoded = 0usize;

    for i in 0..n_instr {
        if !decoder.can_decode() {
            break;
        }
        let mut instr = Instruction::default();
        decoder.decode_out(&mut instr);
        if instr.is_invalid() {
            return Err(anyhow!("invalid x86 instruction at byte {accumulated_len}"));
        }
        let len = instr.len();
        if accumulated_len + len > bytes.len() {
            return Err(anyhow!(
                "instruction at byte {accumulated_len} runs past end of captured bytes"
            ));
        }
        let instr_bytes = &bytes[accumulated_len..accumulated_len + len];

        let imm_size = total_immediate_bytes(&instr).min(len);
        let DisplacementInfo {
            offset_in_instr,
            size: disp_size,
            is_rip_relative,
        } = parse_displacement(instr_bytes);

        if disp_size > 0 && offset_in_instr + disp_size <= len {
            let abs = accumulated_len + offset_in_instr;
            wildcards[abs..abs + disp_size].fill(true);
            if i == 0 {
                first_disp_abs_offset = Some(abs);
                first_disp_size = disp_size;
                first_is_rip_rel = is_rip_relative;
            }
        }

        if imm_size > 0 {
            let imm_start = accumulated_len + len - imm_size;
            wildcards[imm_start..accumulated_len + len].fill(true);
        }

        accumulated_len += len;
        decoded += 1;
    }

    if accumulated_len == 0 {
        return Err(anyhow!("no decodable instructions"));
    }

    let mut pattern = String::with_capacity(accumulated_len * 3);
    for i in 0..accumulated_len {
        if i > 0 {
            pattern.push(' ');
        }
        if wildcards[i] {
            pattern.push_str("??");
        } else {
            pattern.push_str(&format!("{:02X}", bytes[i]));
        }
    }

    let (kind, off, size) = if first_is_rip_rel {
        (
            AobResolveKind::RipRelative,
            first_disp_abs_offset.unwrap_or(0),
            first_disp_size,
        )
    } else {
        (AobResolveKind::Direct, 0, 0)
    };

    Ok(AobSynthesis {
        pattern,
        resolve_kind: kind,
        instruction_offset: off,
        operand_size: size,
        instructions_included: decoded,
    })
}

fn total_immediate_bytes(instr: &Instruction) -> usize {
    let mut total = 0;
    for i in 0..instr.op_count() {
        total += match instr.op_kind(i) {
            OpKind::Immediate8
            | OpKind::Immediate8_2nd
            | OpKind::Immediate8to16
            | OpKind::Immediate8to32
            | OpKind::Immediate8to64 => 1,
            OpKind::Immediate16 => 2,
            OpKind::Immediate32 | OpKind::Immediate32to64 => 4,
            OpKind::Immediate64 => 8,
            _ => 0,
        };
    }
    total
}

struct DisplacementInfo {
    /// Byte offset within the single instruction's bytes where the disp begins.
    offset_in_instr: usize,
    /// Displacement size in bytes (0/1/2/4).
    size: usize,
    /// True for `[rip+disp32]` addressing (mod=00, rm=101 in x86-64).
    is_rip_relative: bool,
}

fn parse_displacement(instr_bytes: &[u8]) -> DisplacementInfo {
    let none = DisplacementInfo {
        offset_in_instr: 0,
        size: 0,
        is_rip_relative: false,
    };
    let mut i = 0;
    // Legacy prefixes
    while i < instr_bytes.len() && is_legacy_prefix(instr_bytes[i]) {
        i += 1;
    }
    // REX prefix (single byte 0x40..=0x4F)
    if i < instr_bytes.len() && (instr_bytes[i] & 0xF0) == 0x40 {
        i += 1;
    }
    if i >= instr_bytes.len() {
        return none;
    }
    // Opcode (1-3 bytes; 0x0F escape, 0x38/0x3A 3-byte escapes)
    if instr_bytes[i] == 0x0F {
        i += 1;
        if i < instr_bytes.len() && (instr_bytes[i] == 0x38 || instr_bytes[i] == 0x3A) {
            i += 1;
        }
    }
    i += 1; // primary opcode byte
    if i >= instr_bytes.len() {
        return none;
    }

    let modrm = instr_bytes[i];
    let mod_bits = modrm >> 6;
    let rm_bits = modrm & 0b111;
    i += 1;

    let has_sib = mod_bits != 0b11 && rm_bits == 0b100;
    let sib_base = if has_sib {
        if i >= instr_bytes.len() {
            return none;
        }
        let s = instr_bytes[i];
        i += 1;
        s & 0b111
    } else {
        0
    };

    let (size, is_ip_rel) = match (mod_bits, rm_bits) {
        (0b00, 0b101) if !has_sib => (4, true),
        (0b00, 0b100) if sib_base == 0b101 => (4, false),
        (0b00, _) => (0, false),
        (0b01, _) => (1, false),
        (0b10, _) => (4, false),
        _ => (0, false),
    };

    DisplacementInfo {
        offset_in_instr: i,
        size,
        is_rip_relative: is_ip_rel,
    }
}

fn is_legacy_prefix(b: u8) -> bool {
    matches!(
        b,
        0x66 | 0x67 | 0x26 | 0x2E | 0x36 | 0x3E | 0x64 | 0x65 | 0xF0 | 0xF2 | 0xF3
    )
}

/// Synthesize an AOB and grow the window until it matches uniquely in the
/// target's primary module `.text`. Returns the smallest n-instruction pattern
/// that is unique, up to `max_instructions`.
pub fn synthesize_unique(
    ctx: &dyn Ctx,
    module: &str,
    bytes: &[u8],
    ip: u64,
    start_n: usize,
    max_instructions: usize,
) -> Result<AobSynthesis> {
    let mut last_err: Option<anyhow::Error> = None;
    for n in start_n..=max_instructions {
        match build_pattern(bytes, n, ip) {
            Ok(syn) => {
                let pat = Pattern::parse(&syn.pattern)?;
                let matches = ctx.scan_module_all(module, &pat)?;
                match matches.len() {
                    0 => {
                        last_err = Some(anyhow!(
                            "synthesized AOB ({n} instructions) had zero matches in module {module:?}"
                        ));
                    }
                    1 => return Ok(syn),
                    n_matches => {
                        last_err = Some(anyhow!(
                            "synthesized AOB ({n} instructions) matched {n_matches}x in module {module:?}"
                        ));
                    }
                }
            }
            Err(e) => {
                last_err = Some(e);
                break;
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow!("AOB synthesis failed")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `mov rax, [rip+0x7fe5d2]`  →  REX.W + 8B + ModR/M(05) + disp32
    /// bytes: `48 8B 05 d2 e5 7f 00`
    #[test]
    fn mov_rax_rip_rel() {
        let bytes = [0x48u8, 0x8B, 0x05, 0xD2, 0xE5, 0x7F, 0x00];
        let syn = build_pattern(&bytes, 1, 0x1000_0000).expect("synth");
        assert_eq!(syn.pattern, "48 8B 05 ?? ?? ?? ??");
        assert_eq!(syn.resolve_kind, AobResolveKind::RipRelative);
        assert_eq!(syn.instruction_offset, 3);
        assert_eq!(syn.operand_size, 4);
    }

    /// `mov dword ptr [rax+0xC0], ecx`  →  89 88 C0 00 00 00 (no REX)
    /// Disp32 with mod=10 rm=000.
    #[test]
    fn mov_at_offset_register() {
        let bytes = [0x89u8, 0x88, 0xC0, 0x00, 0x00, 0x00];
        let syn = build_pattern(&bytes, 1, 0x1000_0000).expect("synth");
        assert_eq!(syn.pattern, "89 88 ?? ?? ?? ??");
        assert_eq!(syn.resolve_kind, AobResolveKind::Direct);
        assert_eq!(syn.operand_size, 0);
    }

    /// Three instructions concatenated: load + write + test.
    #[test]
    fn three_instructions() {
        let bytes = [
            0x48, 0x8B, 0x05, 0xD2, 0xE5, 0x7F, 0x00, // mov rax, [rip+...]
            0x89, 0x88, 0xC0, 0x00, 0x00, 0x00, // mov [rax+...], ecx
            0x48, 0x85, 0xC0, // test rax, rax
        ];
        let syn = build_pattern(&bytes, 3, 0x1000_0000).expect("synth");
        assert_eq!(
            syn.pattern,
            "48 8B 05 ?? ?? ?? ?? 89 88 ?? ?? ?? ?? 48 85 C0"
        );
        assert_eq!(syn.resolve_kind, AobResolveKind::RipRelative);
        assert_eq!(syn.instruction_offset, 3);
        assert_eq!(syn.operand_size, 4);
    }

    /// `mov dword ptr [rip+0x12345678], 0xDEADBEEF` — disp32 + imm32
    /// bytes: C7 05 78 56 34 12 EF BE AD DE
    #[test]
    fn mov_rip_rel_with_immediate() {
        let bytes = [0xC7, 0x05, 0x78, 0x56, 0x34, 0x12, 0xEF, 0xBE, 0xAD, 0xDE];
        let syn = build_pattern(&bytes, 1, 0x1000_0000).expect("synth");
        // Opcode + ModR/M kept; disp + imm wildcarded.
        assert_eq!(syn.pattern, "C7 05 ?? ?? ?? ?? ?? ?? ?? ??");
        assert_eq!(syn.resolve_kind, AobResolveKind::RipRelative);
        assert_eq!(syn.instruction_offset, 2);
        assert_eq!(syn.operand_size, 4);
    }
}
