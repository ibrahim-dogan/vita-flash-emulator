//! RuffleVita: lenient null access. Offline, nothing a game requests from
//! the web arrives: portal APIs (Kongregate, MindJolt, ...), ad SDKs, score
//! servers, config files. Many games then call into the missing object
//! anyway (`kongregate.stats.submit(...)`), and the TypeError (#1009/#1010)
//! aborts the rest of the frame script or event handler, so a button never
//! gets its listener or a screen never advances.
//!
//! When reading a property, writing one or calling a method on null or
//! undefined would throw and nothing on the AVM2 call stack can catch it, the
//! operation is skipped instead (reads and calls give `undefined`) and the
//! code carries on. Code that has a `try` anywhere up the stack keeps Flash's
//! behaviour, so games that rely on catching these errors are unaffected.
//! A loop that only ends through such an error (`do x = x.parent while
//! (x != stage)` with `x` null) would spin instead, so after
//! `FRAME_BUDGET` skips in one frame the errors are thrown again until the
//! next frame. `RUFFLEVITA_STRICT_NULL=1` turns this off.

use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};

/// 0: not read yet, 1: lenient, 2: strict.
static MODE: AtomicU8 = AtomicU8::new(0);
static SKIPPED: AtomicU32 = AtomicU32::new(0);
static FRAME_SKIPS: AtomicU32 = AtomicU32::new(0);
const FRAME_BUDGET: u32 = 10_000;

/// Called at the start of every frame.
pub fn new_frame() {
    FRAME_SKIPS.store(0, Ordering::Relaxed);
}

/// Whether this access may be skipped: enabled and within this frame's budget.
pub fn allow() -> bool {
    if !enabled() {
        return false;
    }
    let n = FRAME_SKIPS.fetch_add(1, Ordering::Relaxed);
    if n == FRAME_BUDGET {
        tracing::warn!("{FRAME_BUDGET} null accesses skipped this frame; throwing them again until the next one");
    }
    n < FRAME_BUDGET
}

fn enabled() -> bool {
    match MODE.load(Ordering::Relaxed) {
        1 => true,
        2 => false,
        _ => {
            let strict = std::env::var_os("RUFFLEVITA_STRICT_NULL").is_some_and(|v| v != "0");
            MODE.store(if strict { 2 } else { 1 }, Ordering::Relaxed);
            !strict
        }
    }
}

/// Logs a skipped access: the first 20, then every 1000th, so a game that
/// hits one every frame doesn't flood the log.
pub fn note(describe: impl FnOnce() -> String) {
    let n = SKIPPED.fetch_add(1, Ordering::Relaxed) + 1;
    if n <= 20 || n % 1000 == 0 {
        tracing::warn!("Skipped null access #{n}: {}", describe());
    }
}
