//! Local-file navigator: lets games load sibling files (external SWFs, XML,
//! sounds) from the folder they live in. Vita paths like `ux0:data/...`
//! aren't Unix paths, so URLs are mapped to paths by hand rather than via
//! `Url::to_file_path`.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_channel::{Receiver, Sender};
use encoding_rs::Encoding;
use indexmap::IndexMap;
use ruffle_core::backend::navigator::{
    ErrorResponse, NavigationMethod, NavigatorBackend, NullExecutor, NullSpawner, OwnedFuture,
    Request, SuccessResponse,
};
use ruffle_core::loader::Error;
use ruffle_core::socket::{ConnectionState, SocketAction, SocketHandle};
use url::{ParseError, Url};

/// `file:///` URL for a local path, e.g. `ux0:data/FlashGames/a.swf` ->
/// `file:///ux0:data/FlashGames/a.swf`.
pub fn path_to_url(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    let s = s.trim_start_matches('/');
    let mut url = Url::parse("file:///").unwrap();
    url.set_path(s);
    url.to_string()
}

fn url_to_path(url: &Url) -> Option<PathBuf> {
    if url.scheme() != "file" {
        return None;
    }
    let decoded = percent_decode(url.path());
    #[cfg(target_os = "vita")]
    let p = decoded.trim_start_matches('/').to_owned();
    #[cfg(not(target_os = "vita"))]
    let p = if cfg!(windows) { decoded.trim_start_matches('/').to_owned() } else { decoded };
    Some(PathBuf::from(p))
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// File names the game asked for but that aren't on the memory card.
pub type MissingFiles = std::sync::Arc<std::sync::Mutex<Vec<String>>>;

pub struct LocalNavigator {
    spawner: NullSpawner,
    base_url: Url,
    /// Folder of the root movie.
    base_dir: Option<PathBuf>,
    missing: MissingFiles,
}

impl LocalNavigator {
    /// `movie_url` is the root movie's URL; relative loads resolve against it.
    pub fn new(executor: &NullExecutor, movie_url: &str, missing: MissingFiles) -> Self {
        let base_url = Url::parse(movie_url).unwrap_or_else(|_| Url::parse("file:///").unwrap());
        let base_dir = url_to_path(&base_url).and_then(|p| p.parent().map(Path::to_path_buf));
        Self { spawner: executor.spawner(), base_url, base_dir, missing }
    }

    /// Where a URL lives locally. Web URLs (portal loaders fetching the real
    /// game from their long-dead servers) map into the game's own folder,
    /// first by path, then by bare file name.
    fn local_path(&self, url: &Url) -> Option<PathBuf> {
        if url.scheme() == "file" {
            return url_to_path(url);
        }
        let dir = self.base_dir.as_ref()?;
        let rel = percent_decode(url.path().trim_start_matches('/'));
        let by_path = dir.join(&rel);
        if by_path.is_file() {
            return Some(by_path);
        }
        let name = rel.rsplit('/').next().filter(|n| !n.is_empty())?;
        Some(dir.join(name))
    }
}

struct LocalResponse {
    url: String,
    path: PathBuf,
    file: Option<std::fs::File>,
}

impl SuccessResponse for LocalResponse {
    fn url(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.url)
    }

    fn body(self: Box<Self>) -> OwnedFuture<Vec<u8>, Error> {
        let result = std::fs::read(&self.path).map_err(|e| Error::FetchError(e.to_string()));
        Box::pin(async move { result })
    }

    fn text_encoding(&self) -> Option<&'static Encoding> {
        None
    }

    fn status(&self) -> u16 {
        0
    }

    fn redirected(&self) -> bool {
        false
    }

    fn next_chunk(&mut self) -> OwnedFuture<Option<Vec<u8>>, Error> {
        use std::io::Read;
        if self.file.is_none() {
            match std::fs::File::open(&self.path) {
                Ok(f) => self.file = Some(f),
                Err(e) => {
                    let e = Error::FetchError(e.to_string());
                    return Box::pin(async move { Err(e) });
                }
            }
        }
        let mut buf = vec![0; 64 * 1024];
        let res = self.file.as_mut().unwrap().read(&mut buf);
        Box::pin(async move {
            match res {
                Ok(0) => Ok(None),
                Ok(n) => {
                    buf.truncate(n);
                    Ok(Some(buf))
                }
                Err(e) => Err(Error::FetchError(e.to_string())),
            }
        })
    }

    fn expected_length(&self) -> Result<Option<u64>, Error> {
        Ok(std::fs::metadata(&self.path).ok().map(|m| m.len()))
    }
}

