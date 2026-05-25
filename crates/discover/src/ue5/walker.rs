//! Property (FProperty) and Function (UFunction) walkers across UClasses.

use anyhow::Result;
use openforge_core::Ctx;
use std::sync::Arc;

use crate::ue5::locate::UeEngine;
use crate::ue5::{PropInfo, UFunctionInfo};

// Engine reflection constants
const FUNC_NATIVE: u32 = 0x00000400;

impl UeEngine {
    /// Walk FProperties of a class, stepping up the super class chain to support inheritance.
    pub fn walk_properties(&self, class_ptr: usize) -> Result<Vec<PropInfo>> {
        let mut out = Vec::new();
        let mut curr_class = class_ptr;

        while curr_class != 0 {
            let class_name = if let Ok(name) = self.get_object_name(curr_class) {
                name
            } else {
                "UnknownClass".to_string()
            };

            let mut child_props_buf = [0u8; 8];
            if self
                .process
                .read_bytes(
                    curr_class + self.offsets.ustruct_child_properties,
                    &mut child_props_buf,
                )
                .is_err()
            {
                break;
            }
            let mut prop_ptr = u64::from_le_bytes(child_props_buf) as usize;

            while prop_ptr != 0 {
                // Read FField NamePrivate
                let (comparison_index, number) =
                    self.read_fname_at(prop_ptr + self.offsets.ffield_name_private)?;
                let prop_name = self
                    .decode_fname(comparison_index, number)
                    .unwrap_or_else(|_| "UnknownProp".to_string());

                // Read FFieldClass description pointer
                let mut field_class_buf = [0u8; 8];
                let prop_type = if self
                    .process
                    .read_bytes(
                        prop_ptr + self.offsets.ffield_class_private,
                        &mut field_class_buf,
                    )
                    .is_ok()
                {
                    let field_class = u64::from_le_bytes(field_class_buf) as usize;
                    // First 8 bytes of FFieldClass is a NamePrivate representing the class name
                    let (fc_comparison, fc_number) = self.read_fname_at(field_class)?;
                    self.decode_fname(fc_comparison, fc_number)
                        .unwrap_or_else(|_| "UnknownType".to_string())
                } else {
                    "UnknownType".to_string()
                };

                // Read FProperty offset and size
                let mut offset_buf = [0u8; 4];
                let offset = if self
                    .process
                    .read_bytes(
                        prop_ptr + self.offsets.fproperty_offset_internal,
                        &mut offset_buf,
                    )
                    .is_ok()
                {
                    u32::from_le_bytes(offset_buf)
                } else {
                    0
                };

                let mut size_buf = [0u8; 4];
                let size = if self
                    .process
                    .read_bytes(
                        prop_ptr + self.offsets.fproperty_element_size,
                        &mut size_buf,
                    )
                    .is_ok()
                {
                    u32::from_le_bytes(size_buf)
                } else {
                    0
                };

                out.push(PropInfo {
                    name: prop_name,
                    offset,
                    size,
                    kind: prop_type,
                    defined_in_class: class_name.clone(),
                });

                // Next pointer in FField linked list
                let mut next_buf = [0u8; 8];
                if self
                    .process
                    .read_bytes(prop_ptr + self.offsets.ffield_next, &mut next_buf)
                    .is_err()
                {
                    break;
                }
                prop_ptr = u64::from_le_bytes(next_buf) as usize;
            }

            // Move to SuperStruct class to inherit parent properties
            let mut super_buf = [0u8; 8];
            if self
                .process
                .read_bytes(
                    curr_class + self.offsets.ustruct_super_struct,
                    &mut super_buf,
                )
                .is_err()
            {
                break;
            }
            curr_class = u64::from_le_bytes(super_buf) as usize;
        }

        Ok(out)
    }

