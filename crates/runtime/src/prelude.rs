//! Conveniences for game crates. `use openforge_runtime::prelude::*;`

pub use crate::feature::{
    DeclFeatureSrc, DeclarativeFeature, Feature, FeatureMeta, Tier, WriteStrategyKind,
};
pub use crate::game::{Game, GameMeta};
pub use crate::registry::{CustomFeatureEntry, GameEntry};
pub use crate::value::{Value, ValueKind, ValueRange};
pub use openforge_core::{Ctx, Pattern, Resolve, Target};
