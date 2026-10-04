//! The game library: what's on the memory card, what we know about each
//! file, and a small database (play history, cached analysis) persisted
//! between runs so startup never re-parses unchanged files.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{self, Sort};
use crate::platform;
use crate::swfinfo::SwfInfo;
use crate::worker::{Job, Worker};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DbEntry {
    pub size: u64,
    pub mtime: i64,
    pub info: Option<SwfInfo>,
    pub error: Option<String>,
    /// Cover extraction was attempted for this size/mtime.
    pub cover_checked: bool,
    pub has_thumb: bool,
    /// The thumbnail was captured in-game (never replaced automatically).
    pub screenshot: bool,
    pub last_played: i64,
    pub play_count: u32,
}

pub struct Game {
    /// Path relative to the games directory, with '/' separators. Also the
    /// key for profiles, thumbnails and the database.
    pub key: String,
    pub path: PathBuf,
    pub name: String,
    pub folder: Option<String>,
    pub db: DbEntry,
}

impl Game {
    pub fn title(&self) -> &str {
        self.db
            .info
            .as_ref()
            .and_then(|i| i.title.as_deref())
            .filter(|t| !is_generic_title(t))
            .unwrap_or(&self.name)
    }
}

/// Metadata titles that tools fill in by default ("Adobe Flex 4
/// Application", "Untitled-1") or that name the publisher or its site
/// ("Easy Street Games", "spilgames.com") say nothing about the game; the
/// file name is better.
fn is_generic_title(title: &str) -> bool {
    let t = title.trim().to_lowercase();
    const EXACT: [&str; 12] = [
        "flash", "flash movie", "flash game", "main", "preloader", "document", "movie", "game", "app",
        "application", "swf", "title",
    ];
    EXACT.contains(&t.as_str())
        || t.starts_with("adobe flex")
        || t.starts_with("adobe flash")
        || t.starts_with("flex ")
        || t.starts_with("macromedia")
        || t.strip_prefix("untitled").is_some_and(|r| r.chars().all(|c| !c.is_alphabetic()))
        || [".com", ".net", ".org", "www.", "http", "|"].iter().any(|p| t.contains(p))
        || [" games", " studio", " studios", " entertainment", " interactive"].iter().any(|p| t.ends_with(p))
}

pub struct Library {
    pub games: Vec<Game>,
    db: BTreeMap<String, DbEntry>,
    dirty: bool,
}

fn db_path() -> PathBuf {
    platform::data_dir().join("library.ron")
}

fn pretty_name(stem: &str) -> String {
    let s = stem.replace(['_', '+'], " ");
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.is_empty() { stem.to_owned() } else { s }
}

fn mtime(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn scan_dir(root: &Path, dir: &Path, depth: u32, out: &mut Vec<(String, PathBuf, std::fs::Metadata)>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = std::fs::metadata(&path) else { continue };
        let file_name = entry.file_name().to_string_lossy().into_owned();
        if file_name.starts_with('.') {
            continue;
        }
        if meta.is_dir() {
            if depth < 2 {
                scan_dir(root, &path, depth + 1, out);
            }
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("swf"))
        {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            out.push((rel, path, meta));
        }
    }
}

impl Library {
    pub fn load() -> Self {
        let db = config::read_ron(&db_path()).unwrap_or_default();
        let mut lib = Library { games: Vec::new(), db, dirty: false };
        lib.rescan();
        lib
    }

    /// Re-reads the games directory (cheap: directory listing + stat only).
    pub fn rescan(&mut self) {
        let root = platform::games_dir();
        let mut found = Vec::new();
        scan_dir(&root, &root, 0, &mut found);

        let mut games = Vec::with_capacity(found.len());
        for (key, path, meta) in found {
            let size = meta.len();
            let mt = mtime(&meta);
            let mut db = self.db.get(&key).cloned().unwrap_or_default();
            if db.size != size || db.mtime != mt {
                // The file changed: forget derived data, keep history.
                db = DbEntry {
                    size,
                    mtime: mt,
                    has_thumb: db.has_thumb && crate::thumbs::path(&key).exists(),
                    screenshot: db.screenshot,
                    last_played: db.last_played,
                    play_count: db.play_count,
                    ..Default::default()
                };
                self.dirty = true;
            }
            let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let folder = key.rsplit_once('/').map(|(f, _)| f.to_owned());
            games.push(Game { name: pretty_name(&stem), folder, key, path, db });
        }
        // Drop database rows for files that are gone.
        let keys: HashSet<&str> = games.iter().map(|g| g.key.as_str()).collect();
        let before = self.db.len();
        self.db.retain(|k, _| keys.contains(k.as_str()));
        self.dirty |= self.db.len() != before;
        self.games = games;
        for g in &self.games {
            self.db.insert(g.key.clone(), g.db.clone());
        }
    }

    /// Queues analysis for games we know nothing about yet.
    pub fn queue_analysis(&self, worker: &Worker) {
        // Recently played first: those are the covers people look at.
        let mut order: Vec<&Game> = self.games.iter().collect();
        order.sort_by_key(|g| std::cmp::Reverse(g.db.last_played));
        for g in order {
            let want_info = g.db.info.is_none() && g.db.error.is_none();
            let want_cover = !g.db.has_thumb && !g.db.cover_checked && g.db.error.is_none();
            if want_info || want_cover {
                worker.submit(Job::Analyze {
                    key: g.key.clone(),
                    path: g.path.clone(),
                    want_info,
                    want_cover,
                });
            }
        }
    }

    pub fn sort(&mut self, sort: Sort) {
        match sort {
            Sort::Name => self.games.sort_by_cached_key(|g| g.title().to_lowercase()),
            Sort::Recent => self.games.sort_by_cached_key(|g| {
                (std::cmp::Reverse(g.db.last_played), g.title().to_lowercase())
            }),
        }
    }

    pub fn index_of(&self, key: &str) -> Option<usize> {
        self.games.iter().position(|g| g.key == key)
    }

    pub fn update(&mut self, key: &str, f: impl FnOnce(&mut DbEntry)) {
        if let Some(g) = self.games.iter_mut().find(|g| g.key == key) {
            f(&mut g.db);
            self.db.insert(key.to_owned(), g.db.clone());
            self.dirty = true;
        }
    }

    pub fn save_if_dirty(&mut self) {
        if self.dirty {
            config::write_ron(&db_path(), &self.db);
            self.dirty = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_generic_title;

    #[test]
    fn generic_titles() {
        for t in ["Adobe Flex 4 Application", "Adobe Flex 3 Application", "Untitled-1", "untitled", "Main", " Preloader ", "zlonggames.com|spilgames.com", "Easy Street Games"] {
            assert!(is_generic_title(t), "{t}");
        }
        for t in ["Raft Wars", "Untitled Goose", "Mainframe Defenders", "Flashback"] {
            assert!(!is_generic_title(t), "{t}");
        }
    }
}
