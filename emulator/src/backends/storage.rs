// Based on ruffle_frontend_utils' DiskStorageBackend.
//
// Saves live in one folder per game, `saves/<Title> [hash]/<name>.sol`,
// where the hash comes from the game key. Ruffle names shared objects
// `<host>/<movie path>/<name>`, which on the Vita turned into deep
// `localhost/ux0_data/rufflevita/games/<file>.swf/` trees, and made every
// game that saved under `localPath "/"` share (and overwrite) one file.
// Saves in that old layout are moved over the first time a game reads them.
use ruffle_core::backend::storage::StorageBackend;
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::config::file_key;

pub struct DiskStorageBackend {
    root: PathBuf,
    game_dir: PathBuf,
    /// `<host>/<movie path>/`, the prefix Ruffle puts on this game's own
    /// shared objects.
    own_prefix: String,
    /// `<host>/`.
    host_prefix: String,
}

impl DiskStorageBackend {
    pub fn new(root: PathBuf, game_key: &str, title: &str, movie_url: &str) -> Self {
        let (host, movie_path) = movie_location(movie_url);
        let backend = DiskStorageBackend {
            game_dir: game_dir(&root, game_key, title),
            root,
            own_prefix: format!("{host}/{movie_path}/"),
            host_prefix: format!("{host}/"),
        };
        // Old builds could leave an empty folder for the game behind.
        backend.prune(backend.legacy_path(&backend.own_prefix).parent());
        backend
    }

    /// Verifies that the path contains no `..` components to prevent accessing files outside of the Ruffle directory.
    fn is_path_allowed(path: &Path) -> bool {
        path.components().all(|c| c != Component::ParentDir)
    }

    /// Where `name` lives inside the game's folder: the game's own objects
    /// at the top, ones saved under a parent `localPath` in `shared/`.
    fn relative_name(&self, name: &str) -> String {
        if let Some(rest) = name.strip_prefix(&self.own_prefix) {
            rest.to_owned()
        } else if let Some(rest) = name.strip_prefix(&self.host_prefix) {
            // `<localPath>/<name>`, where localPath is a parent folder of the
            // movie (possibly empty). Only the object name matters here.
            let own_dir = self.own_prefix[self.host_prefix.len()..].trim_end_matches('/');
            let mut so = rest;
            for (i, _) in own_dir.match_indices('/').chain([(own_dir.len(), "")]) {
                if let Some(r) = rest.strip_prefix(&own_dir[..i]).and_then(|r| r.strip_prefix('/')) {
                    so = r;
                }
            }
            if let Some(r) = rest.strip_prefix('/') {
                so = r;
            }
            format!("shared/{so}")
        } else {
            format!("other/{name}")
        }
    }

    fn get_shared_object_path(&self, name: &str) -> PathBuf {
        let rel: Vec<String> = self.relative_name(name).split('/').filter(|s| !s.is_empty()).map(safe_component).collect();
        self.game_dir.join(format!("{}.sol", rel.join("/")))
    }

    /// The pre-1.2 location of `name`.
    fn legacy_path(&self, name: &str) -> PathBuf {
        self.root.join(format!("{}.sol", name.split('/').map(safe_component).collect::<Vec<_>>().join("/")))
    }

    /// Brings a save over from the old layout. The game's own objects are
    /// moved; ones under a parent `localPath` are copied, since another
    /// game may read the same file.
    fn migrate(&self, name: &str, path: &Path) {
        let old = self.legacy_path(name);
        if !Self::is_path_allowed(&old) || !old.is_file() {
            return;
        }
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let own = name.starts_with(&self.own_prefix);
        let result = if own { fs::rename(&old, path) } else { fs::copy(&old, path).map(|_| ()) };
        match result {
            Ok(()) => {
                tracing::info!("{} save {} -> {}", if own { "Moved" } else { "Copied" }, old.display(), path.display());
                self.prune(old.parent());
            }
            Err(e) => tracing::warn!("Unable to migrate save {}: {e}", old.display()),
        }
    }

    /// Removes `dir` and its parents up to the saves folder while empty.
    fn prune(&self, mut dir: Option<&Path>) {
        while let Some(d) = dir.filter(|d| *d != self.root && d.starts_with(&self.root)) {
            if fs::remove_dir(d).is_err() {
                break;
            }
            dir = d.parent();
        }
    }
}

/// Host and path the way Ruffle's `SharedObject.getLocal` derives them.
fn movie_location(movie_url: &str) -> (String, String) {
    let Ok(url) = url::Url::parse(movie_url) else {
        return ("localhost".into(), String::new());
    };
    let mut path = url.path();
    path = path.strip_prefix('/').unwrap_or(path);
    path = path.strip_suffix('/').unwrap_or(path);
    let host = if url.scheme() == "file" {
        if let [_, b':', b'/', ..] = path.as_bytes() {
            path = &path[3..];
        }
        "localhost"
    } else {
        url.host_str().unwrap_or_default()
    };
    (host.to_owned(), path.to_owned())
}