impl NavigatorBackend for LocalNavigator {
    fn navigate_to_url(
        &self,
        url: &str,
        _target: &str,
        _vars_method: Option<(NavigationMethod, IndexMap<String, String>)>,
    ) {
        tracing::info!("Game tried to open {url} (no browser on this device)");
    }

    fn fetch(&self, request: Request) -> OwnedFuture<Box<dyn SuccessResponse>, ErrorResponse> {
        let url_str = request.url().to_owned();
        let result = match self.resolve_url(&url_str) {
            Ok(url) => {
                let mut fs_url = url.clone();
                fs_url.set_query(None);
                fs_url.set_fragment(None);
                match self.local_path(&fs_url) {
                    Some(path) if path.is_file() => {
                        if url.scheme() != "file" {
                            tracing::info!("Serving {url} from {}", path.display());
                        }
                        Ok(Box::new(LocalResponse { url: url.to_string(), path, file: None })
                            as Box<dyn SuccessResponse>)
                    }
                    Some(path) => {
                        if let Some(name) = path.file_name() {
                            self.missing.lock().unwrap().push(name.to_string_lossy().into_owned());
                        }
                        Err(ErrorResponse {
                            url: url.to_string(),
                            error: Error::FetchError(format!("{} not found", path.display())),
                        })
                    }
                    None => Err(ErrorResponse {
                        url: url.to_string(),
                        error: Error::FetchError("Network access is not available".into()),
                    }),
                }
            }
            Err(e) => Err(ErrorResponse { url: url_str, error: Error::FetchError(e.to_string()) }),
        };
        if let Err(e) = &result {
            tracing::warn!("Fetch failed for {}: {}", e.url, e.error);
        }
        Box::pin(async move { result })
    }

    fn resolve_url(&self, url: &str) -> Result<Url, ParseError> {
        match Url::parse(url) {
            Ok(u) => Ok(u),
            Err(ParseError::RelativeUrlWithoutBase) => self.base_url.join(url),
            Err(e) => Err(e),
        }
    }

    fn spawn_future(&mut self, future: OwnedFuture<(), Error>) {
        self.spawner.spawn_local(future);
    }

    fn pre_process_url(&self, url: Url) -> Url {
        url
    }

    fn connect_socket(
        &mut self,
        _host: String,
        _port: u16,
        _timeout: Duration,
        handle: SocketHandle,
        _receiver: Receiver<Vec<u8>>,
        sender: Sender<SocketAction>,
    ) {
        let _ = sender.try_send(SocketAction::Connect(handle, ConnectionState::Failed));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vita_style_paths_round_trip() {
        let url = path_to_url(Path::new("ux0:data/FlashGames/My Game.swf"));
        assert_eq!(url, "file:///ux0:data/FlashGames/My%20Game.swf");
        let joined = Url::parse(&url).unwrap().join("levels/l2.swf").unwrap();
        assert_eq!(joined.as_str(), "file:///ux0:data/FlashGames/levels/l2.swf");
        let decoded = percent_decode(joined.path());
        assert_eq!(decoded, "/ux0:data/FlashGames/levels/l2.swf");
        assert_eq!(percent_decode("/a%20b%2"), "/a b%2");
    }
}
