//! Locate GUObjectArray, FNamePool, and probe the UObject/FProperty offsets at runtime.

use anyhow::{Result, bail};
use iced_x86::{Decoder, DecoderOptions, Instruction};
use openforge_core::{Ctx, Pattern, Target};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tracing::{debug, info, warn};

use crate::ue5::{PropInfo, UFunctionInfo, UeOffsets};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UeCache {
    pub guobject_array: usize,
    pub fname_pool: usize,
    pub fname_pool_chunks_offset: usize,
    pub fuobject_item_stride: usize,
    pub offsets: UeOffsets,
    pub fproperty_offsets_validated: bool,
}

fn get_cache_path(pe_hash: &str) -> Option<PathBuf> {
    let mut path = dirs::data_local_dir()?;
    path.push("openforge");
    path.push("ue5-cache");
    let _ = std::fs::create_dir_all(&path);
    path.push(format!("{}.json", pe_hash));
    Some(path)
}

fn get_pe_hash(ctx: &Target) -> Result<String> {
    let base = ctx.main().base;
    let mut e_lfanew = [0u8; 4];
    ctx.read_bytes(base + 0x3C, &mut e_lfanew)?;
    let pe_off = u32::from_le_bytes(e_lfanew) as usize;
    let pe_addr = base + pe_off;

    let mut time_date_stamp = [0u8; 4];
    ctx.read_bytes(pe_addr + 8, &mut time_date_stamp)?;
    let timestamp = u32::from_le_bytes(time_date_stamp);

    let mut size_of_image_bytes = [0u8; 4];
    ctx.read_bytes(pe_addr + 0x50, &mut size_of_image_bytes)?;
    let size_of_image = u32::from_le_bytes(size_of_image_bytes);

    Ok(format!("{:08X}_{:08X}", timestamp, size_of_image))
}

