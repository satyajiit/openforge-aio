//! Walker to enumerate live UObjects and generate their Fully Qualified Names (FQN).

use anyhow::Result;
use openforge_core::Ctx;

use crate::ue5::UeObjectRef;
use crate::ue5::locate::UeEngine;

impl UeEngine {
    pub fn get_object_name(&self, obj_ptr: usize) -> Result<String> {
        let (comparison_index, number) =
            self.read_fname_at(obj_ptr + self.offsets.uobject_name_private)?;
        self.decode_fname(comparison_index, number)
    }

    pub fn get_object_class(&self, obj_ptr: usize) -> Result<usize> {
        let mut buf = [0u8; 8];
        self.process
            .read_bytes(obj_ptr + self.offsets.uobject_class_private, &mut buf)?;
        Ok(u64::from_le_bytes(buf) as usize)
    }

    pub fn get_object_outer(&self, obj_ptr: usize) -> Result<usize> {
        let mut buf = [0u8; 8];
        self.process
            .read_bytes(obj_ptr + self.offsets.uobject_outer_private, &mut buf)?;
        Ok(u64::from_le_bytes(buf) as usize)
    }

    pub fn get_fqn(&self, obj_ptr: usize) -> String {
        let mut parts = Vec::new();
        let mut curr = obj_ptr;

        // Max depth to prevent infinite loops in cyclic references
        for _ in 0..64 {
            if curr == 0 {
                break;
            }
            if let Ok(name) = self.get_object_name(curr) {
                parts.push(name);
            } else {
                break;
            }
            if let Ok(outer) = self.get_object_outer(curr) {
                curr = outer;
            } else {
                break;
            }
        }

        parts.reverse();
        parts.join("/")
    }

    pub fn walk_objects(&self) -> Result<Vec<UeObjectRef>> {
        // Read ChunkedFixedUObjectArray layout
        let mut header = [0u8; 32];
        self.process.read_bytes(self.guobject_array, &mut header)?;

        let objects_ptr = u64::from_le_bytes(header[0..8].try_into().unwrap()) as usize;
        let num_elements = u32::from_le_bytes(header[20..24].try_into().unwrap()) as usize;
        let num_chunks = u32::from_le_bytes(header[28..32].try_into().unwrap()) as usize;

        if objects_ptr == 0 || num_elements == 0 || num_chunks == 0 {
            return Ok(Vec::new());
        }

        let mut out = Vec::new();
        let chunk_size = 65536;

        for chunk_idx in 0..num_chunks {
            let mut chunk_ptr_buf = [0u8; 8];
            if self
                .process
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
                    break;
                }

                let item_addr = chunk_addr + within_chunk * self.fuobject_item_stride;
                let mut item_buf = [0u8; 8];
                if self.process.read_bytes(item_addr, &mut item_buf).is_err() {
                    continue;
                }

                let obj_ptr = u64::from_le_bytes(item_buf) as usize;
                if obj_ptr == 0 {
                    continue;
                }

                // Check alignment and minimal address sanity
                if !obj_ptr.is_multiple_of(8) || obj_ptr < 0x10000 {
                    continue;
                }

                if self.get_object_name(obj_ptr).is_ok()
                    && let Ok(class_ptr) = self.get_object_class(obj_ptr)
                {
                    let class_name = self
                        .get_object_name(class_ptr)
                        .unwrap_or_else(|_| "UnknownClass".to_string());
                    let fqn = self.get_fqn(obj_ptr);
                    out.push(UeObjectRef {
                        addr: obj_ptr,
                        class_ptr,
                        class_name,
                        fqn,
                    });
                }
            }
        }

        Ok(out)
    }
}
