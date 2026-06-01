//! FProperty + UFunction walkers. Mirrors `crates/discover/src/ue5/walker.rs`.

use std::sync::Arc;

use openforge_dll_common::local_reader::ReadError;
use openforge_ue5_protocol::{PropInfo, UFunctionInfo};

use crate::engine::UeEngine;

const FUNC_NATIVE: u32 = 0x0000_0400;

/// FProperty::PropertyFlags offset for the LotDK build. Cross-validated
/// against the UE4SS log dump (line 293: `FProperty::PropertyFlags =
/// 0x38`). Other UE5 titles using the stock layout typically place this at
/// 0x40; future per-build configs should surface as a UeOffsets field
/// rather than hardcoding here.
const FPROPERTY_PROPERTY_FLAGS_OFFSET: usize = 0x38;

impl UeEngine {
    /// Walk FProperty chain of `class_ptr` and its supers.
    pub fn walk_properties(&self, class_ptr: usize) -> Result<Vec<PropInfo>, ReadError> {
        let mut out: Vec<PropInfo> = Vec::new();
        let mut cur_class = class_ptr;
        // Bound the super chain to defend against cycles in pathological reads.
        for _ in 0..64 {
            if cur_class == 0 {
                break;
            }
            let class_name = self
                .get_object_name(cur_class)
                .unwrap_or_else(|_| "UnknownClass".to_string());

            let mut prop_ptr = match self
                .reader
                .read_ptr(cur_class + self.offsets.ustruct_child_properties as usize)
            {
                Ok(v) => v,
                Err(_) => break,
            };

            let mut prop_hops = 0usize;
            while prop_ptr != 0 && prop_hops < 8192 {
                prop_hops += 1;

                // FField NamePrivate
                let prop_name = match self
                    .read_fname_at(prop_ptr + self.offsets.ffield_name_private as usize)
                {
                    Ok((ci, num)) => self
                        .decode_fname(ci, num)
                        .unwrap_or_else(|_| "UnknownProp".to_string()),
                    Err(_) => "UnknownProp".to_string(),
                };

                // FFieldClass
                let kind = match self
                    .reader
                    .read_ptr(prop_ptr + self.offsets.ffield_class_private as usize)
                {
                    Ok(fc) if fc != 0 => match self.read_fname_at(fc) {
                        Ok((ci, num)) => self
                            .decode_fname(ci, num)
                            .unwrap_or_else(|_| "UnknownType".to_string()),
                        Err(_) => "UnknownType".to_string(),
                    },
                    _ => "UnknownType".to_string(),
                };

                let offset = self
                    .reader
                    .read_u32(prop_ptr + self.offsets.fproperty_offset_internal as usize)
                    .unwrap_or(0);
                let size = self
                    .reader
                    .read_u32(prop_ptr + self.offsets.fproperty_element_size as usize)
                    .unwrap_or(0);

                out.push(PropInfo {
                    name: prop_name,
                    offset,
                    size,
                    kind,
                    defined_in_class: class_name.clone(),
                });

                let next = match self
                    .reader
                    .read_ptr(prop_ptr + self.offsets.ffield_next as usize)
                {
                    Ok(v) => v,
                    Err(_) => break,
                };
                if next == prop_ptr {
                    break; // self-loop guard
                }
                prop_ptr = next;
            }

            cur_class = match self
                .reader
                .read_ptr(cur_class + self.offsets.ustruct_super_struct as usize)
            {
                Ok(v) => v,
                Err(_) => break,
            };
        }
        Ok(out)
    }

    /// Walk UFunction chain of `class_ptr` and its supers.
    pub fn walk_functions(&self, class_ptr: usize) -> Result<Vec<UFunctionInfo>, ReadError> {
        let mut out: Vec<UFunctionInfo> = Vec::new();
        let mut cur_class = class_ptr;
        for _ in 0..64 {
            if cur_class == 0 {
                break;
            }
            let class_name = self
                .get_object_name(cur_class)
                .unwrap_or_else(|_| "UnknownClass".to_string());

            let mut field_ptr = match self
                .reader
                .read_ptr(cur_class + self.offsets.ustruct_children as usize)
            {
                Ok(v) => v,
                Err(_) => break,
            };

            let mut field_hops = 0usize;
            while field_ptr != 0 && field_hops < 8192 {
                field_hops += 1;

                if let Ok(field_class_ptr) = self.get_object_class(field_ptr) {
                    let field_class_name =
                        self.get_object_name(field_class_ptr).unwrap_or_default();
                    if field_class_name == "Function" {
                        let name = self
                            .get_object_name(field_ptr)
                            .unwrap_or_else(|_| "UnknownFunc".to_string());
                        let native_func_ptr = self
                            .reader
                            .read_ptr(field_ptr + self.offsets.ufunction_func as usize)
                            .unwrap_or(0);
                        let flags = self
                            .reader
                            .read_u32(field_ptr + self.offsets.ufunction_flags as usize)
                            .unwrap_or(0);
                        let is_native = (flags & FUNC_NATIVE) != 0;
                        out.push(UFunctionInfo {
                            name,
                            addr: field_ptr as u64,
                            native_func_ptr: native_func_ptr as u64,
                            is_native,
                            defined_in_class: class_name.clone(),
                        });
                    }
                }

                let next = match self
                    .reader
                    .read_ptr(field_ptr + self.offsets.ufield_next as usize)
                {
                    Ok(v) => v,
                    Err(_) => break,
                };
                if next == field_ptr {
                    break;
                }
                field_ptr = next;
            }

            cur_class = match self
                .reader
                .read_ptr(cur_class + self.offsets.ustruct_super_struct as usize)
            {
                Ok(v) => v,
                Err(_) => break,
            };
        }
        Ok(out)
    }

