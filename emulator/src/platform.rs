//! Everything that differs between the PS Vita and a desktop dev build:
//! storage locations, clocks, battery, time.

use std::path::PathBuf;

pub const SCREEN_W: u32 = 960;
pub const SCREEN_H: u32 = 544;

#[cfg(target_os = "vita")]
mod paths {
    pub const GAMES_DIR: &str = "ux0:data/FlashGames";
    pub const DATA_DIR: &str = "ux0:data/flashvita";
}

/// Where the user drops their `.swf` files.
pub fn games_dir() -> PathBuf {
    #[cfg(target_os = "vita")]
    {
        PathBuf::from(paths::GAMES_DIR)
    }
    #[cfg(not(target_os = "vita"))]
    {
        std::env::var_os("FLASHVITA_GAMES")
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
        std::env::var_os("FLASHVITA_DATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("flashvita-data"))
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
