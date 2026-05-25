use std::io;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("process not found: {0}")]
    ProcessNotFound(String),

    #[error("module not found: {0}")]
    ModuleNotFound(String),

    #[error("pattern has no non-wildcard bytes; cannot scan")]
    EmptyPattern,

    #[error("invalid pattern syntax: {0}")]
    BadPattern(String),

    #[error("pattern matched {0} times; expected exactly 1")]
    PatternAmbiguous(usize),

    #[error("pattern not found in module {0}")]
    PatternNotFound(String),

    #[error("address 0x{addr:X} is out of bounds for module {module}")]
    AddressOutOfBounds { addr: usize, module: String },

    #[error("read failed at 0x{addr:X} ({len} bytes): {source}")]
    ReadFailed {
        addr: usize,
        len: usize,
        #[source]
        source: io::Error,
    },

    #[error("write failed at 0x{addr:X} ({len} bytes): {source}")]
    WriteFailed {
        addr: usize,
        len: usize,
        #[source]
        source: io::Error,
    },

    #[error("win32 call {api} failed: {source}")]
    Win32 {
        api: &'static str,
        #[source]
        source: io::Error,
    },

    #[error("PE parsing failed for module {module}: {reason}")]
    PeParse { module: String, reason: String },

    #[error("{0}")]
    Custom(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    pub(crate) fn win32(api: &'static str) -> Self {
        Self::Win32 {
            api,
            source: io::Error::last_os_error(),
        }
    }
}
