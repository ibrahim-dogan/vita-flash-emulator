//! Everything that differs between the PS Vita and a desktop dev build:
//! storage locations, clocks, battery, time.

use std::path::PathBuf;
#[cfg(target_os = "vita")]
use std::path::Path;

pub const SCREEN_W: u32 = 960;
pub const SCREEN_H: u32 = 544;

#[cfg(target_os = "vita")]
mod paths {
    pub const GAMES_DIR: &str = "ux0:data/FlashGames";
    pub const DATA_DIR: &str = "ux0:data/rufflevita";
    /// Where versions before 1.0.1, released as "FlashVita", kept their data.
    pub const OLD_DATA_DIR: &str = "ux0:data/flashvita";
}

/// Where the user drops their `.swf` files.
pub fn games_dir() -> PathBuf {
    #[cfg(target_os = "vita")]
    {
        PathBuf::from(paths::GAMES_DIR)
    }
    #[cfg(not(target_os = "vita"))]
    {
        std::env::var_os("RUFFLEVITA_GAMES")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("games"))
    }
}

/// Human readable form of [`games_dir`] for on-screen instructions.
pub fn games_dir_display() -> String {
    let mut s = games_dir().to_string_lossy().into_owned();
    if !s.ends_with('/') {
        s.push('/');
    }
    s
}

/// Settings, profiles, thumbnails, saves and logs.
pub fn data_dir() -> PathBuf {
    #[cfg(target_os = "vita")]
    {
        PathBuf::from(paths::DATA_DIR)
    }
    #[cfg(not(target_os = "vita"))]
    {
        std::env::var_os("RUFFLEVITA_DATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("rufflevita-data"))
    }
}

pub fn thumbs_dir() -> PathBuf {
    data_dir().join("thumbs")
}

pub fn profiles_dir() -> PathBuf {
    data_dir().join("profiles")
}

pub fn saves_dir() -> PathBuf {
    data_dir().join("saves")
}

/// On the first start after the rename from FlashVita, copies the settings,
/// profiles, game saves and covers over from the old data folder, which is
/// left untouched. Returns what happened, for the log.
pub fn migrate_old_data() -> Option<String> {
    #[cfg(target_os = "vita")]
    {
        let (old, new) = (PathBuf::from(paths::OLD_DATA_DIR), data_dir());
        // `settings.ron` tells our old folder apart from another app's.
        if new.exists() || !old.join("settings.ron").exists() {
            return None;
        }
        Some(match copy_dir(&old, &new) {
            Ok(files) => format!("Copied {files} files from {} to {}", old.display(), new.display()),
            Err(e) => format!("Couldn't copy {} to {}: {e}", old.display(), new.display()),
        })
    }
    #[cfg(not(target_os = "vita"))]
    {
        None
    }
}

#[cfg(target_os = "vita")]
fn copy_dir(from: &Path, to: &Path) -> std::io::Result<usize> {
    std::fs::create_dir_all(to)?;
    let mut files = 0;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            files += copy_dir(&entry.path(), &target)?;
        } else if entry.file_name() != "log.txt" {
            std::fs::copy(entry.path(), &target)?;
            files += 1;
        }
    }
    Ok(files)
}

pub fn ensure_dirs() {
    for dir in [games_dir(), data_dir(), thumbs_dir(), profiles_dir(), saves_dir()] {
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!("Couldn't create {}: {e}", dir.display());
        }
    }
}

/// Seconds since the Unix epoch.
pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Raises clocks and pins the main thread to its own core.
///
/// Stock clocks are 333 MHz CPU / 111 MHz GPU; Ruffle's interpreter is CPU
/// bound and vector fills are fill-rate bound, so both matter a lot.
pub fn init_hardware() {
    #[cfg(target_os = "vita")]
    unsafe {
        use vitasdk_sys::*;
        scePowerSetArmClockFrequency(444);
        scePowerSetBusClockFrequency(222);
        scePowerSetGpuClockFrequency(222);
        scePowerSetGpuXbarClockFrequency(166);

        let id = sceKernelGetThreadId();
        sceKernelChangeThreadPriority(id, SCE_KERNEL_PROCESS_PRIORITY_USER_HIGH as _);
        sceKernelChangeThreadCpuAffinityMask(id, SCE_KERNEL_CPU_MASK_USER_1 as _);
    }
}

/// Development automation: `autorun.txt` in the data folder holds
/// `KEY=VALUE` lines (`RUFFLEVITA_AUTOSTART`, `RUFFLEVITA_SCRIPT`,
/// `RUFFLEVITA_BENCH`, ...) that are set as environment variables at startup,
/// which is how the Vita gets them. The file is used once: it's renamed to
/// `autorun.last`, so the next normal launch is a normal launch. Returns the
/// keys that were set, for the log.
pub fn apply_autorun() -> Vec<String> {
    let path = data_dir().join("autorun.txt");
    let Ok(text) = std::fs::read_to_string(&path) else { return Vec::new() };
    let _ = std::fs::rename(&path, data_dir().join("autorun.last"));
    let mut keys = Vec::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        if let Some((key, value)) = line.split_once('=') {
            // SAFETY: called first thing in `run`, before any other thread exists.
            unsafe { std::env::set_var(key.trim(), value.trim()) };
            keys.push(key.trim().to_owned());
        }
    }
    keys
}

/// Asks macOS to keep the main thread on the performance cores even though
/// the app has no visible window (otherwise it counts as background work).
#[cfg(not(target_os = "vita"))]
pub fn keep_foreground_priority() {
    #[cfg(target_os = "macos")]
    unsafe {
        const QOS_CLASS_USER_INTERACTIVE: u32 = 0x21;
        unsafe extern "C" {
            fn pthread_set_qos_class_self_np(qos: u32, relative_priority: i32) -> i32;
        }
        pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE, 0);
    }
}

