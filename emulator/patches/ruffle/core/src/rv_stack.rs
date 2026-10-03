//! RuffleVita: a guard for the native stack. Ruffle has no limit on AVM2
//! recursion, so a game that recurses too deeply overflowed the native stack,
//! which kills the whole app (the Vita's main thread has 4 MiB). Flash throws
//! "Error #1023: Stack overflow occurred." instead, which games can catch.

use std::sync::atomic::{AtomicUsize, Ordering};

/// Lowest stack address calls may start from (0: no guard).
static LIMIT: AtomicUsize = AtomicUsize::new(0);

/// Room left for what runs after the check (natives, error construction).
const RESERVE: usize = 512 * 1024;

#[inline(always)]
fn stack_pointer() -> usize {
    let marker = 0u8;
    std::ptr::addr_of!(marker) as usize
}

/// Called on the thread that runs the player, near the top of its stack of
/// `size` bytes.
pub fn set_stack_size(size: usize) {
    let limit = stack_pointer().saturating_sub(size.saturating_sub(RESERVE));
    LIMIT.store(limit, Ordering::Relaxed);
}

/// Whether the stack is too close to its end for another script call.
#[inline(always)]
pub fn exhausted() -> bool {
    stack_pointer() < LIMIT.load(Ordering::Relaxed)
}
