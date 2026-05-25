//! Engine-ABI repr types and the function-pointer signature for
//! `FName::ToString`.
//!
//! UE5 ships exactly one canonical overload of `FName::ToString` on Windows
//! x64: `void FName::ToString(FString& Out) const`. The `this` pointer is
//! passed in `RCX` and the out-FString reference in `RDX` per the MSVC x64
//! calling convention. The function allocates the FString's backing buffer
//! via the engine's own allocator (`FMemory::Realloc`) and writes a UTF-16
//! LE null-terminated string into it. The `num` field on the returned
//! FString counts wide characters **including** the trailing NUL.
//!
//! The repr types in this module mirror the engine's in-memory layout
//! byte-for-byte. They are deliberately tiny and self-contained so they can
//! be unit-tested for ABI invariants (size, field offsets) on any platform —
//! the live call path is exercised in-process against the engine itself.
//!
//! Lifetime note: we cannot free the `FString.data` buffer the engine
//! allocates because we do not have a validated pointer to `FMemory::Free`.
//! Decoded names are cached at the `UeEngine` layer so each unique FName is
//! decoded exactly once; the leak per unique name is bounded by the per-
//! FString cap (we reject `num > 16384`, so the worst case is 32 KiB per
//! unique FName, and there are typically tens of thousands of unique
//! FNames in a shipping UE5 title).

/// In-memory layout of UE5's `FName`: a `(comparison_index, number)` pair of
/// 32-bit unsigned integers. Total size is exactly 8 bytes. The engine's
/// `FName::ToString` reads its argument via `*const FName`, so we pass our
/// repr by reference and the ABI matches.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FNameRepr {
    pub comparison_index: u32,
    pub number: u32,
}

/// In-memory layout of UE5's `FString` (which is just an `TArray<TCHAR>`
/// with `TCHAR = UTF-16`). Three fields: `data` (heap-allocated wide buffer),
/// `num` (count of wide chars including trailing NUL), `max` (allocated
/// capacity). Total size is exactly 16 bytes on Windows x64. We never read
/// `max`; it's present for layout fidelity only.
#[repr(C)]
#[derive(Debug)]
pub(crate) struct FStringRepr {
    pub data: *mut u16,
    pub num: i32,
    pub max: i32,
}

impl FStringRepr {
    /// Construct a zeroed out-FString suitable for passing into
    /// `FName::ToString(&fname, &mut fstring)`. The engine overwrites all
    /// three fields when the function returns.
    #[inline]
    pub(crate) fn zeroed() -> Self {
        Self {
            data: core::ptr::null_mut(),
            num: 0,
            max: 0,
        }
    }
}

/// The canonical `void FName::ToString(FString& Out) const` overload.
///
/// On Windows x64 / MSVC:
/// * `RCX` = `this` (`*const FName`)
/// * `RDX` = `&Out` (`*mut FString`)
/// * Return value: void (the function writes into `*Out`).
///
/// `extern "system"` selects the MSVC x64 ABI on Windows, which is the
/// engine's ABI for non-virtual member functions compiled by MSVC. The
/// engine's vtable-dispatched virtuals would have a different `this`
/// convention (still RCX, but reached via `*(*vtable)[idx]`); `ToString` is
/// **not** virtual so a direct-call signature is correct.
pub(crate) type ToStringFn = unsafe extern "system" fn(*const FNameRepr, *mut FStringRepr);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fname_repr_size_is_8() {
        // The engine ABI requires sizeof(FName) == 8. Any drift here breaks
        // every call site for strategy D / decode_temp_via_to_string.
        assert_eq!(core::mem::size_of::<FNameRepr>(), 8);
    }

    #[test]
    fn fstring_repr_size_is_16() {
        // sizeof(FString) on x64 = 8 (data ptr) + 4 (num) + 4 (max) = 16.
        // The compiler is free to introduce padding only if alignment
        // forces it; FStringRepr has 8-byte alignment from `data`, and 4-
        // byte trailing fields fit without padding. If this ever fires we
        // need to switch to `#[repr(C, packed)]` — but the layout is well-
        // defined for x64 ABI as-is.
        assert_eq!(core::mem::size_of::<FStringRepr>(), 16);
    }

    #[test]
    fn fstring_repr_layout() {
        // `data` at offset 0, `num` at offset 8, `max` at offset 12. This
        // mirrors the engine's TArray<TCHAR> layout exactly. We use
        // `offset_of!` which is stable on the rust-version pinned in this
        // workspace; if it isn't available the test won't compile and we
        // get an immediate signal.
        assert_eq!(core::mem::offset_of!(FStringRepr, data), 0);
        assert_eq!(core::mem::offset_of!(FStringRepr, num), 8);
        assert_eq!(core::mem::offset_of!(FStringRepr, max), 12);
    }

    #[test]
    fn fname_repr_layout() {
        // `comparison_index` at offset 0, `number` at offset 4.
        assert_eq!(core::mem::offset_of!(FNameRepr, comparison_index), 0);
        assert_eq!(core::mem::offset_of!(FNameRepr, number), 4);
    }

    #[test]
    fn fstring_zeroed_is_null_and_zero() {
        let z = FStringRepr::zeroed();
        assert!(z.data.is_null());
        assert_eq!(z.num, 0);
        assert_eq!(z.max, 0);
    }
}
