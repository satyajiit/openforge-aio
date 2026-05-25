use crate::error::{Error, Result};

/// An AOB (array-of-bytes) pattern with optional wildcard bytes.
///
/// Parsed from a string like `"48 8B 05 ?? ?? ?? ?? 89 88"` where each token is
/// either two hex digits or `??` (a wildcard).
#[derive(Debug, Clone)]
pub struct Pattern {
    bytes: Vec<Option<u8>>,
    anchor: usize,
}

impl Pattern {
    pub fn parse(s: &str) -> Result<Self> {
        let mut bytes = Vec::new();
        for tok in s.split_whitespace() {
            if tok == "??" || tok == "?" {
                bytes.push(None);
            } else if tok.len() == 2 && tok.chars().all(|c| c.is_ascii_hexdigit()) {
                let b = u8::from_str_radix(tok, 16)
                    .map_err(|_| Error::BadPattern(format!("bad hex token {tok:?}")))?;
                bytes.push(Some(b));
            } else {
                return Err(Error::BadPattern(format!(
                    "token {tok:?} is not two hex digits or '??'"
                )));
            }
        }
        let anchor = bytes
            .iter()
            .position(|b| b.is_some())
            .ok_or(Error::EmptyPattern)?;
        Ok(Self { bytes, anchor })
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Serialise the pattern into a `(bytes, mask)` pair compatible with the
    /// IPC protocol's `PatternWire` shape. `mask[i] == 0xFF` marks a literal
    /// byte; `mask[i] == 0x00` marks a wildcard. Used by the
    /// single-channel `scan_module` / `scan_module_all` ops to ship the
    /// pattern to the DLL without re-parsing the source string.
    pub fn wire(&self) -> (Vec<u8>, Vec<u8>) {
        let mut bytes = Vec::with_capacity(self.bytes.len());
        let mut mask = Vec::with_capacity(self.bytes.len());
        for b in &self.bytes {
            match b {
                Some(v) => {
                    bytes.push(*v);
                    mask.push(0xFF);
                }
                None => {
                    bytes.push(0);
                    mask.push(0x00);
                }
            }
        }
        (bytes, mask)
    }

    /// Reconstruct a `Pattern` from the wire `(bytes, mask)` shape. The
    /// inverse of [`Pattern::wire`].
    pub fn from_wire(bytes: &[u8], mask: &[u8]) -> Result<Self> {
        if bytes.len() != mask.len() {
            return Err(Error::BadPattern(format!(
                "pattern wire length mismatch: bytes={} mask={}",
                bytes.len(),
                mask.len()
            )));
        }
        let mut out: Vec<Option<u8>> = Vec::with_capacity(bytes.len());
        for (b, m) in bytes.iter().zip(mask.iter()) {
            match *m {
                0xFF => out.push(Some(*b)),
                0x00 => out.push(None),
                other => {
                    return Err(Error::BadPattern(format!(
                        "pattern mask byte must be 0x00 or 0xFF; got 0x{other:02X}"
                    )));
                }
            }
        }
        let anchor = out
            .iter()
            .position(|b| b.is_some())
            .ok_or(Error::EmptyPattern)?;
        Ok(Self { bytes: out, anchor })
    }

    /// Scan a byte slice for the first match. Returns the offset within `haystack`.
    pub fn scan(&self, haystack: &[u8]) -> Option<usize> {
        let anchor_byte = self.bytes[self.anchor].expect("anchor exists by construction");
        let plen = self.bytes.len();
        if haystack.len() < plen {
            return None;
        }
        let end = haystack.len() - plen + 1;
        for i in 0..end {
            if haystack[i + self.anchor] != anchor_byte {
                continue;
            }
            if self.matches_at(haystack, i) {
                return Some(i);
            }
        }
        None
    }

    /// Scan for every match in the haystack. Useful for `verify` / debugging
    /// signature uniqueness.
    pub fn scan_all(&self, haystack: &[u8]) -> Vec<usize> {
        let anchor_byte = self.bytes[self.anchor].expect("anchor exists by construction");
        let plen = self.bytes.len();
        if haystack.len() < plen {
            return Vec::new();
        }
        let end = haystack.len() - plen + 1;
        let mut out = Vec::new();
        for i in 0..end {
            if haystack[i + self.anchor] != anchor_byte {
                continue;
            }
            if self.matches_at(haystack, i) {
                out.push(i);
            }
        }
        out
    }

    #[inline]
    fn matches_at(&self, haystack: &[u8], offset: usize) -> bool {
        for (i, mb) in self.bytes.iter().enumerate() {
            if let Some(b) = mb
                && haystack[offset + i] != *b
            {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_matches() {
        let p = Pattern::parse("48 8B 05 ?? ?? ?? ?? 89").unwrap();
        let hay = b"\x00\x48\x8B\x05\xde\xad\xbe\xef\x89\x00";
        assert_eq!(p.scan(hay), Some(1));
    }

    #[test]
    fn rejects_all_wildcards() {
        assert!(matches!(Pattern::parse("?? ??"), Err(Error::EmptyPattern)));
    }

    #[test]
    fn rejects_bad_hex() {
        assert!(matches!(Pattern::parse("ZZ"), Err(Error::BadPattern(_))));
    }

    #[test]
    fn scans_all_matches() {
        let p = Pattern::parse("AA BB").unwrap();
        let hay = b"\xAA\xBB\x00\xAA\xBB\x01\xAA\xBB";
        assert_eq!(p.scan_all(hay), vec![0, 3, 6]);
    }

    #[test]
    fn wire_roundtrip() {
        let src = "48 8B 05 ?? ?? ?? ?? 89";
        let p = Pattern::parse(src).unwrap();
        let (bytes, mask) = p.wire();
        assert_eq!(bytes.len(), 8);
        assert_eq!(mask, vec![0xFF, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00, 0xFF]);
        let p2 = Pattern::from_wire(&bytes, &mask).unwrap();
        let hay = b"\x00\x48\x8B\x05\xde\xad\xbe\xef\x89\x00";
        assert_eq!(p2.scan(hay), Some(1));
    }

    #[test]
    fn from_wire_rejects_mismatched_lengths() {
        match Pattern::from_wire(&[0; 3], &[0xFF; 2]) {
            Err(Error::BadPattern(_)) => {}
            other => panic!("expected BadPattern, got {other:?}"),
        }
    }

    #[test]
    fn from_wire_rejects_unknown_mask_byte() {
        match Pattern::from_wire(&[0x48], &[0x01]) {
            Err(Error::BadPattern(s)) => assert!(s.contains("0x01")),
            other => panic!("expected BadPattern, got {other:?}"),
        }
    }

    #[test]
    fn from_wire_rejects_all_wildcard() {
        match Pattern::from_wire(&[0, 0], &[0x00, 0x00]) {
            Err(Error::EmptyPattern) => {}
            other => panic!("expected EmptyPattern, got {other:?}"),
        }
    }
}