    /// Cached property walk. Returns an `Arc` so repeated requests share data.
    pub fn walk_properties_cached(&self, class_ptr: u64) -> Result<Arc<Vec<PropInfo>>, ReadError> {
        if let Some(hit) = self.prop_cache.lock().get(&class_ptr) {
            return Ok(hit.clone());
        }
        let v = Arc::new(self.walk_properties(class_ptr as usize)?);
        let mut g = self.prop_cache.lock();
        Ok(g.entry(class_ptr).or_insert_with(|| v.clone()).clone())
    }

    /// Cached function walk.
    pub fn walk_functions_cached(
        &self,
        class_ptr: u64,
    ) -> Result<Arc<Vec<UFunctionInfo>>, ReadError> {
        if let Some(hit) = self.func_cache.lock().get(&class_ptr) {
            return Ok(hit.clone());
        }
        let v = Arc::new(self.walk_functions(class_ptr as usize)?);
        let mut g = self.func_cache.lock();
        Ok(g.entry(class_ptr).or_insert_with(|| v.clone()).clone())
    }

    /// Walk a UFunction's parameter chain (UFunction is a UStruct, so this
    /// reuses the FProperty walk over its `ChildProperties` list), reading
    /// the FProperty `PropertyFlags` for each entry. Used by the Lua
    /// metatable generator to drive arg marshalling — needs the flag bits
    /// to distinguish IN / OUT / RETURN parameters.
    ///
    /// Unlike [`Self::walk_properties`], this DOES NOT walk the super
    /// chain: a UFunction's parameter list lives entirely on the function
    /// itself.
    pub fn walk_function_params(
        &self,
        ufunction_ptr: usize,
    ) -> Result<Vec<(PropInfo, u64)>, ReadError> {
        let mut out: Vec<(PropInfo, u64)> = Vec::new();
        if ufunction_ptr == 0 {
            return Ok(out);
        }
        let class_name = self
            .get_object_name(ufunction_ptr)
            .unwrap_or_else(|_| "UnknownFunc".to_string());
        let mut prop_ptr = self
            .reader
            .read_ptr(ufunction_ptr + self.offsets.ustruct_child_properties as usize)?;
        let mut hops = 0usize;
        while prop_ptr != 0 && hops < 8192 {
            hops += 1;
            let prop_name =
                match self.read_fname_at(prop_ptr + self.offsets.ffield_name_private as usize) {
                    Ok((ci, num)) => self
                        .decode_fname(ci, num)
                        .unwrap_or_else(|_| "UnknownParam".to_string()),
                    Err(_) => "UnknownParam".to_string(),
                };
            let kind = match self
                .reader
                .read_ptr(prop_ptr + self.offsets.ffield_class_private as usize)
            {
                Ok(fc) if fc != 0 => match self.read_fname_at(fc) {
                    Ok((ci, num)) => self
                        .decode_fname(ci, num)
                        .unwrap_or_else(|_| "UnknownType".to_string()),
                    Err(_) => "UnknownType".to_string(),
                },
                _ => "UnknownType".to_string(),
            };
            let offset = self
                .reader
                .read_u32(prop_ptr + self.offsets.fproperty_offset_internal as usize)
                .unwrap_or(0);
            let size = self
                .reader
                .read_u32(prop_ptr + self.offsets.fproperty_element_size as usize)
                .unwrap_or(0);
            let flags = self
                .reader
                .read_u64(prop_ptr + FPROPERTY_PROPERTY_FLAGS_OFFSET)
                .unwrap_or(0);
            out.push((
                PropInfo {
                    name: prop_name,
                    offset,
                    size,
                    kind,
                    defined_in_class: class_name.clone(),
                },
                flags,
            ));
            let next = match self
                .reader
                .read_ptr(prop_ptr + self.offsets.ffield_next as usize)
            {
                Ok(v) => v,
                Err(_) => break,
            };
            if next == prop_ptr {
                break;
            }
            prop_ptr = next;
        }
        Ok(out)
    }
}
