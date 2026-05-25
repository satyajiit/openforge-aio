//! Core Unreal Engine 5 reflection structures, types, and offsets.

pub mod emit;
pub mod locate;
pub mod names;
pub mod objects;
pub mod walker;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct UeOffsets {
    // UObject layout
    pub uobject_class_private: usize,
    pub uobject_name_private: usize,
    pub uobject_outer_private: usize,

    // UStruct / UClass layout
    pub ustruct_super_struct: usize,
    pub ustruct_child_properties: usize,
    pub ustruct_children: usize,

    // FField layout
    pub ffield_class_private: usize,
    pub ffield_next: usize,
    pub ffield_name_private: usize,

    // FProperty layout (inherits FField size)
    pub fproperty_offset_internal: usize,
    pub fproperty_element_size: usize,

    // UField layout
    pub ufield_next: usize,

    // UFunction layout (extends UStruct)
    pub ufunction_flags: usize,
    pub ufunction_func: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PropInfo {
    pub name: String,
    pub offset: u32,
    pub size: u32,
    pub kind: String,
    pub defined_in_class: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UFunctionInfo {
    pub name: String,
    pub addr: usize,
    pub native_func_ptr: usize,
    pub is_native: bool,
    pub defined_in_class: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UeObjectRef {
    pub addr: usize,
    pub class_ptr: usize,
    pub class_name: String,
    pub fqn: String,
}
