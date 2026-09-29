//! Full-screen views and modal overlays.

pub mod explore;
pub mod keypicker;
pub mod library;
pub mod loading;
pub mod pause;
pub mod settings;

use std::collections::{HashMap, HashSet, VecDeque};

use crate::library::Library;
use crate::platform::SCREEN_W;
use crate::swfinfo::Image;
use crate::thumbs::ThumbKind;
use crate::ui::{self, FontId, Gfx, Rect, Texture, theme};
use crate::worker::{Job, Worker};

/// The two top-level screens, switched with L and R.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Library,
    Explore,
}

/// Wordmark, Library/Explore tabs, a note (e.g. "12 games") and the clock.
/// Returns the tab rectangles, for touch.
pub fn top_bar(g: &mut Gfx, active: Tab, note: &str) -> Vec<Rect> {
    let cy = 35.0;
    let w = ui::wordmark(g, 20.0, cy, 32.0);
    let names = ["Library", "Explore"];
    let rects = ui::tabs(g, 20.0 + w + 26.0, cy, &names, active as usize);
    if let Some(last) = rects.last() {
        let x = last.right() + 22.0 + ui::button_glyph_width(g, crate::input::Btn::R, 22.0) + 16.0;
        let max_w = SCREEN_W as f32 - 180.0 - x;
        let note = g.ellipsize(FontId::Bold, 14.0, note, max_w);
        g.text_mid(FontId::Bold, 14.0, x, cy, theme::ON_BLUE_DIM, &note);
    }
    ui::status(g, SCREEN_W as f32 - 22.0, cy);
    rects
}

/// Seconds since the app started, for animations.
pub fn time_secs() -> f32 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START.get_or_init(std::time::Instant::now).elapsed().as_secs_f32()
}

/// A thin ink scrollbar along the right edge of `area`.
pub fn scrollbar(g: &mut Gfx, area: Rect, scroll: f32, total: f32) {
    if total <= area.h {
        return;
    }
    let h = (area.h * area.h / total).max(30.0);
    let y = area.y + (scroll / (total - area.h)).clamp(0.0, 1.0) * (area.h - h);
    g.rounded(Rect::new(area.right() - 3.0, y, 4.0, h), 2.0, theme::INK.alpha(0.55));
}

/// GPU textures for cover thumbnails, loaded lazily for what's on screen.
pub struct ThumbCache {
    textures: HashMap<String, (Texture, ThumbKind)>,
    lru: VecDeque<String>,
    pending: HashSet<String>,
}

const MAX_THUMBS: usize = 40;

impl ThumbCache {
    pub fn new() -> Self {
        Self { textures: HashMap::new(), lru: VecDeque::new(), pending: HashSet::new() }
    }

    pub fn get(&self, key: &str) -> Option<&Texture> {
        self.textures.get(key).map(|(t, _)| t)
    }

    /// Asks the worker for a thumbnail the library says exists.
    pub fn want(&mut self, key: &str, lib: &Library, worker: &Worker) {
        if self.textures.contains_key(key) || self.pending.contains(key) {
            if let Some(pos) = self.lru.iter().position(|k| k == key) {
                let k = self.lru.remove(pos).unwrap();
                self.lru.push_back(k);
            }
            return;
        }
        let has = lib.games.iter().any(|g| g.key == key && g.db.has_thumb);
        if has {
            self.pending.insert(key.to_owned());
            worker.submit(Job::LoadThumb { key: key.to_owned() });
        }
    }

    pub fn insert(&mut self, gfx: &Gfx, key: &str, thumb: Option<(Image, ThumbKind)>) {
        self.pending.remove(key);
        let Some((img, kind)) = thumb else {
            self.textures.remove(key);
            return;
        };
        if let Some(tex) = gfx.create_texture(img.w, img.h, &img.rgba) {
            self.textures.insert(key.to_owned(), (tex, kind));
            self.lru.retain(|k| k != key);
            self.lru.push_back(key.to_owned());
            while self.lru.len() > MAX_THUMBS {
                if let Some(old) = self.lru.pop_front() {
                    self.textures.remove(&old);
                }
            }
        }
    }
}