    /// Walk UFunctions of a class, including native and Blueprint elements.
    pub fn walk_functions(&self, class_ptr: usize) -> Result<Vec<UFunctionInfo>> {
        let mut out = Vec::new();
        let mut curr_class = class_ptr;

        while curr_class != 0 {
            let class_name = self
                .get_object_name(curr_class)
                .unwrap_or_else(|_| "UnknownClass".to_string());

            // Children points to a UField linked list containing both properties (UE4) and functions (UE4 & UE5)
            let mut children_buf = [0u8; 8];
            if self
                .process
                .read_bytes(
                    curr_class + self.offsets.ustruct_children,
                    &mut children_buf,
                )
                .is_err()
            {
                break;
            }
            let mut field_ptr = u64::from_le_bytes(children_buf) as usize;

            while field_ptr != 0 {
                // Check if this child field is a UFunction
                // In UE, the first 8 bytes of UObject (which UField is) after vtable is flags, internal index, then UClass pointer.
                // Let's grab the class pointer and decode it.
                if let Ok(field_class_ptr) = self.get_object_class(field_ptr) {
                    let field_class_name =
                        self.get_object_name(field_class_ptr).unwrap_or_default();

                    if field_class_name == "Function" {
                        let name = self
                            .get_object_name(field_ptr)
                            .unwrap_or_else(|_| "UnknownFunc".to_string());

                        // Read native entry point Func
                        let mut func_buf = [0u8; 8];
                        let native_func_ptr = if self
                            .process
                            .read_bytes(field_ptr + self.offsets.ufunction_func, &mut func_buf)
                            .is_ok()
                        {
                            u64::from_le_bytes(func_buf) as usize
                        } else {
                            0
                        };

                        // Read flags to see if native or Blueprint
                        let mut flags_buf = [0u8; 4];
                        let flags = if self
                            .process
                            .read_bytes(field_ptr + self.offsets.ufunction_flags, &mut flags_buf)
                            .is_ok()
                        {
                            u32::from_le_bytes(flags_buf)
                        } else {
                            0
                        };

                        let is_native = (flags & FUNC_NATIVE) != 0;

                        out.push(UFunctionInfo {
                            name,
                            addr: field_ptr,
                            native_func_ptr,
                            is_native,
                            defined_in_class: class_name.clone(),
                        });
                    }
                }

                // Next pointer in UField linked list
                let mut next_buf = [0u8; 8];
                if self
                    .process
                    .read_bytes(field_ptr + self.offsets.ufield_next, &mut next_buf)
                    .is_err()
                {
                    break;
                }
                field_ptr = u64::from_le_bytes(next_buf) as usize;
            }

            // Move to Super class
            let mut super_buf = [0u8; 8];
            if self
                .process
                .read_bytes(
                    curr_class + self.offsets.ustruct_super_struct,
                    &mut super_buf,
                )
                .is_err()
            {
                break;
            }
            curr_class = u64::from_le_bytes(super_buf) as usize;
        }

        Ok(out)
    }

    /// Cached variant of [`Self::walk_properties`]. Returns an `Arc` so callers
    /// can share without re-walking the class hierarchy.
    pub fn walk_properties_cached(&self, class_ptr: usize) -> Result<Arc<Vec<PropInfo>>> {
        {
            let cache = self.prop_cache.lock().unwrap();
            if let Some(hit) = cache.get(&class_ptr) {
                return Ok(hit.clone());
            }
        }
        let props = Arc::new(self.walk_properties(class_ptr)?);
        let mut cache = self.prop_cache.lock().unwrap();
        cache.entry(class_ptr).or_insert_with(|| props.clone());
        Ok(props)
    }

    /// Cached variant of [`Self::walk_functions`].
    pub fn walk_functions_cached(&self, class_ptr: usize) -> Result<Arc<Vec<UFunctionInfo>>> {
        {
            let cache = self.func_cache.lock().unwrap();
            if let Some(hit) = cache.get(&class_ptr) {
                return Ok(hit.clone());
            }
        }
        let funcs = Arc::new(self.walk_functions(class_ptr)?);
        let mut cache = self.func_cache.lock().unwrap();
        cache.entry(class_ptr).or_insert_with(|| funcs.clone());
        Ok(funcs)
    }
}