fn load_cache(pe_hash: &str) -> Option<UeCache> {
    let path = get_cache_path(pe_hash)?;
    if !path.exists() {
        return None;
    }
    let data = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

fn save_cache(pe_hash: &str, cache: &UeCache) -> Result<()> {
    if let Some(path) = get_cache_path(pe_hash) {
        let data = serde_json::to_string_pretty(cache)?;
        std::fs::write(path, data)?;
    }
    Ok(())
}

pub struct UeEngine {
    pub process: Arc<Target>,
    pub guobject_array: usize,
    pub fname_pool: usize,
    /// Offset from `fname_pool` to the 8-byte pointer that holds the chunks
    /// array base. Different UE5 minor versions place it at +0x08, +0x10,
    /// +0x18, or +0x20 depending on the FNamePoolBase header layout.
    pub fname_pool_chunks_offset: usize,
    pub offsets: UeOffsets,
    pub fuobject_item_stride: usize,
    /// True only when both UObject and FProperty offsets were positively
    /// validated. When false, signature emission paths refuse to write TOMLs
    /// rather than emit silently-bad data.
    pub fproperty_offsets_validated: bool,
    pub name_cache: Mutex<HashMap<u32, String>>,
    pub prop_cache: Mutex<HashMap<usize, Arc<Vec<PropInfo>>>>,
    pub func_cache: Mutex<HashMap<usize, Arc<Vec<UFunctionInfo>>>>,
}

impl UeEngine {
    pub fn attach(process: Arc<Target>) -> Result<Self> {
        let process_ref: &Target = &process;

        // Try cache first
        let pe_hash = get_pe_hash(process_ref).ok();
        if let Some(ref hash) = pe_hash
            && let Some(cached) = load_cache(hash)
        {
            info!("Found cached UE5 reflection config for hash {}", hash);
            // Perform cheap validation
            let mut header = [0u8; 32];
            if process_ref
                .read_bytes(cached.guobject_array, &mut header)
                .is_ok()
            {
                let max_elements = u32::from_le_bytes(header[16..20].try_into().unwrap());
                let num_elements = u32::from_le_bytes(header[20..24].try_into().unwrap());
                let num_chunks = u32::from_le_bytes(header[28..32].try_into().unwrap());
                if num_elements > 0
                    && max_elements >= num_elements
                    && num_chunks > 0
                    && num_chunks < 1000
                {
                    // MaxElements/NumElements look sane, now structurally validate FNamePool
                    if Self::validate_fname_pool_structural(
                        process_ref,
                        cached.fname_pool,
                        Some(cached.guobject_array),
                    )
                    .map(|(off, _)| off == cached.fname_pool_chunks_offset)
                    .unwrap_or(false)
                    {
                        info!("Cached UE5 configuration structurally validated successfully!");
                        return Ok(Self {
                            process,
                            guobject_array: cached.guobject_array,
                            fname_pool: cached.fname_pool,
                            fname_pool_chunks_offset: cached.fname_pool_chunks_offset,
                            offsets: cached.offsets,
                            fuobject_item_stride: cached.fuobject_item_stride,
                            fproperty_offsets_validated: cached.fproperty_offsets_validated,
                            name_cache: Mutex::new(HashMap::new()),
                            prop_cache: Mutex::new(HashMap::new()),
                            func_cache: Mutex::new(HashMap::new()),
                        });
                    }
                }
            }
            warn!(
                "Cached configuration failed structural validation. Running cold locator path..."
            );
        }

        // 1. Locate GUObjectArray
        let guobject_array = Self::find_guobject_array(process_ref)?;
        info!("GUObjectArray located at 0x{:X}", guobject_array);

        // 2. Locate FNamePool — pass guobject_array so the locator can use it
        // as a structural oracle (decode the first UObject's name against
        // each FNamePool candidate; whichever yields a valid identifier wins).
        let (fname_pool, fname_pool_chunks_offset) =
            Self::find_fname_pool(process_ref, guobject_array)?;
        info!(
            "FNamePool located at 0x{:X} (chunks_offset = +0x{:X})",
            fname_pool, fname_pool_chunks_offset
        );

        // 3. Determine FUObjectItem stride and base offsets via runtime probing.
        //    probe_layout may override chunks_offset if it finds a better one
        //    using UObject FName decode as a stronger oracle than the upstream
        //    substring-based validator.
        let (stride, mut offsets, uobj_layout_ok, fname_pool_chunks_offset) = Self::probe_layout(
            process_ref,
            guobject_array,
            fname_pool,
            fname_pool_chunks_offset,
        )?;
        info!("Detected FUObjectItem stride: {} bytes", stride);
        info!(
            "UObject layout probed (ok={}, chunks_off=+0x{:X}). Offsets: {:?}",
            uobj_layout_ok, fname_pool_chunks_offset, offsets
        );

        // 4. Probe FProperty offset/element_size against a known UStruct.
        let fprop_ok = Self::probe_fproperty_offsets(
            process_ref,
            guobject_array,
            fname_pool,
            fname_pool_chunks_offset,
            stride,
            &mut offsets,
        )
        .unwrap_or(false);

        let validated = uobj_layout_ok && fprop_ok;
        if !validated {
            warn!(
                "Layout validation incomplete (uobject={}, fproperty={}). Signature emission will be refused.",
                uobj_layout_ok, fprop_ok
            );
        } else {
            info!("Layout locked & validated: {:?}", offsets);
        }

        // Save to cache if hash is available
        if let Some(ref hash) = pe_hash {
            let cache = UeCache {
                guobject_array,
                fname_pool,
                fname_pool_chunks_offset,
                fuobject_item_stride: stride,
                offsets,
                fproperty_offsets_validated: validated,
            };
            if let Err(e) = save_cache(hash, &cache) {
                warn!("Failed to save UE5 reflection config cache: {:?}", e);
            } else {
                info!("Saved UE5 reflection config cache for hash {}", hash);
            }
        }

        Ok(Self {
            process,
            guobject_array,
            fname_pool,
            fname_pool_chunks_offset,
            offsets,
            fuobject_item_stride: stride,
            fproperty_offsets_validated: validated,
            name_cache: Mutex::new(HashMap::new()),
            prop_cache: Mutex::new(HashMap::new()),
            func_cache: Mutex::new(HashMap::new()),
        })
    }

    fn find_guobject_array(ctx: &Target) -> Result<usize> {
        // Known AOB patterns for GUObjectArray in UE5
        let patterns = [
            "48 8B 05 ?? ?? ?? ?? 48 8B 0C C8 48 8D 04 D1 EB 02", // GUObjectArray.Objects[index]
            "48 8D 0D ?? ?? ?? ?? E8 ?? ?? ?? ?? 48 8B 5C 24",    // lea rcx, [GUObjectArray]
            "48 8D 0D ?? ?? ?? ?? E8 ?? ?? ?? ?? 48 8B D8 48 85 C0",
        ];

        for (i, pat_str) in patterns.iter().enumerate() {
            let pat = Pattern::parse(pat_str)?;
            if let Ok(Some(addr)) = ctx.scan_module("", &pat) {
                // Resolve RIP-relative address
                let mut disp = [0u8; 4];
                if ctx.read_bytes(addr + 3, &mut disp).is_ok() {
                    let offset = i32::from_le_bytes(disp) as i64;
                    let resolved = (addr as i64 + 7 + offset) as usize;
                    debug!(
                        "Pattern {} matched at 0x{:X} -> resolved 0x{:X}",
                        i, addr, resolved
                    );

                    // Validate header: MaxElements and NumElements
                    let mut header = [0u8; 32];
                    if ctx.read_bytes(resolved, &mut header).is_ok() {
                        let max_elements = u32::from_le_bytes(header[16..20].try_into().unwrap());
                        let num_elements = u32::from_le_bytes(header[20..24].try_into().unwrap());
                        let num_chunks = u32::from_le_bytes(header[28..32].try_into().unwrap());
                        if num_elements > 0
                            && max_elements >= num_elements
                            && num_chunks > 0
                            && num_chunks < 1000
                        {
                            return Ok(resolved);
                        }
                    }
                }
            }
        }

        // Advanced Fallback: Search for String xref to "Maximum number of UObjects"
        info!("Standard GUObjectArray patterns failed. Attempting advanced fallbacks...");
        let main_module = ctx.main();
        let mut mod_bytes = vec![0u8; main_module.size];
        ctx.read_bytes(main_module.base, &mut mod_bytes)?;

        // Fallback A — structural `.data` fingerprint of FChunkedFixedUObjectArray.
        // Independent of code patterns AND log strings, so it survives game
        // updates and different store binaries (the AOB patterns above are
        // codegen-specific; the string xref below depends on a log string the
        // Shipping build may strip or store as UTF-16).
        //
        //   FChunkedFixedUObjectArray (the inner array `GUObjectArray.ObjObjects`):
        //     +0x00 Objects**   (heap pointer to the chunk-pointer array)
        //     +0x10 MaxElements (u32)
        //     +0x14 NumElements (u32)
        //     +0x18 MaxChunks   (u32)
        //     +0x1C NumChunks   (u32) == ceil(NumElements / 65536)
        {
            const CHUNK_CAP: u64 = 65536;
            let base = main_module.base;
            let mod_end = base + main_module.size;
            let n = mod_bytes.len();
            let mut off = 0usize;
            while off + 32 <= n {
                let num_elements =
                    u32::from_le_bytes(mod_bytes[off + 0x14..off + 0x18].try_into().unwrap());
                // Cheap first gate: any UE5 process has thousands of live
                // UObjects. Skips ~all offsets before the costlier checks.
                if (1000..=50_000_000).contains(&num_elements) {
                    let objects_ptr =
                        u64::from_le_bytes(mod_bytes[off..off + 8].try_into().unwrap());
                    let max_elements =
                        u32::from_le_bytes(mod_bytes[off + 0x10..off + 0x14].try_into().unwrap());
                    let max_chunks =
                        u32::from_le_bytes(mod_bytes[off + 0x18..off + 0x1C].try_into().unwrap());
                    let num_chunks =
                        u32::from_le_bytes(mod_bytes[off + 0x1C..off + 0x20].try_into().unwrap());
                    let expected_chunks = (num_elements as u64).div_ceil(CHUNK_CAP);
                    let header_ok = max_elements >= num_elements
                        && max_elements <= 60_000_000
                        && (num_chunks as u64) >= expected_chunks
                        && (num_chunks as u64) <= expected_chunks + 2
                        && max_chunks >= num_chunks
                        && max_chunks <= 4000
                        && objects_ptr != 0
                        && objects_ptr.is_multiple_of(8)
                        && objects_ptr > 0x10000;
                    if header_ok {
                        // Confirm via deref: Objects[0][0].Object must be a live
                        // UObject whose vtable lands inside the module image.
                        let objects_ptr = objects_ptr as usize;
                        let mut buf = [0u8; 8];
                        if ctx.read_bytes(objects_ptr, &mut buf).is_ok() {
                            let chunk0 = u64::from_le_bytes(buf) as usize;
                            if chunk0 != 0
                                && chunk0.is_multiple_of(8)
                                && chunk0 > 0x10000
                                && ctx.read_bytes(chunk0, &mut buf).is_ok()
                            {
                                let first_obj = u64::from_le_bytes(buf) as usize;
                                if first_obj != 0
                                    && first_obj.is_multiple_of(8)
                                    && first_obj > 0x10000
                                    && ctx.read_bytes(first_obj, &mut buf).is_ok()
                                {
                                    let vtable = u64::from_le_bytes(buf) as usize;
                                    if vtable >= base && vtable < mod_end {
                                        let resolved = base + off;
                                        info!(
                                            "Located GUObjectArray via structural .data fingerprint at 0x{:X} (NumElements={}, NumChunks={})",
                                            resolved, num_elements, num_chunks
                                        );
                                        return Ok(resolved);
                                    }
                                }
                            }
                        }
                    }
                }
                off += 8;
            }
            info!(
                "Structural .data fingerprint found no GUObjectArray; trying string xref fallback..."
            );
        }

        let needle = b"Maximum number of UObjects";
        let mut string_addr = None;
        for i in 0..(mod_bytes.len() - needle.len() + 1) {
            if &mod_bytes[i..i + needle.len()] == needle {
                string_addr = Some(main_module.base + i);
                break;
            }
        }

        if let Some(str_addr) = string_addr {
            debug!(
                "Found 'Maximum number of UObjects' string at 0x{:X}",
                str_addr
            );

            // Search .text section for RIP-relative references to str_addr.
            let text_start = main_module.text_base();
            let text_end = text_start + main_module.text_size;
            let mut candidates = Vec::new();

            for inst_addr in text_start..(text_end - 7) {
                let offset_in_mod = inst_addr - main_module.base;
                let opcode = &mod_bytes[offset_in_mod..offset_in_mod + 3];

                let is_lea = (opcode[0] == 0x48
                    && opcode[1] == 0x8D
                    && (opcode[2] == 0x0D || opcode[2] == 0x15))
                    || (opcode[0] == 0x4C
                        && opcode[1] == 0x8D
                        && (opcode[2] == 0x05 || opcode[2] == 0x0D));

                if is_lea {
                    let disp = i32::from_le_bytes(
                        mod_bytes[offset_in_mod + 3..offset_in_mod + 7]
                            .try_into()
                            .unwrap(),
                    ) as i64;
                    let target = (inst_addr as i64 + 7 + disp) as usize;
                    if target == str_addr {
                        debug!(
                            "Found RIP-relative LEA reference to string at 0x{:X}",
                            inst_addr
                        );
                        candidates.push(inst_addr);
                    }
                }
            }

            // Inspect the surrounding function window (e.g. +/- 128 bytes)
            for inst_addr in candidates {
                let window_start = inst_addr.saturating_sub(128).max(text_start);
                let window_end = (inst_addr + 128).min(text_end);

                for test_addr in window_start..(window_end - 7) {
                    let offset_in_mod = test_addr - main_module.base;
                    let opcode = &mod_bytes[offset_in_mod..offset_in_mod + 3];

                    let is_rip_access = opcode[0] == 0x48
                        && (opcode[1] == 0x8B || opcode[1] == 0x8D)
                        && (opcode[2] == 0x05 || opcode[2] == 0x0D);

                    if is_rip_access {
                        let disp = i32::from_le_bytes(
                            mod_bytes[offset_in_mod + 3..offset_in_mod + 7]
                                .try_into()
                                .unwrap(),
                        ) as i64;
                        let resolved = (test_addr as i64 + 7 + disp) as usize;

                        // Check if resolved address satisfies GUObjectArray structure
                        let mut header = [0u8; 32];
                        if ctx.read_bytes(resolved, &mut header).is_ok() {
                            let max_elements =
                                u32::from_le_bytes(header[16..20].try_into().unwrap());
                            let num_elements =
                                u32::from_le_bytes(header[20..24].try_into().unwrap());
                            let num_chunks = u32::from_le_bytes(header[28..32].try_into().unwrap());

                            if num_elements > 0
                                && max_elements >= num_elements
                                && num_chunks > 0
                                && num_chunks < 1000
                            {
                                let objects_ptr =
                                    u64::from_le_bytes(header[0..8].try_into().unwrap()) as usize;
                                if objects_ptr != 0
                                    && objects_ptr.is_multiple_of(8)
                                    && objects_ptr > 0x10000
                                {
                                    let mut first_chunk_buf = [0u8; 8];
                                    if ctx.read_bytes(objects_ptr, &mut first_chunk_buf).is_ok() {
                                        let first_chunk =
                                            u64::from_le_bytes(first_chunk_buf) as usize;
                                        if first_chunk != 0 && first_chunk.is_multiple_of(8) {
                                            info!(
                                                "Successfully located GUObjectArray via string xref fallback at 0x{:X}",
                                                resolved
                                            );
                                            return Ok(resolved);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        bail!(
            "Could not locate GUObjectArray via signatures or advanced string xref scanning fallback. Ensure process is fully loaded."
        )
    }

    /// Canonical short names that always appear early in any UE5 FNamePool.
    /// We accept the first decoded entry if its bytes match ANY of these.
    /// Sorted by likelihood / specificity.
    #[allow(dead_code)]
    const FNAME_CANONICAL_NAMES: &'static [&'static str] = &[
        "None",
        "Object",
        "Package",
        "Class",
        "Function",
        "Field",
        "ByteProperty",
        "IntProperty",
        "BoolProperty",
        "FloatProperty",
        "ObjectProperty",
        "ArrayProperty",
        "StrProperty",
        "NameProperty",
        "StructProperty",
        "Default__Object",
        "CoreUObject",
    ];

    /// Candidate offsets to the `chunks` 8-byte pointer inside the FNamePoolBase
    /// header. Layout drifted across UE5 minor versions depending on whether
    /// the lock + counters are inlined or in a sub-struct. Some shipping
    /// configurations also push the chunks array further out for cache-line
    /// alignment, and UE 5.3+ pools with FRWLock can have larger headers.
    const FNAME_POOL_CHUNKS_OFFSETS: &'static [usize] = &[
        0x08, 0x10, 0x18, 0x20, 0x28, 0x30, 0x40, 0x48, 0x50, 0x58, 0x60, 0x80, 0xC0, 0x100, 0x140,
        0x180, 0x1C0, 0x200,
    ];

    /// Stage A: ensure `[chunks_addr]` looks like a dense array of heap chunk
    /// pointers, not a slab of random code/data pointers.
    ///
    /// * At least 8 of the first 16 slots are non-zero, 8-aligned, > 0x10000.
    /// * NONE of the non-zero slot values fall inside the main module image
    ///   (real chunk pointers point into NamePool heap arenas, never into the
    ///   game .text/.rdata).
    /// * At least 75% of the non-zero slots share the same high-32-bit prefix
    ///   (chunks all live in the same allocation arena).
    fn stage_a_validate_chunks_array(ctx: &Target, chunks_addr: usize) -> bool {
        let main = ctx.main();
        let mod_lo = main.base;
        let mod_hi = main.base + main.size;

        let mut slots = [0u8; 16 * 8];
        if ctx.read_bytes(chunks_addr, &mut slots).is_err() {
            return false;
        }
        let mut values: Vec<u64> = Vec::with_capacity(16);
        for chunk in slots.chunks_exact(8) {
            values.push(u64::from_le_bytes(chunk.try_into().unwrap()));
        }

        let mut nonzero: Vec<u64> = Vec::new();
        for &v in &values {
            if v == 0 {
                continue;
            }
            if !(v as usize).is_multiple_of(8) || (v as usize) <= 0x10000 {
                return false;
            }
            // Reject high-half values (kernel space / sentinels).
            if v >= 0x0000_8000_0000_0000 {
                return false;
            }
            // No chunk pointer should land inside the main module image.
            if (v as usize) >= mod_lo && (v as usize) < mod_hi {
                return false;
            }
            nonzero.push(v);
        }

        if nonzero.len() < 8 {
            return false;
        }

        // Cluster check: >=75% share the same high-32-bit prefix (same arena).
        let mut prefixes: Vec<u32> = nonzero.iter().map(|&v| (v >> 32) as u32).collect();
        prefixes.sort_unstable();
        // Find the mode.
        let mut best_count = 0usize;
        let mut cur_count = 1usize;
        let mut cur_prefix = prefixes[0];
        for &p in &prefixes[1..] {
            if p == cur_prefix {
                cur_count += 1;
            } else {
                if cur_count > best_count {
                    best_count = cur_count;
                }
                cur_prefix = p;
                cur_count = 1;
            }
        }
        if cur_count > best_count {
            best_count = cur_count;
        }
        let needed = (nonzero.len() * 3).div_ceil(4); // >= 75%
        best_count >= needed
    }

    /// Stage B: parse `chunk_base` as a sequence of FNameEntry records and
    /// require the first 8 entries to be identifier-shaped.
    ///
    /// Entry layout (UE5 narrow): u16 header, then `len` ASCII bytes.
    /// Header encodes `len = (header >> 6) & 0x3FF` and `is_wide = header & 1`.
    /// Entries are 2-byte-aligned in the chunk.
    ///
    /// Returns true iff at least 8 valid entries were walked successfully.
    fn stage_b_validate_first_chunk(ctx: &Target, chunk_base: usize) -> bool {
        let mut buf = vec![0u8; 1024];
        if ctx.read_bytes(chunk_base, &mut buf).is_err() {
            return false;
        }

        let is_ident_char =
            |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'.' || c == b'/';
        let is_ident_start = |c: u8| c.is_ascii_alphabetic() || c == b'_';

        let mut offset: usize = 0;
        let mut entries_ok: usize = 0;
        let max_entries = 16;

        for _ in 0..max_entries {
            if offset + 2 > buf.len() {
                break;
            }
            let header = u16::from_le_bytes(buf[offset..offset + 2].try_into().unwrap());
            let len = ((header >> 6) & 0x3FF) as usize;
            let is_wide = (header & 1) != 0;

            // First entry may be "None" (len=4). After that, accept any
            // identifier-shape entry with reasonable length.
            if is_wide {
                return false;
            }
            if !(2..=64).contains(&len) {
                return false;
            }
            if offset + 2 + len > buf.len() {
                break;
            }

            let bytes = &buf[offset + 2..offset + 2 + len];
            // All bytes printable ASCII.
            for &b in bytes {
                if !(0x20..=0x7E).contains(&b) {
                    return false;
                }
            }
            // Identifier-shape: start with letter/underscore, rest in
            // alnum/underscore/dot/slash.
            if !is_ident_start(bytes[0]) {
                return false;
            }
            if !bytes[1..].iter().all(|&b| is_ident_char(b)) {
                return false;
            }

            entries_ok += 1;
            // Advance to next entry, 2-byte aligned.
            let advance = 2 + len;
            let advance = advance.div_ceil(2) * 2;
            offset += advance;

            if entries_ok >= 8 {
                return true;
            }
        }

        entries_ok >= 8
    }

    /// Stage C: walk the first N live UObjects and decode their `NamePrivate`
    /// FNames against the candidate pool. Returns the count of identifier-shape
    /// decodes out of `max_samples`.
    fn stage_c_score_uobject_names(
        ctx: &Target,
        guobject_array: usize,
        pool_addr: usize,
        chunks_offset: usize,
        stride: usize,
        max_samples: usize,
    ) -> (usize, usize, Vec<String>) {
        let mut header = [0u8; 32];
        if ctx.read_bytes(guobject_array, &mut header).is_err() {
            return (0, 0, Vec::new());
        }
        let objects_ptr = u64::from_le_bytes(header[0..8].try_into().unwrap()) as usize;
        let num_elements = u32::from_le_bytes(header[20..24].try_into().unwrap()) as usize;
        let num_chunks = u32::from_le_bytes(header[28..32].try_into().unwrap()) as usize;
        if objects_ptr == 0 || num_elements == 0 || num_chunks == 0 {
            return (0, 0, Vec::new());
        }

        const CHUNK_SIZE: usize = 65536;
        const NAME_OFF: usize = 0x18;

        let canonical_prefixes = ["/Script/", "/Engine/", "/Game/"];
        let canonical_names: &[&str] = &[
            "None",
            "Object",
            "Class",
            "Package",
            "Function",
            "ScriptStruct",
            "BlueprintGeneratedClass",
            "Pawn",
            "Actor",
            "World",
            "Default__Object",
            "CoreUObject",
            "Engine",
            "Transient",
        ];
        let is_identifier = |s: &str| -> bool {
            let len = s.len();
            if !(2..=64).contains(&len) {
                return false;
            }
            if canonical_names.contains(&s) {
                return true;
            }
            if canonical_prefixes.iter().any(|p| s.starts_with(p)) {
                return true;
            }
            let mut chars = s.chars();
            match chars.next() {
                Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
                _ => return false,
            }
            chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        };

        let mut tried = 0usize;
        let mut pass = 0usize;
        let mut samples: Vec<String> = Vec::new();

        for index in 0..num_elements {
            if tried >= max_samples {
                break;
            }
            let chunk_idx = index / CHUNK_SIZE;
            let within_chunk = index % CHUNK_SIZE;
            if chunk_idx >= num_chunks {
                break;
            }
            let chunk_addr = if chunk_idx == 0 {
                let mut b = [0u8; 8];
                if ctx.read_bytes(objects_ptr, &mut b).is_err() {
                    break;
                }
                u64::from_le_bytes(b) as usize
            } else {
                let mut b = [0u8; 8];
                if ctx.read_bytes(objects_ptr + chunk_idx * 8, &mut b).is_err() {
                    break;
                }
                u64::from_le_bytes(b) as usize
            };
            if chunk_addr == 0 {
                continue;
            }

            let item_addr = chunk_addr + within_chunk * stride;
            let mut obj_ptr_buf = [0u8; 8];
            if ctx.read_bytes(item_addr, &mut obj_ptr_buf).is_err() {
                continue;
            }
            let obj_ptr = u64::from_le_bytes(obj_ptr_buf) as usize;
            if obj_ptr == 0 || !obj_ptr.is_multiple_of(8) || obj_ptr <= 0x10000 {
                continue;
            }
            let mut nbuf = [0u8; 8];
            if ctx.read_bytes(obj_ptr + NAME_OFF, &mut nbuf).is_err() {
                continue;
            }
            let ci = u32::from_le_bytes(nbuf[0..4].try_into().unwrap());
            let num = u32::from_le_bytes(nbuf[4..8].try_into().unwrap());
            if ci == 0 || (ci >> 16) >= 1024 || num > 0xFFFF {
                continue;
            }

            tried += 1;
            if let Ok(name) = Self::decode_temp(ctx, pool_addr, chunks_offset, ci, num)
                && is_identifier(&name)
            {
                pass += 1;
                if samples.len() < 6 {
                    samples.push(name);
                }
            }
        }

        (pass, tried, samples)
    }

    /// Detect the FUObjectItem stride for a fresh process. Used by
    /// `validate_fname_pool_structural` Stage C — we can't depend on the
    /// engine's pre-detected stride because validation runs before stride
    /// detection.
    fn detect_stride(ctx: &Target, guobject_array: usize) -> usize {
        let mut header = [0u8; 16];
        if ctx.read_bytes(guobject_array, &mut header).is_err() {
            return 24;
        }
        let objects_ptr = u64::from_le_bytes(header[0..8].try_into().unwrap()) as usize;
        if objects_ptr == 0 {
            return 24;
        }
        let mut buf = [0u8; 8];
        if ctx.read_bytes(objects_ptr, &mut buf).is_err() {
            return 24;
        }
        let first_chunk = u64::from_le_bytes(buf) as usize;
        if first_chunk == 0 {
            return 24;
        }

        for stride in [24usize, 16, 20] {
            let mut valid_count = 0;
            for i in 0..10 {
                let item_addr = first_chunk + i * stride;
                let mut obj_ptr_buf = [0u8; 8];
                if ctx.read_bytes(item_addr, &mut obj_ptr_buf).is_ok() {
                    let obj_ptr = u64::from_le_bytes(obj_ptr_buf) as usize;
                    if obj_ptr != 0 && obj_ptr.is_multiple_of(8) && obj_ptr > 0x10000 {
                        let mut test = [0u8; 8];
                        if ctx.read_bytes(obj_ptr, &mut test).is_ok() {
                            valid_count += 1;
                        }
                    }
                }
            }
            if valid_count >= 8 {
                return stride;
            }
        }
        24
    }

    /// Strong structural validator. For each candidate `chunks_offset`, runs:
    ///   Stage A: dense chunk pointer array.
    ///   Stage B: first chunk parses as FNameEntries (>=8 identifier-shape).
    ///   Stage C: >=70% of first 50 UObject names decode to identifiers.
    ///
    /// Returns `Some((chunks_offset, stage_c_score))` for the BEST chunks_offset
    /// that passes A+B+C, or `None` if no offset passes.
    fn validate_fname_pool_structural(
        ctx: &Target,
        pool_addr: usize,
        guobject_array: Option<usize>,
    ) -> Option<(usize, usize)> {
        let mut best: Option<(usize, usize)> = None;

        for &chunks_offset in Self::FNAME_POOL_CHUNKS_OFFSETS {
            let mut chunks_header_buf = [0u8; 8];
            if ctx
                .read_bytes(pool_addr + chunks_offset, &mut chunks_header_buf)
                .is_err()
            {
                continue;
            }
            let chunks_addr = u64::from_le_bytes(chunks_header_buf) as usize;
            if chunks_addr == 0 || !chunks_addr.is_multiple_of(8) || chunks_addr <= 0x10000 {
                continue;
            }

            // Stage A
            if !Self::stage_a_validate_chunks_array(ctx, chunks_addr) {
                continue;
            }

            // Read first chunk pointer.
            let mut buf = [0u8; 8];
            if ctx.read_bytes(chunks_addr, &mut buf).is_err() {
                continue;
            }
            let first_chunk = u64::from_le_bytes(buf) as usize;
            if first_chunk == 0 || !first_chunk.is_multiple_of(8) {
                continue;
            }

            // Stage B
            if !Self::stage_b_validate_first_chunk(ctx, first_chunk) {
                continue;
            }

            // Stage C (only if we have GUObjectArray)
            let Some(guobject_array) = guobject_array else {
                // No oracle — A+B pass, accept with score 0.
                debug!(
                    "validate_fname_pool_structural: pool=0x{:X} chunks_off=+0x{:X} \
                     passed A+B (no Stage C oracle)",
                    pool_addr, chunks_offset
                );
                if best.is_none() {
                    best = Some((chunks_offset, 0));
                }
                continue;
            };

            let stride = Self::detect_stride(ctx, guobject_array);
            let (pass, tried, samples) = Self::stage_c_score_uobject_names(
                ctx,
                guobject_array,
                pool_addr,
                chunks_offset,
                stride,
                50,
            );
            let needed = (tried * 7).div_ceil(10).max(35);
            if tried >= 10 && pass >= needed {
                debug!(
                    "validate_fname_pool_structural: pool=0x{:X} chunks_off=+0x{:X} \
                     ACCEPTED via A+B+C (Stage C: {}/{} pass, samples={:?})",
                    pool_addr, chunks_offset, pass, tried, samples
                );
                let challenger_better = best.map(|(_, s)| pass > s).unwrap_or(true);
                if challenger_better {
                    best = Some((chunks_offset, pass));
                }
            } else {
                debug!(
                    "validate_fname_pool_structural: pool=0x{:X} chunks_off=+0x{:X} \
                     A+B ok but Stage C weak ({}/{}, need {}, samples={:?})",
                    pool_addr, chunks_offset, pass, tried, needed, samples
                );
            }
        }

        best
    }

    /// Validate a candidate FNamePool base address. Tries each plausible
    /// `chunks_offset` (where the 8-byte pointer to the chunks array lives in
    /// the pool header), reads the first chunk, decodes the first interned
    /// entry, and accepts it if the decoded name matches one of the canonical
    /// well-known UE5 short names.
    ///
    /// Returns the validated `chunks_offset`, or `None`.
    #[allow(dead_code)]
    fn validate_fname_pool(ctx: &Target, resolved: usize) -> Option<usize> {
        for &chunks_offset in Self::FNAME_POOL_CHUNKS_OFFSETS {
            let mut chunks_header_buf = [0u8; 8];
            if ctx
                .read_bytes(resolved + chunks_offset, &mut chunks_header_buf)
                .is_err()
            {
                continue;
            }
            let chunks_addr = u64::from_le_bytes(chunks_header_buf) as usize;
            if chunks_addr == 0 || !chunks_addr.is_multiple_of(8) || chunks_addr <= 0x10000 {
                continue;
            }

            let mut first_chunk_buf = [0u8; 8];
            if ctx.read_bytes(chunks_addr, &mut first_chunk_buf).is_err() {
                continue;
            }
            let first_chunk = u64::from_le_bytes(first_chunk_buf) as usize;
            if first_chunk == 0 || !first_chunk.is_multiple_of(8) || first_chunk <= 0x10000 {
                continue;
            }

            // Read a generous prefix of the first chunk and scan it for any
            // canonical name appearing inside. We require at least TWO of
            // the canonical names to appear inside the first chunk's first
            // 16 KiB — random memory will rarely contain two of these short
            // identifiers as substrings together, but the FNamePool first
            // chunk always contains all of them as packed entries.
            let mut prefix = vec![0u8; 16 * 1024];
            if ctx.read_bytes(first_chunk, &mut prefix).is_err() {
                continue;
            }

            let mut hits: Vec<&'static str> = Vec::new();
            for &canon in Self::FNAME_CANONICAL_NAMES {
                let needle = canon.as_bytes();
                if needle.len() > prefix.len() {
                    continue;
                }
                // Find the needle as a substring; require it to be bounded
                // by non-identifier bytes (so we don't match
                // "ObjectProperty" when looking for "Object").
                let mut found = false;
                let mut i = 0;
                while i + needle.len() <= prefix.len() {
                    if prefix[i..i + needle.len()] == *needle {
                        let before = if i == 0 { 0u8 } else { prefix[i - 1] };
                        let after = prefix.get(i + needle.len()).copied().unwrap_or(0);
                        let bounded_before = !(before.is_ascii_alphanumeric() || before == b'_');
                        let bounded_after = !(after.is_ascii_alphanumeric() || after == b'_');
                        if bounded_before && bounded_after {
                            found = true;
                            break;
                        }
                    }
                    i += 1;
                }
                if found {
                    hits.push(canon);
                    if hits.len() >= 2 {
                        break;
                    }
                }
            }

            if hits.len() >= 2 {
                debug!(
                    "validate_fname_pool: pool=0x{:X} chunks_off=+0x{:X} first_chunk=0x{:X} \
                     ACCEPTED — canonical names found: {:?}",
                    resolved, chunks_offset, first_chunk, hits
                );
                return Some(chunks_offset);
            } else {
                debug!(
                    "validate_fname_pool: pool=0x{:X} chunks_off=+0x{:X} first_chunk=0x{:X}: \
                     only {} canonical names found ({:?}), need >=2",
                    resolved,
                    chunks_offset,
                    first_chunk,
                    hits.len(),
                    hits
                );
            }
        }
        None
    }

    /// Detect any 48/4C 8D <modrm> RIP-relative LEA encoding (mod=00, rm=101).
    #[inline]
    fn is_rip_relative_lea(bytes: &[u8]) -> bool {
        if bytes.len() < 3 {
            return false;
        }
        // REX byte: 0x48 (W=1, R=0) targets rax/rcx/rdx/rbx/rsp/rbp/rsi/rdi
        // REX byte: 0x4C (W=1, R=1) targets r8/r9/r10/r11/r12/r13/r14/r15
        let rex_ok = bytes[0] == 0x48 || bytes[0] == 0x4C;
        let opcode_ok = bytes[1] == 0x8D;
        // ModR/M: mod=00, rm=101 => RIP-relative => byte & 0xC7 == 0x05
        let modrm_ok = (bytes[2] & 0xC7) == 0x05;
        rex_ok && opcode_ok && modrm_ok
    }

    /// Detect 48/4C 8B <modrm> RIP-relative MOV reg, qword ptr [rip+disp32].
    /// Compilers very often access a global pool's *field* via this encoding,
    /// so the resolved address is `&pool + field_offset` rather than `&pool`.
    #[inline]
    fn is_rip_relative_mov(bytes: &[u8]) -> bool {
        if bytes.len() < 3 {
            return false;
        }
        let rex_ok = bytes[0] == 0x48 || bytes[0] == 0x4C;
        let opcode_ok = bytes[1] == 0x8B;
        let modrm_ok = (bytes[2] & 0xC7) == 0x05;
        rex_ok && opcode_ok && modrm_ok
    }

    /// Returns `(fname_pool_addr, chunks_offset)`.
    fn find_fname_pool(ctx: &Target, guobject_array: usize) -> Result<(usize, usize)> {
        let main_module = ctx.main();
        let base = main_module.base;
        let mut mod_bytes = vec![0u8; main_module.size];
        ctx.read_bytes(base, &mut mod_bytes)?;

        let validate = |pool_addr: usize| -> Option<usize> {
            // Fast pre-filter ONLY when pool_addr is inside the main module —
            // we can read chunks_addr from the snapshot instead of doing a
            // remote read. For heap pools (out-of-module), skip the pre-filter
            // and call the full validator directly.
            let pool_in_module = pool_addr >= base && pool_addr + 0x200 <= base + mod_bytes.len();

            if pool_in_module {
                // Pre-check: at least ONE chunks_offset must yield a
                // plausibly-shaped chunks pointer. If none do, reject early
                // without spending remote reads.
                let mut any_plausible = false;
                for &chunks_offset in Self::FNAME_POOL_CHUNKS_OFFSETS {
                    let off = (pool_addr + chunks_offset) - base;
                    if off + 8 > mod_bytes.len() {
                        continue;
                    }
                    let chunks_addr =
                        u64::from_le_bytes(mod_bytes[off..off + 8].try_into().unwrap()) as usize;
                    if chunks_addr != 0
                        && chunks_addr.is_multiple_of(8)
                        && chunks_addr > 0x10000
                        && chunks_addr < 0x0000_8000_0000_0000
                    {
                        any_plausible = true;
                        break;
                    }
                }
                if !any_plausible {
                    return None;
                }
            }

            // Full structural validation (iterates all chunks_offsets, runs
            // Stage A+B+C with remote reads).
            Self::validate_fname_pool_structural(ctx, pool_addr, Some(guobject_array))
                .map(|(off, _score)| off)
        };

        // 1. Target FName::ToString signatures and extract GNamePool from its body via iced-x86 disassembly
        let to_string_patterns = [
            "48 89 5C 24 08 57 48 83 EC 30 48 8B D9 48 8B FA E8 ?? ?? ?? ?? 48 8B D8 48 8D",
            "48 89 5C 24 08 48 89 74 24 10 57 48 83 EC 20 48 8B DA 48 8B F9 48 85 DB",
            "48 8B C4 48 89 58 08 48 89 70 10 48 89 78 18 4C 89 70 20 55 48 8D 68 A1",
            "48 83 EC 38 48 8B D9 48 8B FA E8 ?? ?? ?? ?? 48 8B D8",
            "48 83 EC 28 48 8B D9 48 8B FA E8 ?? ?? ?? ?? 48 8B D8",
        ];

        for (pat_idx, pat_str) in to_string_patterns.iter().enumerate() {
            let pat = Pattern::parse(pat_str)?;
            if let Ok(Some(addr)) = ctx.scan_module("", &pat) {
                debug!(
                    "Matched FName::ToString pattern {} at 0x{:X}",
                    pat_idx, addr
                );
                // Read 1024 bytes from function start and disassemble with iced-x86.
                // 256 was too tight — GNamePool is sometimes accessed deeper
                // in the prologue setup or after an inlined helper call.
                let mut func_bytes = vec![0u8; 1024];
                let mut ip_rel_candidates: Vec<usize> = Vec::new();
                if ctx.read_bytes(addr, &mut func_bytes).is_ok() {
                    let mut decoder =
                        Decoder::with_ip(64, &func_bytes, addr as u64, DecoderOptions::NONE);
                    let mut instr = Instruction::default();
                    while decoder.can_decode() {
                        decoder.decode_out(&mut instr);
                        if instr.is_invalid() {
                            break;
                        }
                        if instr.is_ip_rel_memory_operand() {
                            let candidate = instr.ip_rel_memory_address() as usize;
                            ip_rel_candidates.push(candidate);
                            if let Some(chunks_offset) = validate(candidate) {
                                info!(
                                    "Successfully located FNamePool at 0x{:X} (chunks_off=+0x{:X}) via FName::ToString disassembly pattern {}",
                                    candidate, chunks_offset, pat_idx
                                );
                                return Ok((candidate, chunks_offset));
                            }
                            // Also try dereferencing the candidate pointer (modern UE5 sometimes stores FNamePool as a global pointer)
                            let mut ptr_buf = [0u8; 8];
                            if ctx.read_bytes(candidate, &mut ptr_buf).is_ok() {
                                let deref_candidate = u64::from_le_bytes(ptr_buf) as usize;
                                if deref_candidate != 0
                                    && deref_candidate.is_multiple_of(8)
                                    && let Some(chunks_offset) = validate(deref_candidate)
                                {
                                    info!(
                                        "Successfully located FNamePool at 0x{:X} (chunks_off=+0x{:X}) via FName::ToString pointer deref pattern {}",
                                        deref_candidate, chunks_offset, pat_idx
                                    );
                                    return Ok((deref_candidate, chunks_offset));
                                }
                            }
                        }
                    }
                }
                debug!(
                    "FName::ToString pattern {} at 0x{:X}: walked {} RIP-rel candidates, none validated. First few: {:?}",
                    pat_idx,
                    addr,
                    ip_rel_candidates.len(),
                    ip_rel_candidates
                        .iter()
                        .take(10)
                        .map(|a| format!("0x{:X}", a))
                        .collect::<Vec<_>>()
                );
            }
        }

        // 2. Fallback: Classic FNamePool::Resolve patterns (direct RIP-relative global patterns)
        let resolve_patterns = [
            "48 8D 0D ?? ?? ?? ?? E8 ?? ?? ?? ?? 46 0F B6 F8",
            "48 8D 0D ?? ?? ?? ?? E8 ?? ?? ?? ?? 48 8B C8 48 8B 10",
            "48 8D 0D ?? ?? ?? ?? E8 ?? ?? ?? ?? C5 F8 29 7C 24 20",
        ];

        for (i, pat_str) in resolve_patterns.iter().enumerate() {
            let pat = Pattern::parse(pat_str)?;
            if let Ok(Some(addr)) = ctx.scan_module("", &pat) {
                let mut disp = [0u8; 4];
                if ctx.read_bytes(addr + 3, &mut disp).is_ok() {
                    let offset = i32::from_le_bytes(disp) as i64;
                    let resolved = (addr as i64 + 7 + offset) as usize;
                    if let Some(chunks_offset) = validate(resolved) {
                        debug!(
                            "FNamePool Pattern {} matched at 0x{:X} (chunks_off=+0x{:X})",
                            i, resolved, chunks_offset
                        );
                        return Ok((resolved, chunks_offset));
                    }
                    // Try dereferencing
                    let mut ptr_buf = [0u8; 8];
                    if ctx.read_bytes(resolved, &mut ptr_buf).is_ok() {
                        let deref_candidate = u64::from_le_bytes(ptr_buf) as usize;
                        if deref_candidate != 0
                            && deref_candidate.is_multiple_of(8)
                            && let Some(chunks_offset) = validate(deref_candidate)
                        {
                            debug!(
                                "FNamePool Pattern {} deref matched at 0x{:X} (chunks_off=+0x{:X})",
                                i, deref_candidate, chunks_offset
                            );
                            return Ok((deref_candidate, chunks_offset));
                        }
                    }
                }
            }
        }

        // 3. Fallback: Structural Scanning (highly optimized in-memory .text scanner searching for all RIP-relative LEAs)
        info!(
            "Standard FNamePool signatures failed. Running structural RIP-relative LEA scanning fallback..."
        );

        // Dynamically query PE sections to find code/executable ranges, supporting custom layouts
        let mut code_sections = Vec::new();
        let base = main_module.base;
        let mut e_lfanew = [0u8; 4];
        if ctx.read_bytes(base + 0x3C, &mut e_lfanew).is_ok() {
            let pe_off = u32::from_le_bytes(e_lfanew) as usize;
            let pe_addr = base + pe_off;
            let mut coff = [0u8; 20];
            if ctx.read_bytes(pe_addr + 4, &mut coff).is_ok() {
                let num_sections = u16::from_le_bytes([coff[2], coff[3]]) as usize;
                let opt_header_size = u16::from_le_bytes([coff[16], coff[17]]) as usize;
                let sections_addr = pe_addr + 4 + 20 + opt_header_size;
                for i in 0..num_sections {
                    let sect_addr = sections_addr + i * 40;
                    let mut sect = [0u8; 40];
                    if ctx.read_bytes(sect_addr, &mut sect).is_ok() {
                        let vsize =
                            u32::from_le_bytes([sect[8], sect[9], sect[10], sect[11]]) as usize;
                        let vaddr =
                            u32::from_le_bytes([sect[12], sect[13], sect[14], sect[15]]) as usize;
                        let characts = u32::from_le_bytes([sect[36], sect[37], sect[38], sect[39]]);
                        // Executable code characteristics: IMAGE_SCN_MEM_EXECUTE (0x20000000) or IMAGE_SCN_CNT_CODE (0x00000020)
                        if (characts & 0x20000020) != 0 {
                            code_sections.push((base + vaddr, base + vaddr + vsize));
                        }
                    }
                }
            }
        }

        // If we couldn't parse sections, fall back to the text_base/size
        if code_sections.is_empty() {
            code_sections.push((
                main_module.text_base(),
                main_module.text_base() + main_module.text_size,
            ));
        }

        debug!(
            "Structural scanner detected code sections: {:?}",
            code_sections
        );

        let mut tried: std::collections::HashSet<usize> = std::collections::HashSet::new();
        let mut scanned_leas: usize = 0;

        for &(start, end) in &code_sections {
            let start_off = start.saturating_sub(main_module.base);
            let end_off = (end.saturating_sub(main_module.base)).min(mod_bytes.len());
            if start_off >= end_off {
                continue;
            }

            for inst_addr in start..(end.saturating_sub(7)) {
                let offset_in_mod = inst_addr - main_module.base;
                if offset_in_mod + 7 > mod_bytes.len() {
                    break;
                }
                let opcode = &mod_bytes[offset_in_mod..offset_in_mod + 3];

                let is_lea = Self::is_rip_relative_lea(opcode);
                let is_mov = !is_lea && Self::is_rip_relative_mov(opcode);
                if !is_lea && !is_mov {
                    continue;
                }
                scanned_leas += 1;

                let disp = i32::from_le_bytes(
                    mod_bytes[offset_in_mod + 3..offset_in_mod + 7]
                        .try_into()
                        .unwrap(),
                ) as i64;
                let resolved = (inst_addr as i64 + 7 + disp) as usize;

                // Skip targets that point inside any executable code section (globals are in data sections)
                let is_code_target = code_sections
                    .iter()
                    .any(|&(s, e)| resolved >= s && resolved < e);
                if is_code_target {
                    continue;
                }
                if !resolved.is_multiple_of(8) {
                    continue;
                }

                let back_offsets: &[usize] = if is_mov {
                    &[0x00, 0x08, 0x10, 0x18, 0x20, 0x28, 0x30, 0x40]
                } else {
                    &[0x00]
                };

                // For BOTH lea and mov: if `resolved` lies inside the module's
                // data sections, the 8 bytes there may be a global pointer to
                // a heap-allocated FNamePool. Snapshot read is free; validate
                // the dereferenced address.
                if resolved >= main_module.base
                    && resolved + 8 <= main_module.base + mod_bytes.len()
                {
                    let off = resolved - main_module.base;
                    let deref =
                        u64::from_le_bytes(mod_bytes[off..off + 8].try_into().unwrap()) as usize;
                    if deref != 0
                        && deref.is_multiple_of(8)
                        && deref > 0x10000
                        && deref < 0x0000_8000_0000_0000
                        // heap target: not inside module image
                        && (deref < main_module.base
                            || deref >= main_module.base + main_module.size)
                        && tried.insert(deref)
                        && let Some(chunks_offset) = validate(deref)
                    {
                        let rex = opcode[0];
                        let modrm = opcode[2];
                        let reg_id = ((modrm >> 3) & 0x7) | ((rex & 0x04) << 1);
                        info!(
                            "Located FNamePool via structural scanner (deref path): pool=0x{:X} chunks_off=+0x{:X} \
                             via {} at 0x{:X} -> [global 0x{:X}] (rex=0x{:02X} dest_reg_id={}, scanned {} insns)",
                            deref,
                            chunks_offset,
                            if is_lea { "LEA" } else { "MOV" },
                            inst_addr,
                            resolved,
                            rex,
                            reg_id,
                            scanned_leas
                        );
                        return Ok((deref, chunks_offset));
                    }
                }

                for &back in back_offsets {
                    if back > resolved {
                        continue;
                    }
                    let candidate = resolved - back;
                    if !tried.insert(candidate) {
                        continue;
                    }
                    if let Some(chunks_offset) = validate(candidate) {
                        let rex = opcode[0];
                        let modrm = opcode[2];
                        let reg_id = ((modrm >> 3) & 0x7) | ((rex & 0x04) << 1);
                        info!(
                            "Located FNamePool via structural scanner: addr=0x{:X} chunks_off=+0x{:X} \
                             via {} at 0x{:X} (rex=0x{:02X} dest_reg_id={} back_off=0x{:X}, scanned {} insns)",
                            candidate,
                            chunks_offset,
                            if is_lea { "LEA" } else { "MOV" },
                            inst_addr,
                            rex,
                            reg_id,
                            back,
                            scanned_leas
                        );
                        return Ok((candidate, chunks_offset));
                    }
                }
            }
        }

        info!(
            "Structural scan exhausted: {} RIP-rel insns scanned, {} unique candidates tried, none passed validator",
            scanned_leas,
            tried.len()
        );
        bail!(
            "Could not locate FNamePool via signature-first scanning or optimized structural fallbacks. Brute-force scanning is disabled."
        )
    }

    /// Temporary FName decoder used during layout probing. Does not depend on
    /// an existing `UeEngine` instance (engine is being constructed).
    fn decode_temp(
        ctx: &Target,
        fname_pool: usize,
        chunks_offset: usize,
        comparison_index: u32,
        number: u32,
    ) -> Result<String> {
        let chunk_index = (comparison_index >> 16) as usize;
        let byte_offset = ((comparison_index & 0xFFFF) << 1) as usize;

        let mut chunks_header = [0u8; 8];
        ctx.read_bytes(fname_pool + chunks_offset, &mut chunks_header)?;
        let chunks_addr = u64::from_le_bytes(chunks_header) as usize;

        let mut chunk_base_buf = [0u8; 8];
        ctx.read_bytes(chunks_addr + chunk_index * 8, &mut chunk_base_buf)?;
        let chunk_base = u64::from_le_bytes(chunk_base_buf) as usize;
        if chunk_base == 0 {
            bail!("Chunk base is null.");
        }

        let entry_addr = chunk_base + byte_offset;
        let mut header_buf = [0u8; 2];
        ctx.read_bytes(entry_addr, &mut header_buf)?;
        let header = u16::from_le_bytes(header_buf);
        let len = ((header >> 6) & 0x3FF) as usize;
        if len == 0 || len > 1024 {
            bail!("Invalid name length.");
        }
        let is_wide = (header & 1) != 0;
        if is_wide {
            let mut buf = vec![0u8; len * 2];
            ctx.read_bytes(entry_addr + 2, &mut buf)?;
            let words: Vec<u16> = buf
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            let s = String::from_utf16_lossy(&words);
            Ok(if number != 0 {
                format!("{}_{}", s, number - 1)
            } else {
                s
            })
        } else {
            let mut buf = vec![0u8; len];
            ctx.read_bytes(entry_addr + 2, &mut buf)?;
            let s = String::from_utf8_lossy(&buf).into_owned();
            Ok(if number != 0 {
                format!("{}_{}", s, number - 1)
            } else {
                s
            })
        }
    }

    /// Returns `(stride, offsets, uobject_layout_validated, chunks_offset)`.
    ///
    /// The returned `chunks_offset` may differ from the input — UObject
    /// FName decoding is a much stronger oracle than the substring-based
    /// validation in `find_fname_pool`, so this probe is allowed to override
    /// the chunks_offset if a different one gives a higher decode score.
    fn probe_layout(
        ctx: &Target,
        guobject_array: usize,
        fname_pool: usize,
        chunks_offset: usize,
    ) -> Result<(usize, UeOffsets, bool, usize)> {
        // Read ChunkedFixedUObjectArray header (need num_elements + num_chunks
        // so we can walk across chunks when collecting samples).
        let mut header = [0u8; 32];
        ctx.read_bytes(guobject_array, &mut header)?;
        let objects_ptr = u64::from_le_bytes(header[0..8].try_into().unwrap()) as usize;
        let num_elements = u32::from_le_bytes(header[20..24].try_into().unwrap()) as usize;
        let num_chunks = u32::from_le_bytes(header[28..32].try_into().unwrap()) as usize;
        if objects_ptr == 0 {
            bail!("GUObjectArray.Objects base pointer is null.");
        }

        // Read pointer to first chunk
        let mut chunk_ptr_buf = [0u8; 8];
        ctx.read_bytes(objects_ptr, &mut chunk_ptr_buf)?;
        let first_chunk = u64::from_le_bytes(chunk_ptr_buf) as usize;
        if first_chunk == 0 {
            bail!("First FUObjectItem chunk pointer is null.");
        }

        // Detect stride: typical values are 16 (UE default), 24, 20
        let strides = [16, 24, 20];
        let mut stride = 16;

        for s in strides {
            let mut valid_count = 0;
            for i in 0..10 {
                let item_addr = first_chunk + i * s;
                let mut obj_ptr_buf = [0u8; 8];
                if ctx.read_bytes(item_addr, &mut obj_ptr_buf).is_ok() {
                    let obj_ptr = u64::from_le_bytes(obj_ptr_buf) as usize;
                    if obj_ptr != 0 && obj_ptr.is_multiple_of(8) && obj_ptr > 0x10000 {
                        // Ensure it's readable
                        let mut test = [0u8; 8];
                        if ctx.read_bytes(obj_ptr, &mut test).is_ok() {
                            valid_count += 1;
                        }
                    }
                }
            }
            if valid_count >= 8 {
                stride = s;
                break;
            }
        }

        // Collect up to 200 live UObject pointers across chunks. We iterate by
        // global index so we don't fall off the end of the first chunk.
        const SAMPLE_TARGET: usize = 200;
        const CHUNK_SIZE: usize = 65536;
        let mut sample_objs: Vec<usize> = Vec::with_capacity(SAMPLE_TARGET);

        let total_elements = if num_elements == 0 || num_chunks == 0 {
            // Best-effort: just walk the first chunk for SAMPLE_TARGET items.
            SAMPLE_TARGET * 4
        } else {
            num_elements
        };

        for index in 0..total_elements {
            if sample_objs.len() >= SAMPLE_TARGET {
                break;
            }
            let chunk_idx = index / CHUNK_SIZE;
            let within_chunk = index % CHUNK_SIZE;
            if num_chunks > 0 && chunk_idx >= num_chunks {
                break;
            }

            // Resolve chunk address.
            let chunk_addr = if chunk_idx == 0 {
                first_chunk
            } else {
                let mut buf = [0u8; 8];
                if ctx
                    .read_bytes(objects_ptr + chunk_idx * 8, &mut buf)
                    .is_err()
                {
                    break;
                }
                u64::from_le_bytes(buf) as usize
            };
            if chunk_addr == 0 {
                continue;
            }

            let item_addr = chunk_addr + within_chunk * stride;
            let mut obj_ptr_buf = [0u8; 8];
            if ctx.read_bytes(item_addr, &mut obj_ptr_buf).is_err() {
                continue;
            }
            let obj_ptr = u64::from_le_bytes(obj_ptr_buf) as usize;
            if obj_ptr == 0 || !obj_ptr.is_multiple_of(8) || obj_ptr <= 0x10000 {
                continue;
            }
            // Smoke-test the read.
            let mut test = [0u8; 8];
            if ctx.read_bytes(obj_ptr, &mut test).is_err() {
                continue;
            }
            sample_objs.push(obj_ptr);
        }

        if sample_objs.is_empty() {
            bail!("Could not identify any valid UObjects in GUObjectArray.");
        }
        info!(
            "probe_layout: collected {} live UObject samples (target {})",
            sample_objs.len(),
            SAMPLE_TARGET
        );

        // -----------------------------------------------------------------
        // STEP 1: sweep uobject_name_private offsets.
        // -----------------------------------------------------------------
        const NAME_OFF_CANDIDATES: &[usize] = &[
            0x10, 0x14, 0x18, 0x1C, 0x20, 0x24, 0x28, 0x2C, 0x30, 0x34, 0x38, 0x40,
        ];

        // Canonical UE class/identifier names to bolster the "valid" heuristic.
        const CANONICAL_NAMES: &[&str] = &[
            "Object",
            "Class",
            "Package",
            "Function",
            "Struct",
            "Field",
            "Enum",
            "Actor",
            "Pawn",
            "PlayerController",
            "GameMode",
            "World",
            "Level",
            "ArrayProperty",
            "ObjectProperty",
            "IntProperty",
            "BoolProperty",
            "FloatProperty",
            "StructProperty",
            "ByteProperty",
            "NameProperty",
            "StrProperty",
            "TextProperty",
            "DoubleProperty",
            "Int64Property",
            "Int8Property",
            "Int16Property",
            "UInt16Property",
            "UInt32Property",
            "UInt64Property",
            "EnumProperty",
            "MapProperty",
            "SetProperty",
            "DelegateProperty",
            "MulticastDelegateProperty",
            "InterfaceProperty",
            "SoftObjectProperty",
            "WeakObjectProperty",
            "LazyObjectProperty",
            "ScriptStruct",
            "Default__Object",
            "CoreUObject",
            "Engine",
            "Transient",
            "None",
        ];

        let is_ue_identifier = |s: &str| -> bool {
            let len = s.len();
            if !(2..=128).contains(&len) {
                return false;
            }
            if CANONICAL_NAMES.contains(&s) {
                return true;
            }
            let mut chars = s.chars();
            match chars.next() {
                Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
                _ => return false,
            }
            chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        };

        let total = sample_objs.len();
        let min_pass = total.div_ceil(2); // >=50%

        // One-line probe diagnostic: dump raw u32 fields at the first 2 UObjects.
        for (idx, &obj_ptr) in sample_objs.iter().take(2).enumerate() {
            let mut snap = [0u8; 0x48];
            if ctx.read_bytes(obj_ptr, &mut snap).is_ok() {
                let mut line = format!("probe diag obj[{}] @0x{:X}:", idx, obj_ptr);
                for off in (0x00usize..0x48).step_by(4) {
                    let v = u32::from_le_bytes(snap[off..off + 4].try_into().unwrap());
                    line.push_str(&format!(" +{:02X}={:08X}", off, v));
                }
                debug!("{}", line);
            }
        }

        // Joint sweep across (name_off, chunks_off). The FNamePool locator may
        // have accepted a non-canonical chunks_offset (substring matches in
        // random memory). Re-validate against UObject FNames here, which is a
        // far stronger oracle.
        let chunks_off_candidates = Self::FNAME_POOL_CHUNKS_OFFSETS;

        // Print first 2 UObject raw bytes around offset 0x18 for debugging
        for (idx, &obj_ptr) in sample_objs.iter().take(2).enumerate() {
            let mut nbuf = [0u8; 8];
            if ctx.read_bytes(obj_ptr + 0x18, &mut nbuf).is_ok() {
                let ci = u32::from_le_bytes(nbuf[0..4].try_into().unwrap());
                let num = u32::from_le_bytes(nbuf[4..8].try_into().unwrap());
                info!(
                    "Debug: obj[{}] @0x{:X} +0x18 -> ci=0x{:X} num=0x{:X}",
                    idx, obj_ptr, ci, num
                );
                for &co in chunks_off_candidates {
                    match Self::decode_temp(ctx, fname_pool, co, ci, num) {
                        Ok(name) => info!("  co=+0x{:X} -> decoded: \"{}\"", co, name),
                        Err(e) => {
                            if co == 0x200 {
                                info!("  co=+0x200 -> decode error: {:?}", e)
                            }
                        }
                    }
                }
            }
        }

        let try_decode = |co: usize, ci: u32, num: u32| -> Option<String> {
            Self::decode_temp(ctx, fname_pool, co, ci, num).ok()
        };

        let mut best_name_off: Option<(usize, usize, usize, Vec<String>)> = None;

        for &cand in NAME_OFF_CANDIDATES {
            // For each candidate name offset, try EACH candidate chunks_offset
            // and pick the chunks_offset that maximizes valid decodes for THIS
            // name offset.
            let mut best_co_for_name: Option<(usize, usize, Vec<String>)> = None;
            for &co in chunks_off_candidates {
                let mut count = 0usize;
                let mut samples: Vec<String> = Vec::new();
                for &obj_ptr in &sample_objs {
                    let mut buf = [0u8; 8];
                    if ctx.read_bytes(obj_ptr + cand, &mut buf).is_err() {
                        continue;
                    }
                    let ci = u32::from_le_bytes(buf[0..4].try_into().unwrap());
                    let num = u32::from_le_bytes(buf[4..8].try_into().unwrap());
                    if ci == 0 || (ci >> 16) >= 1024 || num > 0xFFFF {
                        continue;
                    }
                    if let Some(name) = try_decode(co, ci, num)
                        && is_ue_identifier(&name)
                    {
                        count += 1;
                        if samples.len() < 5 {
                            samples.push(name);
                        }
                    }
                }
                let better = match &best_co_for_name {
                    None => true,
                    Some((_, prev, _)) => count > *prev,
                };
                if better {
                    best_co_for_name = Some((co, count, samples));
                }
            }
            if let Some((co, count, samples)) = best_co_for_name {
                info!(
                    "name_private sweep: name_off=0x{:X}  best chunks_off=+0x{:X}  valid={}/{}  samples={:?}",
                    cand, co, count, total, samples
                );
                let better = match &best_name_off {
                    None => true,
                    Some((_, _, prev_count, _)) => count > *prev_count,
                };
                if better {
                    best_name_off = Some((cand, co, count, samples));
                }
            }
        }

        let (name_off, chunks_off_locked, name_score, name_samples) = match best_name_off {
            Some(v) => v,
            None => bail!("probe_layout: no name_private candidate produced any decode"),
        };

        if name_score < min_pass {
            warn!(
                "probe_layout: BEST name_off=0x{:X} (chunks_off=+0x{:X}) only scored {}/{} \
                 (need >= {}). UObject layout could not be locked. samples={:?}",
                name_off, chunks_off_locked, name_score, total, min_pass, name_samples
            );
            // Build a placeholder layout so the caller still has *something*.
            let placeholder = UeOffsets {
                uobject_class_private: name_off.saturating_sub(0x08),
                uobject_name_private: name_off,
                uobject_outer_private: name_off + 0x08,
                ustruct_super_struct: 0x40,
                ustruct_child_properties: 0x50,
                ustruct_children: 0x48,
                ffield_class_private: 0x08,
                ffield_next: 0x10,
                ffield_name_private: 0x18,
                fproperty_offset_internal: 0x40,
                fproperty_element_size: 0x34,
                ufield_next: 0x28,
                ufunction_flags: 0x88,
                ufunction_func: 0xB0,
            };
            return Ok((stride, placeholder, false, chunks_offset));
        }
        info!(
            "probe_layout: LOCKED uobject_name_private=0x{:X}, chunks_off=+0x{:X} \
             ({}/{} valid, samples={:?})",
            name_off, chunks_off_locked, name_score, total, name_samples
        );
        // Use the locked chunks_offset for subsequent decodes.
        let chunks_offset = chunks_off_locked;

        // -----------------------------------------------------------------
        // STEP 2: sweep uobject_class_private. Candidates relative to name.
        // -----------------------------------------------------------------
        let class_candidates: Vec<usize> = [
            name_off.checked_sub(0x18),
            name_off.checked_sub(0x10),
            name_off.checked_sub(0x0C),
            name_off.checked_sub(0x08),
            name_off.checked_sub(0x04),
        ]
        .into_iter()
        .flatten()
        .collect();

        let mut best_class_off: Option<(usize, usize, usize, Vec<String>)> = None;

        for &cand in &class_candidates {
            let mut count = 0usize;
            let mut class_ptrs: Vec<usize> = Vec::new();
            let mut samples: Vec<String> = Vec::new();
            for &obj_ptr in &sample_objs {
                let mut buf = [0u8; 8];
                if ctx.read_bytes(obj_ptr + cand, &mut buf).is_err() {
                    continue;
                }
                let class_ptr = u64::from_le_bytes(buf) as usize;
                if class_ptr == 0 || !class_ptr.is_multiple_of(8) || class_ptr <= 0x10000 {
                    continue;
                }
                // Read FName at class_ptr + name_off.
                let mut cname_buf = [0u8; 8];
                if ctx
                    .read_bytes(class_ptr + name_off, &mut cname_buf)
                    .is_err()
                {
                    continue;
                }
                let ci = u32::from_le_bytes(cname_buf[0..4].try_into().unwrap());
                let num = u32::from_le_bytes(cname_buf[4..8].try_into().unwrap());
                if ci == 0 || (ci >> 16) >= 1024 || num > 0xFFFF {
                    continue;
                }
                if let Ok(name) = Self::decode_temp(ctx, fname_pool, chunks_offset, ci, num)
                    && is_ue_identifier(&name)
                {
                    count += 1;
                    class_ptrs.push(class_ptr);
                    if samples.len() < 5 {
                        samples.push(name);
                    }
                }
            }
            let mut unique = class_ptrs.clone();
            unique.sort_unstable();
            unique.dedup();
            let unique_count = unique.len();
            info!(
                "class_private sweep: offset=0x{:X}  valid={}/{}  unique_classes={}  samples={:?}",
                cand, count, total, unique_count, samples
            );
            // Require: count >= 50% AND unique_class_count < 30% of total
            // (instances share a class).
            let passes_count = count >= min_pass;
            let passes_uniq = unique_count < (total * 3 / 10).max(1);
            if !passes_count || !passes_uniq {
                continue;
            }
            let better = match &best_class_off {
                None => true,
                Some((prev_off, prev_count, _, _)) => {
                    // Higher count wins, then prefer smaller offset for stability.
                    count > *prev_count || (count == *prev_count && cand < *prev_off)
                }
            };
            if better {
                best_class_off = Some((cand, count, unique_count, samples));
            }
        }

        let (class_off, class_score, class_unique, class_samples) = match best_class_off {
            Some(v) => v,
            None => {
                warn!(
                    "probe_layout: no class_private candidate passed (need {}/{} valid AND \
                     unique_classes < 30%). UObject layout NOT validated.",
                    min_pass, total
                );
                let placeholder = UeOffsets {
                    uobject_class_private: name_off.saturating_sub(0x08),
                    uobject_name_private: name_off,
                    uobject_outer_private: name_off + 0x08,
                    ustruct_super_struct: 0x40,
                    ustruct_child_properties: 0x50,
                    ustruct_children: 0x48,
                    ffield_class_private: 0x08,
                    ffield_next: 0x10,
                    ffield_name_private: 0x18,
                    fproperty_offset_internal: 0x40,
                    fproperty_element_size: 0x34,
                    ufield_next: 0x28,
                    ufunction_flags: 0x88,
                    ufunction_func: 0xB0,
                };
                return Ok((stride, placeholder, false, chunks_offset));
            }
        };
        info!(
            "probe_layout: LOCKED uobject_class_private=0x{:X} ({}/{} valid, \
             {} unique classes, samples={:?})",
            class_off, class_score, total, class_unique, class_samples
        );

        // -----------------------------------------------------------------
        // STEP 3: sweep uobject_outer_private.
        // -----------------------------------------------------------------
        const PACKAGE_TERMINALS: &[&str] = &[
            "Engine",
            "CoreUObject",
            "Game",
            "Transient",
            "/Script/CoreUObject",
            "/Script/Engine",
            "None",
        ];
        let is_package_terminal = |s: &str| -> bool {
            if PACKAGE_TERMINALS.contains(&s) {
                return true;
            }
            // Package-shaped name: usually starts with '/' or is an identifier
            // and length is reasonable.
            s.starts_with('/') || (is_ue_identifier(s) && s.len() >= 3)
        };

        let outer_candidates: [usize; 3] = [name_off + 0x08, name_off + 0x10, name_off + 0x18];
        let outer_min_pass = (total * 7 / 10).max(1); // >=70%

        let mut best_outer_off: Option<(usize, usize, Vec<String>)> = None;

        for &cand in &outer_candidates {
            let mut clean_count = 0usize;
            let mut terminal_samples: Vec<String> = Vec::new();
            for &obj_ptr in &sample_objs {
                // Walk outer chain up to 16 hops, detecting cycles.
                let mut seen: std::collections::HashSet<usize> = std::collections::HashSet::new();
                let mut cur = obj_ptr;
                seen.insert(cur);
                let mut last_name: Option<String> = None;
                let mut ok = true;
                for _ in 0..16 {
                    let mut buf = [0u8; 8];
                    if ctx.read_bytes(cur + cand, &mut buf).is_err() {
                        ok = false;
                        break;
                    }
                    let outer = u64::from_le_bytes(buf) as usize;
                    if outer == 0 {
                        // Reached root: read CURRENT object's name as the
                        // terminal name.
                        let mut nbuf = [0u8; 8];
                        if ctx.read_bytes(cur + name_off, &mut nbuf).is_ok() {
                            let ci = u32::from_le_bytes(nbuf[0..4].try_into().unwrap());
                            let num = u32::from_le_bytes(nbuf[4..8].try_into().unwrap());
                            if let Ok(name) =
                                Self::decode_temp(ctx, fname_pool, chunks_offset, ci, num)
                            {
                                last_name = Some(name);
                            }
                        }
                        break;
                    }
                    if !outer.is_multiple_of(8) || outer <= 0x10000 {
                        ok = false;
                        break;
                    }
                    if !seen.insert(outer) {
                        ok = false; // cycle
                        break;
                    }
                    cur = outer;
                }
                if !ok {
                    continue;
                }
                if let Some(name) = last_name
                    && is_package_terminal(&name)
                {
                    clean_count += 1;
                    if terminal_samples.len() < 5 {
                        terminal_samples.push(name);
                    }
                }
            }
            info!(
                "outer_private sweep: offset=0x{:X}  clean_chains={}/{}  terminals={:?}",
                cand, clean_count, total, terminal_samples
            );
            if clean_count >= outer_min_pass {
                let better = match &best_outer_off {
                    None => true,
                    Some((_, prev, _)) => clean_count > *prev,
                };
                if better {
                    best_outer_off = Some((cand, clean_count, terminal_samples));
                }
            }
        }

        let outer_off = match &best_outer_off {
            Some((o, score, samples)) => {
                info!(
                    "probe_layout: LOCKED uobject_outer_private=0x{:X} ({}/{} clean, \
                     terminals={:?})",
                    o, score, total, samples
                );
                *o
            }
            None => {
                // Fall back to canonical name + 0x08 without locking.
                warn!(
                    "probe_layout: outer_private sweep did not reach 70% threshold; \
                     defaulting to name_off+0x08=0x{:X} (UObject layout still valid).",
                    name_off + 0x08
                );
                name_off + 0x08
            }
        };

        // -----------------------------------------------------------------
        // Build final offsets. UStruct/FField/UFunction offsets follow the
        // UE5 5.0+ convention; FProperty probe runs after this.
        // -----------------------------------------------------------------
        // UStruct member offsets are relative to UStruct base. The classic
        // UE5 layout places SuperStruct ~0x40, Children ~0x48,
        // ChildProperties ~0x50, but in builds that push UObject members up
        // (uobject_name_private=0x28 instead of 0x18) UStruct members shift
        // similarly. Use a relative-to-name-offset approximation that lines
        // up for both 0x18 and 0x28 builds.
        //
        // For uobject_name_private=0x18 (classic): super=0x40, children=0x48,
        //   child_properties=0x50.
        // For uobject_name_private=0x28 (Lego/UE5.1+ adjusted): shift +0x10.
        let shift = name_off as i64 - 0x18i64;
        let shift = shift.max(0) as usize;

        let offsets = UeOffsets {
            uobject_class_private: class_off,
            uobject_name_private: name_off,
            uobject_outer_private: outer_off,
            ustruct_super_struct: 0x40 + shift,
            ustruct_children: 0x48 + shift,
            ustruct_child_properties: 0x50 + shift,
            ffield_class_private: 0x08,
            ffield_next: 0x10,
            ffield_name_private: 0x18,
            fproperty_offset_internal: 0x40,
            fproperty_element_size: 0x34,
            ufield_next: 0x28 + shift,
            ufunction_flags: 0x88 + shift,
            ufunction_func: 0xB0 + shift,
        };

        info!("probe_layout: FINAL UeOffsets = {:?}", offsets);
        Ok((stride, offsets, true, chunks_offset))
    }

    /// Validate `fproperty_offset_internal` and `fproperty_element_size` against
    /// a known UStruct/UClass walked from the live process. Mutates `offsets`
    /// in place when a working pair is found. Returns true if a valid pair was
    /// locked in.
    fn probe_fproperty_offsets(
        ctx: &Target,
        guobject_array: usize,
        fname_pool: usize,
        chunks_offset: usize,
        stride: usize,
        offsets: &mut UeOffsets,
    ) -> Result<bool> {
        // Walk the GUObjectArray and locate a UStruct whose name is one of a
        // small set of well-known structs that always have properties.
        const TARGET_NAMES: &[&str] = &["Object", "Actor", "Pawn", "PlayerController", "GameMode"];

        // Read array header
        let mut header = [0u8; 32];
        ctx.read_bytes(guobject_array, &mut header)?;
        let objects_ptr = u64::from_le_bytes(header[0..8].try_into().unwrap()) as usize;
        let num_elements = u32::from_le_bytes(header[20..24].try_into().unwrap()) as usize;
        let num_chunks = u32::from_le_bytes(header[28..32].try_into().unwrap()) as usize;
        if objects_ptr == 0 || num_elements == 0 || num_chunks == 0 {
            return Ok(false);
        }

        let chunk_size = 65536;

        let decode =
            |idx: u32, num: u32| Self::decode_temp(ctx, fname_pool, chunks_offset, idx, num);

        // Find an FProperty pointer from a target UStruct.
        let mut found_prop_ptr: Option<usize> = None;
        let mut found_struct_name = String::new();

        'outer: for chunk_idx in 0..num_chunks {
            let mut chunk_ptr_buf = [0u8; 8];
            if ctx
                .read_bytes(objects_ptr + chunk_idx * 8, &mut chunk_ptr_buf)
                .is_err()
            {
                break;
            }
            let chunk_addr = u64::from_le_bytes(chunk_ptr_buf) as usize;
            if chunk_addr == 0 {
                continue;
            }
            for within_chunk in 0..chunk_size {
                let index = chunk_idx * chunk_size + within_chunk;
                if index >= num_elements {
                    break 'outer;
                }
                let item_addr = chunk_addr + within_chunk * stride;
                let mut item_buf = [0u8; 8];
                if ctx.read_bytes(item_addr, &mut item_buf).is_err() {
                    continue;
                }
                let obj_ptr = u64::from_le_bytes(item_buf) as usize;
                if obj_ptr == 0 || !obj_ptr.is_multiple_of(8) || obj_ptr < 0x10000 {
                    continue;
                }

                // Decode this object's name and its class's name.
                let mut name_buf = [0u8; 8];
                if ctx
                    .read_bytes(obj_ptr + offsets.uobject_name_private, &mut name_buf)
                    .is_err()
                {
                    continue;
                }
                let n_ci = u32::from_le_bytes(name_buf[0..4].try_into().unwrap());
                let n_num = u32::from_le_bytes(name_buf[4..8].try_into().unwrap());
                let Ok(name) = decode(n_ci, n_num) else {
                    continue;
                };

                if !TARGET_NAMES.contains(&name.as_str()) {
                    continue;
                }

                // Ensure class is "Class" so we're sure this is a UStruct/UClass.
                let mut class_ptr_buf = [0u8; 8];
                if ctx
                    .read_bytes(obj_ptr + offsets.uobject_class_private, &mut class_ptr_buf)
                    .is_err()
                {
                    continue;
                }
                let class_ptr = u64::from_le_bytes(class_ptr_buf) as usize;
                if class_ptr == 0 {
                    continue;
                }
                let mut cname_buf = [0u8; 8];
                if ctx
                    .read_bytes(class_ptr + offsets.uobject_name_private, &mut cname_buf)
                    .is_err()
                {
                    continue;
                }
                let c_ci = u32::from_le_bytes(cname_buf[0..4].try_into().unwrap());
                let c_num = u32::from_le_bytes(cname_buf[4..8].try_into().unwrap());
                let Ok(class_name) = decode(c_ci, c_num) else {
                    continue;
                };
                if class_name != "Class" {
                    continue;
                }

                // Read ChildProperties from this UStruct.
                let mut child_buf = [0u8; 8];
                if ctx
                    .read_bytes(obj_ptr + offsets.ustruct_child_properties, &mut child_buf)
                    .is_err()
                {
                    continue;
                }
                let prop_ptr = u64::from_le_bytes(child_buf) as usize;
                if prop_ptr == 0 || !prop_ptr.is_multiple_of(8) || prop_ptr < 0x10000 {
                    continue;
                }

                found_prop_ptr = Some(prop_ptr);
                found_struct_name = name;
                break 'outer;
            }
        }

        let Some(prop_ptr) = found_prop_ptr else {
            warn!(
                "FProperty probe could not find a known UStruct with properties. Using default offsets."
            );
            return Ok(false);
        };

        debug!(
            "FProperty probe using UStruct '{}' first property at 0x{:X}",
            found_struct_name, prop_ptr
        );

        const VALID_SIZES: &[u32] = &[1, 2, 4, 8, 12, 16, 24, 32, 40, 48, 64, 96, 128, 256];
        let candidates = [
            (0x40usize, 0x34usize),
            (0x44, 0x38),
            (0x48, 0x3C),
            (0x4C, 0x40),
        ];

        for (off_internal, elem_size) in candidates {
            let mut off_buf = [0u8; 4];
            let mut size_buf = [0u8; 4];
            if ctx
                .read_bytes(prop_ptr + off_internal, &mut off_buf)
                .is_err()
            {
                continue;
            }
            if ctx.read_bytes(prop_ptr + elem_size, &mut size_buf).is_err() {
                continue;
            }
            let offset_value = u32::from_le_bytes(off_buf);
            let size_value = u32::from_le_bytes(size_buf);

            if offset_value > 0 && offset_value < 16384 && VALID_SIZES.contains(&size_value) {
                debug!(
                    "FProperty offsets accepted: internal=0x{:X} elem_size=0x{:X} (offset={}, size={})",
                    off_internal, elem_size, offset_value, size_value
                );
                offsets.fproperty_offset_internal = off_internal;
                offsets.fproperty_element_size = elem_size;
                return Ok(true);
            }
        }

        warn!("FProperty offset probe found no valid candidate; using defaults.");
        Ok(false)
    }
}
