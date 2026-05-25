//! In-memory log ring buffer shipped back to the host via
//! [`openforge_ue5_protocol::Request::DrainLog`].
//!
//! Self-contained — no `tracing` / `log` dependency. The DLL writes lines
//! synchronously; the host drains them lazily. Lines past `CAPACITY` evict the
//! oldest entry FIFO.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU8, Ordering};

use openforge_ue5_protocol::LogLevel;
use parking_lot::Mutex;

const CAPACITY: usize = 1024;

/// Encode a [`LogLevel`] as `u8` for storage in an [`AtomicU8`].
#[inline]
const fn level_to_u8(l: LogLevel) -> u8 {
    match l {
        LogLevel::Off => 0,
        LogLevel::Error => 1,
        LogLevel::Warn => 2,
        LogLevel::Info => 3,
        LogLevel::Debug => 4,
        LogLevel::Trace => 5,
    }
}

#[inline]
#[allow(dead_code)]
fn u8_to_level(v: u8) -> LogLevel {
    match v {
        0 => LogLevel::Off,
        1 => LogLevel::Error,
        2 => LogLevel::Warn,
        3 => LogLevel::Info,
        4 => LogLevel::Debug,
        _ => LogLevel::Trace,
    }
}

pub struct LogRing {
    lines: Mutex<VecDeque<String>>,
    level: AtomicU8,
}

impl LogRing {
    pub const fn new() -> Self {
        Self {
            lines: Mutex::new(VecDeque::new()),
            level: AtomicU8::new(level_to_u8(LogLevel::Info)),
        }
    }

    pub fn set_level(&self, level: LogLevel) {
        self.level.store(level_to_u8(level), Ordering::Relaxed);
    }

    #[allow(dead_code)]
    pub fn level(&self) -> LogLevel {
        u8_to_level(self.level.load(Ordering::Relaxed))
    }

    /// Returns true if `level` is enabled (i.e. `level <= configured`).
    #[inline]
    pub fn enabled(&self, level: LogLevel) -> bool {
        level_to_u8(level) <= self.level.load(Ordering::Relaxed)
            && self.level.load(Ordering::Relaxed) != 0
    }

    pub fn push(&self, level: LogLevel, line: String) {
        if !self.enabled(level) {
            return;
        }
        let prefix = match level {
            LogLevel::Off => return,
            LogLevel::Error => "ERROR",
            LogLevel::Warn => "WARN ",
            LogLevel::Info => "INFO ",
            LogLevel::Debug => "DEBUG",
            LogLevel::Trace => "TRACE",
        };
        let mut g = self.lines.lock();
        if g.len() >= CAPACITY {
            g.pop_front();
        }
        g.push_back(format!("{prefix} {line}"));
    }

    pub fn drain(&self, max_lines: usize) -> Vec<String> {
        let mut g = self.lines.lock();
        let n = max_lines.min(g.len());
        g.drain(..n).collect()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.lines.lock().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eviction_keeps_capacity() {
        let r = LogRing::new();
        r.set_level(LogLevel::Trace);
        for i in 0..(CAPACITY + 10) {
            r.push(LogLevel::Info, format!("line {i}"));
        }
        assert_eq!(r.len(), CAPACITY);
        // The oldest 10 should have been evicted.
        let drained = r.drain(5);
        assert_eq!(drained.len(), 5);
        assert!(drained[0].contains("line 10"));
    }

    #[test]
    fn level_gates_writes() {
        let r = LogRing::new();
        r.set_level(LogLevel::Warn);
        r.push(LogLevel::Debug, "should drop".into());
        r.push(LogLevel::Error, "kept".into());
        assert_eq!(r.len(), 1);
        let lines = r.drain(10);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("kept"));
    }

    #[test]
    fn off_drops_everything() {
        let r = LogRing::new();
        r.set_level(LogLevel::Off);
        r.push(LogLevel::Error, "x".into());
        assert_eq!(r.len(), 0);
    }
}
