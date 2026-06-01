//! UObject walking + FQN building via outer chain.
//!
//! Mirrors `crates/discover/src/ue5/objects.rs` but reads in-process.

use openforge_dll_common::local_reader::ReadError;
use openforge_ue5_protocol::UeObjectRef;

use crate::engine::UeEngine;

const CHUNK_SIZE: usize = 65536;

impl UeEngine {
    pub(crate) fn get_object_name(
        &self,
        obj_ptr: usize,
    ) -> Result<String, crate::names::NameError> {
        let (ci, num) = self.read_fname_at(obj_ptr + self.offsets.uobject_name_private as usize)?;
        self.decode_fname(ci, num)
    }

    pub(crate) fn get_object_class(&self, obj_ptr: usize) -> Result<usize, ReadError> {
        self.reader
            .read_ptr(obj_ptr + self.offsets.uobject_class_private as usize)
    }

    pub(crate) fn get_object_outer(&self, obj_ptr: usize) -> Result<usize, ReadError> {
        self.reader
            .read_ptr(obj_ptr + self.offsets.uobject_outer_private as usize)
    }

    pub(crate) fn build_fqn(&self, obj_ptr: usize) -> String {
        let mut parts: Vec<String> = Vec::new();
        let mut cur = obj_ptr;
        for _ in 0..64 {
            if cur == 0 {
                break;
            }
            match self.get_object_name(cur) {
                Ok(name) => parts.push(name),
                Err(_) => break,
            }
            match self.get_object_outer(cur) {
                Ok(o) => cur = o,
                Err(_) => break,
            }
        }
        parts.reverse();
        parts.join("/")
    }

    /// Walk the GUObjectArray and return every live UObject. `limit` caps the
    /// result count (0 = unlimited).
    pub fn walk_objects(&self, limit: usize) -> Result<Vec<UeObjectRef>, ReadError> {
        let mut header = [0u8; 32];
        self.reader.read_bytes(self.guobject_array, &mut header)?;
        let objects_ptr = u64::from_le_bytes(header[0..8].try_into().unwrap()) as usize;
        let num_elements = u32::from_le_bytes(header[20..24].try_into().unwrap()) as usize;
        let num_chunks = u32::from_le_bytes(header[28..32].try_into().unwrap()) as usize;
        if objects_ptr == 0 || num_elements == 0 || num_chunks == 0 {
            return Ok(Vec::new());
        }

        let cap = if limit == 0 {
            num_elements
        } else {
            limit.min(num_elements)
        };
        let mut out: Vec<UeObjectRef> = Vec::with_capacity(cap.min(1024));

        'outer: for chunk_idx in 0..num_chunks {
            let chunk_addr = match self.reader.read_ptr(objects_ptr + chunk_idx * 8) {
                Ok(v) => v,
                Err(_) => break,
            };
            if chunk_addr == 0 {
                continue;
            }
            for within in 0..CHUNK_SIZE {
                let index = chunk_idx * CHUNK_SIZE + within;
                if index >= num_elements {
                    break 'outer;
                }
                let item_addr = chunk_addr + within * self.fuobject_item_stride as usize;
                let obj_ptr = match self.reader.read_ptr(item_addr) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if obj_ptr == 0 || !obj_ptr.is_multiple_of(8) || obj_ptr < 0x10000 {
                    continue;
                }
                let Ok(_name) = self.get_object_name(obj_ptr) else {
                    continue;
                };
                let Ok(class_ptr) = self.get_object_class(obj_ptr) else {
                    continue;
                };
                let class_name = self
                    .get_object_name(class_ptr)
                    .unwrap_or_else(|_| "UnknownClass".to_string());
                let fqn = self.build_fqn(obj_ptr);
                out.push(UeObjectRef {
                    addr: obj_ptr as u64,
                    class_ptr: class_ptr as u64,
                    class_name,
                    fqn,
                });
                if limit != 0 && out.len() >= limit {
                    break 'outer;
                }
            }
        }
        Ok(out)
    }
}
