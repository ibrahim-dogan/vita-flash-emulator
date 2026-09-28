//! Per-game profiles (bindings + display options) and global settings,
//! persisted as RON under the data directory.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::input::Btn;
use crate::keys;
use crate::platform;

/// What pressing a Vita button does in-game.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub enum Action {
    None,
    Key(u16),
    MouseLeft,
    /// Opens FlashVita's pause menu.
    Menu,
}

impl Action {
    pub fn label(self) -> String {
        match self {
            Action::None => "\u{2014}".into(),
            Action::Key(k) => keys::display_name(k),
            Action::MouseLeft => "Mouse click".into(),
            Action::Menu => "FlashVita menu".into(),
        }
    }
}

impl From<Action> for String {
    fn from(a: Action) -> String {
        match a {
            Action::None => "none".into(),
            Action::Key(k) => format!("key:{}", keys::get(k).id),
            Action::MouseLeft => "mouse_left".into(),
            Action::Menu => "menu".into(),
        }
    }
}

impl TryFrom<String> for Action {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        match s.as_str() {
            "none" => Ok(Action::None),
            "mouse_left" => Ok(Action::MouseLeft),
            "menu" => Ok(Action::Menu),
            other => other
                .strip_prefix("key:")
                .and_then(keys::find)
                .map(Action::Key)
                .ok_or_else(|| format!("unknown action {other}")),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StickMode {
    Off,
    Arrows,
    Wasd,
    Mouse,
}

impl StickMode {
    pub const ALL: [StickMode; 4] = [StickMode::Arrows, StickMode::Wasd, StickMode::Mouse, StickMode::Off];
    pub fn label(self) -> &'static str {
        match self {
            StickMode::Off => "Off",
            StickMode::Arrows => "Arrow keys",
            StickMode::Wasd => "WASD",
            StickMode::Mouse => "Mouse cursor",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScaleMode {
    /// Keep aspect ratio, letterbox (always, even for NoScale movies).
    Fit,
    /// Fill the screen, distorting the aspect ratio.
    Stretch,
    /// Fill the screen, cropping the edges.
    Zoom,
    /// Whatever the movie asks for.
    Native,
}

impl ScaleMode {
    pub const ALL: [ScaleMode; 4] = [ScaleMode::Fit, ScaleMode::Stretch, ScaleMode::Zoom, ScaleMode::Native];
    pub fn label(self) -> &'static str {
        match self {
            ScaleMode::Fit => "Fit to screen",
            ScaleMode::Stretch => "Stretch",
            ScaleMode::Zoom => "Zoom (crop)",
            ScaleMode::Native => "Movie default",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Quality {
    Low,
    Medium,
    High,
}

impl Quality {
    pub const ALL: [Quality; 3] = [Quality::Low, Quality::Medium, Quality::High];
    pub fn label(self) -> &'static str {
        match self {
            Quality::Low => "Low (fastest)",
            Quality::Medium => "Medium",
            Quality::High => "High",
        }
    }
    pub fn stage_quality(self) -> ruffle_render::quality::StageQuality {
        use ruffle_render::quality::StageQuality;
        match self {
            Quality::Low => StageQuality::Low,
            Quality::Medium => StageQuality::Medium,
            Quality::High => StageQuality::High,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub buttons: BTreeMap<Btn, Action>,
    pub left_stick: StickMode,
    pub right_stick: StickMode,
    pub scale: ScaleMode,
    pub quality: Quality,
    pub show_fps: bool,
    /// Virtual cursor speed, 1..=10.
    pub cursor_speed: u8,
    /// Use the rear touchpad as a trackpad for the mouse cursor.
    pub rear_touch: bool,
}

impl Default for Profile {
    fn default() -> Self {
        let mut buttons = BTreeMap::new();
        let k = |id| Action::Key(keys::key(id));
        buttons.insert(Btn::Up, k("Up"));
        buttons.insert(Btn::Down, k("Down"));
        buttons.insert(Btn::Left, k("Left"));
        buttons.insert(Btn::Right, k("Right"));
        buttons.insert(Btn::Cross, k("Space"));
        buttons.insert(Btn::Circle, k("X"));
        buttons.insert(Btn::Square, k("Z"));
        buttons.insert(Btn::Triangle, k("C"));
        buttons.insert(Btn::L, k("Shift"));
        buttons.insert(Btn::R, Action::MouseLeft);
        buttons.insert(Btn::Start, k("Enter"));
        buttons.insert(Btn::Select, Action::Menu);
        Self {
            buttons,
            left_stick: StickMode::Arrows,
            right_stick: StickMode::Mouse,
            scale: ScaleMode::Fit,
            quality: Quality::High,
            show_fps: false,
            cursor_speed: 5,
            rear_touch: false,
        }
    }
}

impl Profile {
    pub fn action(&self, btn: Btn) -> Action {
        self.buttons.get(&btn).copied().unwrap_or(Action::None)
    }

    pub fn set_action(&mut self, btn: Btn, action: Action) {
        self.buttons.insert(btn, action);
    }

    /// One-line summary for the library's detail panel.
    pub fn summary(&self) -> String {
        let move_keys = match (self.action(Btn::Up), self.left_stick) {
            (Action::Key(k), _) if keys::get(k).id == "W" => "WASD",
            (Action::Key(k), _) if keys::get(k).id == "Up" => "Arrows",
            (_, StickMode::Wasd) => "WASD",
            _ => "Custom",
        };
        format!(
            "{move_keys} \u{00b7} Cross: {} \u{00b7} Circle: {}",
            self.action(Btn::Cross).label(),
            self.action(Btn::Circle).label()
        )
    }
}

/// Library ordering.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Sort {
    Recent,
    Name,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub sort: Sort,
    pub last_game: Option<String>,
    pub vsync: bool,
    /// Template for games that have no profile of their own yet.
    pub default_profile: Profile,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            sort: Sort::Recent,
            last_game: None,
            vsync: true,
            default_profile: Profile::default(),
        }
    }
}

fn settings_path() -> PathBuf {
    platform::data_dir().join("settings.ron")
}

/// Stable, filesystem-safe file stem for a game key (its path relative to
/// the games directory).
pub fn file_key(game_key: &str) -> String {
    // FNV-1a keeps names short and avoids characters FAT/exFAT rejects.
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in game_key.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn profile_path(game_key: &str) -> PathBuf {
    platform::profiles_dir().join(format!("{}.ron", file_key(game_key)))
}

pub fn read_ron<T: for<'de> Deserialize<'de>>(path: &std::path::Path) -> Option<T> {
    let text = std::fs::read_to_string(path).ok()?;
    match ron::from_str(&text) {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::warn!("Ignoring unreadable {}: {e}", path.display());
            None
        }
    }
}

/// Writes atomically (temp file + rename) so a crash never leaves a
/// half-written config behind.
pub fn write_ron<T: Serialize>(path: &std::path::Path, value: &T) {
    let text = match ron::ser::to_string_pretty(value, ron::ser::PrettyConfig::default()) {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("Couldn't serialise {}: {e}", path.display());
            return;
        }
    };
    let tmp = path.with_extension("tmp");
    let result = std::fs::write(&tmp, text).and_then(|_| {
        // newlib's rename won't replace an existing file.
        let _ = std::fs::remove_file(path);
        std::fs::rename(&tmp, path)
    });
    if let Err(e) = result {
        tracing::error!("Couldn't write {}: {e}", path.display());
    }
}

impl Settings {
    pub fn load() -> Self {
        read_ron(&settings_path()).unwrap_or_default()
    }

    pub fn save(&self) {
        write_ron(&settings_path(), self);
    }

    pub fn load_profile(&self, game_key: &str) -> Profile {
        read_ron(&profile_path(game_key)).unwrap_or_else(|| self.default_profile.clone())
    }

    pub fn save_profile(&self, game_key: &str, profile: &Profile) {
        write_ron(&profile_path(game_key), profile);
    }
}
