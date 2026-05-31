//! File-based logging for the DLL.
//!
//! Writes to `%TEMP%\openforge-glacier-<pid>.log`, one line per entry:
//! a monotonic-ish timestamp + level + message. The file is opened
//! append-only and `flush`'d after every write so a crash never loses the
//! last entry.
//!
//! Why a separate sink from [`crate::log_ring::LogRing`]? The ring requires a
//! connected client and a `DrainLog` request to surface entries; bring-up bugs
//! that prevent the handshake from completing leave the ring invisible. The
//! file log is always available and answers "what did the DLL do when the
//! client connected?" without needing a working pipe.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use windows::Win32::System::Threading::GetCurrentProcessId;

static FILE_LOG: OnceLock<Mutex<Option<std::fs::File>>> = OnceLock::new();

fn log_path() -> Option<PathBuf> {
    let mut p = PathBuf::from(std::env::var_os("TEMP")?);
    let pid = unsafe { GetCurrentProcessId() };
    p.push(format!("openforge-glacier-{pid}.log"));
    Some(p)
}

fn open_log() -> Option<std::fs::File> {
    let path = log_path()?;
    // Truncate on first open per process load so stale entries from a prior
    // load don't confuse triage.
    OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .ok()
}

pub fn init() {
    FILE_LOG.get_or_init(|| Mutex::new(open_log()));
}

/// Append one line to the log file. Never panics; failures are silent (we
/// don't want logging itself to take the DLL down).
pub fn log(level: &str, msg: &str) {
    let cell = FILE_LOG.get_or_init(|| Mutex::new(open_log()));
    let mut guard = cell.lock();
    if guard.is_none() {
        *guard = open_log();
    }
    let Some(f) = guard.as_mut() else {
        return;
    };
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    let _ = writeln!(f, "[{ts:.3}] {level} {msg}");
    let _ = f.flush();
}

#[macro_export]
macro_rules! flog {
    ($level:expr, $($arg:tt)*) => {{
        $crate::dll_log::log($level, &format!($($arg)*));
    }};
}
