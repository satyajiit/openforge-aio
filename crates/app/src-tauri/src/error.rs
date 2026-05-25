use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("not attached")]
    NotAttached,

    #[error("unknown game: {0}")]
    UnknownGame(String),

    #[error("unknown feature: {feature} (game {game})")]
    UnknownFeature { game: String, feature: String },

    #[error("openforge-core: {0}")]
    Core(#[from] openforge_core::Error),

    #[error("openforge-runtime: {0}")]
    Runtime(#[from] openforge_runtime::RuntimeError),

    #[error("openforge-ue5-host: {0}")]
    Host(#[from] openforge_ue5_host::HostError),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("persistence: {0}")]
    Persistence(String),

    #[error("win32: {0}")]
    Win32(String),

    #[error("{0}")]
    Other(String),
}

impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("AppError", 2)?;
        s.serialize_field("kind", self.kind())?;
        s.serialize_field("message", &self.to_string())?;
        s.end()
    }
}

impl AppError {
    pub fn kind(&self) -> &'static str {
        match self {
            AppError::NotAttached => "not_attached",
            AppError::UnknownGame(_) => "unknown_game",
            AppError::UnknownFeature { .. } => "unknown_feature",
            AppError::Core(_) => "core",
            AppError::Runtime(_) => "runtime",
            AppError::Host(_) => "host",
            AppError::Io(_) => "io",
            AppError::Json(_) => "json",
            AppError::Persistence(_) => "persistence",
            AppError::Win32(_) => "win32",
            AppError::Other(_) => "other",
        }
    }
}

pub type AppResult<T> = std::result::Result<T, AppError>;
