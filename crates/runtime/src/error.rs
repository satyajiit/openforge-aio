use openforge_core::Error as CoreError;

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error(transparent)]
    Core(#[from] CoreError),

    #[error("signature TOML parse failed: {0}")]
    SignatureParse(String),

    #[error("signature for feature {feature:?} is invalid: {reason}")]
    SignatureInvalid { feature: String, reason: String },

    #[error("value kind mismatch: feature expects {expected:?}, got {got:?}")]
    KindMismatch { expected: String, got: String },

    #[error("value out of range: {value} not in [{min:?}, {max:?}]")]
    OutOfRange {
        value: f64,
        min: Option<f64>,
        max: Option<f64>,
    },

    #[error("duplicate game id registered: {0}")]
    DuplicateGame(String),

    #[error("duplicate feature id {feature:?} for game {game:?}")]
    DuplicateFeature { game: String, feature: String },
}

pub type RuntimeResult<T> = std::result::Result<T, RuntimeError>;
