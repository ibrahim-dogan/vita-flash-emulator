//! Full-screen views and modal overlays.

pub mod keypicker;
pub mod library;
pub mod loading;
pub mod pause;
pub mod settings;

use std::collections::{HashMap, HashSet, VecDeque};

use crate::library::Library;
use crate::swfinfo::Image;
use crate::thumbs::ThumbKind;
use crate::ui::{Gfx, Texture};
use crate::worker::{Job, Worker};

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