/// The game's folder: an existing one with the same hash (so renaming the
/// game or its title keeps the saves), else `<Title> [hash]`.
fn game_dir(root: &Path, game_key: &str, title: &str) -> PathBuf {
    let hash = &file_key(game_key)[..8];
    let tag = format!("[{hash}]");
    let existing = fs::read_dir(root).ok().and_then(|entries| {
        entries
            .flatten()
            .find(|e| e.file_name().to_string_lossy().ends_with(&tag) && e.path().is_dir())
            .map(|e| e.path())
    });
    existing.unwrap_or_else(|| {
        let mut name: String = safe_component(title).chars().take(40).collect();
        name = name.trim_matches(|c: char| c == '.' || c.is_whitespace()).to_owned();
        if name.is_empty() {
            name = "Game".into();
        }
        root.join(format!("{name} {tag}"))
    })
}

/// FAT/exFAT reject ':' and friends.
fn safe_component(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\\' | '/' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect()
}

impl StorageBackend for DiskStorageBackend {
    fn get(&self, name: &str) -> Option<Vec<u8>> {
        let path = self.get_shared_object_path(name);
        if !Self::is_path_allowed(&path) {
            return None;
        }
        if !path.exists() {
            let tmp = path.with_extension("sol.tmp");
            if tmp.is_file() {
                let _ = fs::rename(&tmp, &path);
            } else {
                self.migrate(name, &path);
            }
        }
        match fs::read(&path) {
            Ok(data) => Some(data),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                tracing::warn!("Unable to read file \"{}\": {:?}", path.display(), e);
                None
            }
        }
    }

    fn put(&mut self, name: &str, value: &[u8]) -> bool {
        let path = self.get_shared_object_path(name);
        if !Self::is_path_allowed(&path) {
            return false;
        }
        if let Some(parent_dir) = path.parent() {
            if let Err(r) = fs::create_dir_all(parent_dir) {
                tracing::warn!("Unable to create storage dir {}", r);
                return false;
            }
        }
        // Temp file + rename, so a power-off mid-write keeps the old save
        // (or the new one in the temp file, picked up by `get`).
        let tmp = path.with_extension("sol.tmp");
        let result = fs::write(&tmp, value).and_then(|()| {
            // newlib's rename won't replace an existing file.
            let _ = fs::remove_file(&path);
            fs::rename(&tmp, &path)
        });
        match result {
            Ok(()) => true,
            Err(r) => {
                tracing::warn!("Unable to save file {}: {:?}", path.display(), r);
                let _ = fs::remove_file(&tmp);
                false
            }
        }
    }

    fn remove_key(&mut self, name: &str) {
        let path = self.get_shared_object_path(name);
        if !Self::is_path_allowed(&path) {
            return;
        }
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(self.legacy_path(name));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backend(root: &Path) -> DiskStorageBackend {
        DiskStorageBackend::new(root.to_owned(), "Raft Wars.swf", "Raft: Wars", "file:///ux0:data/rufflevita/games/Raft%20Wars.swf")
    }

    #[test]
    fn layout() {
        let root = std::env::temp_dir().join(format!("rv-saves-{}", std::process::id()));
        let b = backend(&root);
        let dir = root.join(format!("Raft_ Wars [{}]", &file_key("Raft Wars.swf")[..8]));
        let own = "localhost/ux0:data/rufflevita/games/Raft%20Wars.swf/save";
        assert_eq!(b.get_shared_object_path(own), dir.join("save.sol"));
        assert_eq!(b.get_shared_object_path("localhost//save"), dir.join("shared/save.sol"));
        assert_eq!(b.get_shared_object_path("localhost/ux0:data/rufflevita/games/save"), dir.join("shared/save.sol"));
        assert_eq!(b.get_shared_object_path("example.com/a/b"), dir.join("other/example.com/a/b.sol"));
    }

    #[test]
    fn migrates_old_saves() {
        let root = std::env::temp_dir().join(format!("rv-saves-mig-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let name = "localhost/ux0:data/rufflevita/games/Raft%20Wars.swf/save";
        let old = root.join("localhost/ux0_data/rufflevita/games/Raft%20Wars.swf/save.sol");
        fs::create_dir_all(old.parent().unwrap()).unwrap();
        fs::write(&old, b"data").unwrap();

        let mut b = backend(&root);
        assert_eq!(b.get(name).as_deref(), Some(&b"data"[..]));
        assert!(!old.exists());
        assert!(!root.join("localhost/ux0_data").exists());
        assert!(b.put(name, b"new"));
        let shared = root.join("localhost/guest_BTD5.sol");
        fs::create_dir_all(shared.parent().unwrap()).unwrap();
        fs::write(&shared, b"btd").unwrap();
        assert_eq!(b.get("localhost//guest_BTD5").as_deref(), Some(&b"btd"[..]));
        assert!(shared.exists());
        // A second session finds the same folder.
        assert_eq!(backend(&root).get(name).as_deref(), Some(&b"new"[..]));
        let _ = fs::remove_dir_all(&root);
    }
}
