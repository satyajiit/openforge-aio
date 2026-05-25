//! Bounded ring buffer for Lua `print()` / runtime-error captures.
//!
//! The DLL runs scripts fire-and-forget; the desktop trainer polls
//! `DrainLuaOutput` periodically to refresh the console. We can't let a
//! noisy `for i=1,1e6 do print(i) end` blow the DLL's heap, so output is
//! ring-buffered: when capacity is exhausted we drop the oldest lines and
//! stamp a single `Warn` line at the boundary noting how many were lost.
//!
//! Timestamps are monotonic milliseconds from a once-per-process start
//! instant — they're useful for ordering and rough latency but should not
//! be confused with wall-clock time.

use std::collections::VecDeque;
use std::time::Instant;

use once_cell::sync::Lazy;
use openforge_ue5_protocol::{LuaLogLevel, LuaOutputLine};
use parking_lot::Mutex;

/// Capacity of the in-memory ring. 4096 was chosen because:
///   * a typical "auto-cheat" script prints under 100 lines per minute, so
///     this is hours of headroom;
///   * a buggy `print` loop saturates the buffer in ~milliseconds and
///     stops; the host then sees the `dropped N lines` marker and can warn
///     the user without OOM-ing the game process.
pub const OUTPUT_CAPACITY: usize = 4096;

/// Process-wide start instant. All [`LuaOutputLine::timestamp_ms`] values
/// are computed as `START.elapsed().as_millis()` so the host can compare
/// timestamps across multiple drain calls.
static START: Lazy<Instant> = Lazy::new(Instant::now);

fn now_ms() -> u64 {
    START.elapsed().as_millis() as u64
}

/// Bounded MPMC-ish print sink. Internally a `Mutex<VecDeque>`; producers
/// (the Lua worker) push and the host drains. Locking is brief and
/// uncontended in practice.
pub struct OutputBuffer {
    inner: Mutex<Inner>,
}

struct Inner {
    lines: VecDeque<LuaOutputLine>,
    /// Count of lines dropped since the last successful push. Folded into
    /// a single `Warn` line the next time we successfully push, so the
    /// drop marker can't itself overflow.
    pending_drops: u64,
}

impl Default for OutputBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl OutputBuffer {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                lines: VecDeque::with_capacity(OUTPUT_CAPACITY),
                pending_drops: 0,
            }),
        }
    }

    /// Push an `Info` line (the default level for stock `print()`).
    pub fn push_info(&self, msg: impl Into<String>) {
        self.push(LuaLogLevel::Info, msg.into());
    }

    /// Push a `Warn` line. Used for the runtime's own diagnostics
    /// (registration warnings, etc.) — user scripts have no direct API to
    /// produce these in v1.
    #[allow(dead_code)]
    pub fn push_warn(&self, msg: impl Into<String>) {
        self.push(LuaLogLevel::Warn, msg.into());
    }

    /// Push an `Error` line. The runtime promotes any uncaught `error()`
    /// from a script to one of these.
    pub fn push_error(&self, msg: impl Into<String>) {
        self.push(LuaLogLevel::Error, msg.into());
    }

    fn push(&self, level: LuaLogLevel, message: String) {
        let mut inner = self.inner.lock();
        // If we previously dropped lines, fold those into a single marker
        // and push it BEFORE the new line so the boundary is visible in
        // the right order.
        if inner.pending_drops > 0 {
            let n = inner.pending_drops;
            inner.pending_drops = 0;
            Self::push_with_eviction(
                &mut inner,
                LuaOutputLine {
                    level: LuaLogLevel::Warn,
                    message: format!("<dropped {n} lines>"),
                    timestamp_ms: now_ms(),
                },
            );
        }
        Self::push_with_eviction(
            &mut inner,
            LuaOutputLine {
                level,
                message,
                timestamp_ms: now_ms(),
            },
        );
    }

    fn push_with_eviction(inner: &mut Inner, line: LuaOutputLine) {
        if inner.lines.len() >= OUTPUT_CAPACITY {
            // Drop the oldest line and remember we lost it. We accumulate
            // until the next push so a rapid-fire overflow only produces
            // one marker, not 4096.
            inner.lines.pop_front();
            inner.pending_drops = inner.pending_drops.saturating_add(1);
        }
        inner.lines.push_back(line);
    }

    /// Drain up to `max` oldest-first lines. Pass `usize::MAX` (or any
    /// value >= len) to drain everything.
    pub fn drain(&self, max: usize) -> Vec<LuaOutputLine> {
        let mut inner = self.inner.lock();
        let take = max.min(inner.lines.len());
        inner.lines.drain(..take).collect()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.inner.lock().lines.is_empty()
    }

    pub fn len(&self) -> usize {
        self.inner.lock().lines.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drain_returns_oldest_first() {
        let buf = OutputBuffer::new();
        buf.push_info("a");
        buf.push_info("b");
        buf.push_info("c");
        let lines = buf.drain(10);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].message, "a");
        assert_eq!(lines[2].message, "c");
        assert!(buf.is_empty());
    }

    #[test]
    fn overflow_drops_oldest_and_marks_boundary() {
        let buf = OutputBuffer::new();
        for i in 0..5000 {
            buf.push_info(format!("{i}"));
        }
        // Should hold exactly `OUTPUT_CAPACITY` lines: at the moment the
        // 4097th push occurs we evict one, then the next push folds the
        // pending-drop counter into a Warn marker. Net: capacity is
        // preserved.
        assert_eq!(buf.len(), OUTPUT_CAPACITY);
        let lines = buf.drain(usize::MAX);
        assert_eq!(lines.len(), OUTPUT_CAPACITY);
        // The first non-evicted line should NOT be "0" — earlier lines
        // were dropped — and somewhere we should see a `<dropped N lines>`
        // warn marker.
        assert_ne!(lines[0].message, "0");
        let marker = lines
            .iter()
            .find(|l| l.message.starts_with("<dropped "))
            .expect("expected a dropped-lines marker");
        assert!(matches!(marker.level, LuaLogLevel::Warn));
    }

    #[test]
    fn push_levels_round_trip() {
        let buf = OutputBuffer::new();
        buf.push_info("i");
        buf.push_warn("w");
        buf.push_error("e");
        let lines = buf.drain(10);
        assert!(matches!(lines[0].level, LuaLogLevel::Info));
        assert!(matches!(lines[1].level, LuaLogLevel::Warn));
        assert!(matches!(lines[2].level, LuaLogLevel::Error));
    }
}