/// Runs on `rv_prof`'s sampling thread: the main thread is pinned to core 1
/// at high priority, so the sampler needs another core and a priority that
/// lets it wake up on time.
pub fn profiler_thread_setup() {
    #[cfg(target_os = "vita")]
    unsafe {
        use vitasdk_sys::*;
        let id = sceKernelGetThreadId();
        sceKernelChangeThreadPriority(id, SCE_KERNEL_PROCESS_PRIORITY_USER_HIGH as _);
        sceKernelChangeThreadCpuAffinityMask(id, SCE_KERNEL_CPU_MASK_USER_0 as _);
    }
}

/// Battery charge in percent and whether it is charging, if the device has one.
pub fn battery() -> Option<(u8, bool)> {
    #[cfg(target_os = "vita")]
    unsafe {
        let pct = vitasdk_sys::scePowerGetBatteryLifePercent();
        if pct < 0 {
            return None;
        }
        let charging = vitasdk_sys::scePowerIsBatteryCharging() != 0;
        Some((pct.clamp(0, 100) as u8, charging))
    }
    #[cfg(not(target_os = "vita"))]
    {
        None
    }
}

/// Local wall-clock time as (hour, minute).
pub fn local_time() -> (u32, u32) {
    #[cfg(target_os = "vita")]
    unsafe {
        let mut t: vitasdk_sys::SceDateTime = std::mem::zeroed();
        vitasdk_sys::sceRtcGetCurrentClockLocalTime(&mut t);
        (t.hour as u32, t.minute as u32)
    }
    #[cfg(not(target_os = "vita"))]
    {
        use chrono::Timelike;
        let now = chrono::Local::now();
        (now.hour(), now.minute())
    }
}

/// Free GPU memory and used heap in MiB, for the performance overlay:
/// "VRAM x · RAM x · PHY x free · heap x/240".
pub fn memory_summary() -> Option<String> {
    #[cfg(target_os = "vita")]
    unsafe {
        #[repr(C)]
        struct Mallinfo {
            arena: usize,
            ordblks: usize,
            smblks: usize,
            hblks: usize,
            hblkhd: usize,
            usmblks: usize,
            fsmblks: usize,
            uordblks: usize,
            fordblks: usize,
            keepcost: usize,
        }
        unsafe extern "C" {
            fn vglMemFree(kind: i32) -> usize;
            fn mallinfo() -> Mallinfo;
        }
        const MIB: usize = 1024 * 1024;
        let heap = mallinfo().uordblks / MIB;
        let total = crate::NEWLIB_HEAP_SIZE_USER as usize / MIB;
        Some(format!(
            "VRAM {} \u{b7} RAM {} \u{b7} PHY {} free \u{b7} heap {heap}/{total} MB",
            vglMemFree(0) / MIB,
            vglMemFree(1) / MIB,
            vglMemFree(2) / MIB,
        ))
    }
    #[cfg(all(not(target_os = "vita"), feature = "heapcount"))]
    {
        let (now, peak) = heapcount::mb();
        Some(format!("heap {now}/{peak} MB (peak)"))
    }
    #[cfg(all(not(target_os = "vita"), not(feature = "heapcount")))]
    {
        None
    }
}

/// Development (`heapcount` feature): counts the Rust heap, so desktop runs
/// show what a game would need of the Vita's heap (desktop RSS also counts
/// the GPU driver).
#[cfg(feature = "heapcount")]
pub mod heapcount {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NOW: AtomicUsize = AtomicUsize::new(0);
    static PEAK: AtomicUsize = AtomicUsize::new(0);
    static LIMIT: AtomicUsize = AtomicUsize::new(usize::MAX);

    /// Makes allocations fail above `mb`, like the Vita's heap does.
    pub fn set_limit_mb(mb: usize) {
        LIMIT.store(mb << 20, Ordering::Relaxed);
    }

    pub struct Counting;

    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            if NOW.load(Ordering::Relaxed) + layout.size() > LIMIT.load(Ordering::Relaxed) {
                return std::ptr::null_mut();
            }
            let p = unsafe { System.alloc(layout) };
            if !p.is_null() {
                let now = NOW.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
                PEAK.fetch_max(now, Ordering::Relaxed);
            }
            p
        }
        unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
            unsafe { System.dealloc(p, layout) };
            NOW.fetch_sub(layout.size(), Ordering::Relaxed);
        }
        unsafe fn realloc(&self, p: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            if new_size > layout.size() && NOW.load(Ordering::Relaxed) + new_size - layout.size() > LIMIT.load(Ordering::Relaxed) {
                return std::ptr::null_mut();
            }
            let q = unsafe { System.realloc(p, layout, new_size) };
            if !q.is_null() {
                NOW.fetch_sub(layout.size(), Ordering::Relaxed);
                let now = NOW.fetch_add(new_size, Ordering::Relaxed) + new_size;
                PEAK.fetch_max(now, Ordering::Relaxed);
            }
            q
        }
    }

    /// (current, peak) in MB.
    pub fn mb() -> (usize, usize) {
        (NOW.load(Ordering::Relaxed) >> 20, PEAK.load(Ordering::Relaxed) >> 20)
    }
}

/// Free GPU memory in bytes, across vitaGL's pools (textures fall back from
/// one to the next).
#[cfg(target_os = "vita")]
pub fn gpu_free() -> usize {
    unsafe extern "C" {
        fn vglMemFree(kind: i32) -> usize;
    }
    unsafe { vglMemFree(0) + vglMemFree(1) + vglMemFree(2) }
}
