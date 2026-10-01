//! RuffleVita: an optional virtual clock for repeatable desktop timedemos.
//! When enabled, `getTimer()` follows the time passed to `Player::tick`
//! instead of the wall clock, and `Math.random` starts from a fixed seed.
//! Off unless a frontend calls `enable`.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static ENABLED: AtomicBool = AtomicBool::new(false);
/// Milliseconds since the start, as f64 bits.
static NOW: AtomicU64 = AtomicU64::new(0);

pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
}

pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

pub(crate) fn advance(dt_ms: f64) {
    if is_enabled() {
        let now = f64::from_bits(NOW.load(Ordering::Relaxed)) + dt_ms;
        NOW.store(now.to_bits(), Ordering::Relaxed);
    }
}

/// Virtual milliseconds since start, when enabled.
pub(crate) fn now_ms() -> Option<u32> {
    is_enabled().then(|| f64::from_bits(NOW.load(Ordering::Relaxed)) as u32)
}
