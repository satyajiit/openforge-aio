//! OpenForge runtime.
//!
//! Game + Feature traits, the inventory-based registry, the declarative
//! signature engine, and the macros game authors use to register their work.

pub use inventory;

pub mod error;
pub mod feature;
pub mod game;
pub mod manifest;
pub mod prelude;
pub mod registry;
pub mod signature;
pub mod value;

pub use crate::error::{RuntimeError, RuntimeResult};
pub use crate::feature::{
    DeclFeatureSrc, DeclarativeFeature, Feature, FeatureMeta, PreflightCheck, PreflightReport,
    Tier, WriteStrategyKind,
};
pub use crate::game::{Game, GameMeta};
pub use crate::manifest::{GameManifest, GameManifestBody, IconSpec};
pub use crate::registry::{CustomFeatureEntry, GameEntry, REGISTRY, Registry};
pub use crate::signature::{
    ControlSpec, Endian, HeapScanSpec, HeapValidatorSpec, HopSpec, LocatorSpec,
    Meta as SignatureMeta, PointerChainSpec, Preset, ResolveSpec, SignatureSpec, ValueSpec,
    WriteSpec,
};
pub use crate::value::{Value, ValueKind, ValueRange};
