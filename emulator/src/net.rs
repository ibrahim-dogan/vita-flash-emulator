//! A small blocking HTTP/1.1 client, enough for Explore: plain `http://`
//! (the Vita can't validate today's TLS certificates), GET with an optional
//! byte range, sized, chunked or gzip bodies, and redirects that stay on
//! http. The Vita's newlib starts its network stack on the first socket.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub const USER_AGENT: &str = concat!("RuffleVita/", env!("CARGO_PKG_VERSION"), " (PlayStation Vita)");

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const READ_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_REDIRECTS: usize = 3;

#[derive(Debug)]
pub enum NetError {
    /// DNS failed or there's no route: usually Wi-Fi is off.
    Offline(String),
    Timeout,
    /// The server answered with this status.
    Status(u16),
    Cancelled,
    Other(String),
}

impl std::fmt::Display for NetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NetError::Offline(host) => write!(f, "Couldn't reach {host}. Check that Wi-Fi is on."),
            NetError::Timeout => f.write_str("The server stopped responding. Try again."),
            NetError::Status(404) => f.write_str("The server doesn't have this file any more."),
            NetError::Status(code) => write!(f, "The server answered with error {code}."),
            NetError::Cancelled => f.write_str("Cancelled."),
            NetError::Other(msg) => f.write_str(msg),
        }
    }
}

impl From<std::io::Error> for NetError {
    fn from(e: std::io::Error) -> Self {
        match e.kind() {
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => NetError::Timeout,
            _ => NetError::Other(format!("Network error: {e}")),
        }
    }
}

pub struct Request<'a> {
    pub url: &'a str,
    /// Inclusive byte range to ask for.
    pub range: Option<(u64, u64)>,
    /// Ask for (and transparently inflate) a gzip body.
    pub gzip: bool,
    pub cancel: Option<&'a AtomicBool>,
}

impl<'a> Request<'a> {
    pub fn get(url: &'a str) -> Self {
        Request { url, range: None, gzip: false, cancel: None }
    }
}

/// What a successful response said about its body.
pub struct Head {
    /// Body length, when known (after range, before gzip).
    pub length: Option<u64>,
    /// The whole file's size, from `Content-Range`.
    pub total: Option<u64>,
}

struct Url<'a> {
    host: &'a str,
    port: u16,
    path: &'a str,
}

fn parse_url(url: &str) -> Result<Url<'_>, NetError> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| NetError::Other(format!("Only http:// addresses are supported: {url}")))?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h, p.parse().map_err(|_| NetError::Other(format!("Bad address: {url}")))?),
        None => (authority, 80),
    };
    Ok(Url { host, port, path })
}

fn cancelled(req: &Request) -> bool {
    req.cancel.is_some_and(|c| c.load(Ordering::Relaxed))
}

/// Fetches `req.url`, passing the body to `on_data` in chunks as it
/// arrives. Fails on any status other than 200/206.
pub fn get(req: &Request, mut on_data: impl FnMut(&Head, &[u8]) -> Result<(), NetError>) -> Result<Head, NetError> {
    let mut url = req.url.to_owned();
    for _ in 0..=MAX_REDIRECTS {
        match get_once(req, &url, &mut on_data)? {
            Outcome::Done(head) => return Ok(head),
            Outcome::Redirect(to) => url = to,
        }
    }
    Err(NetError::Other("Too many redirects".into()))
}

enum Outcome {
    Done(Head),
    Redirect(String),
}

fn get_once(
    req: &Request,
    url: &str,
    on_data: &mut impl FnMut(&Head, &[u8]) -> Result<(), NetError>,
) -> Result<Outcome, NetError> {
    let u = parse_url(url)?;
    let addr = (u.host, u.port)
        .to_socket_addrs()
        .map_err(|_| NetError::Offline(u.host.to_owned()))?
        .next()
        .ok_or_else(|| NetError::Offline(u.host.to_owned()))?;
    let stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT).map_err(|e| match e.kind() {
        std::io::ErrorKind::TimedOut => NetError::Timeout,
        _ => NetError::Offline(u.host.to_owned()),
    })?;
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    stream.set_write_timeout(Some(READ_TIMEOUT))?;

    let mut head = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: {USER_AGENT}\r\nAccept: */*\r\nConnection: close\r\n",
        u.path, u.host
    );
    if let Some((a, b)) = req.range {
        head.push_str(&format!("Range: bytes={a}-{b}\r\n"));
    }
    if req.gzip {
        head.push_str("Accept-Encoding: gzip\r\n");
    }
    head.push_str("\r\n");
    (&stream).write_all(head.as_bytes())?;

    let mut reader = BufReader::with_capacity(64 * 1024, stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let status: u16 = line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| NetError::Other("The server sent a malformed reply.".into()))?;

    let (mut length, mut total, mut chunked, mut gzip, mut location) = (None, None, false, false, None);
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let l = line.trim_end();
        if l.is_empty() {
            break;
        }
        let Some((name, value)) = l.split_once(':') else { continue };
        let value = value.trim();
        match name.to_ascii_lowercase().as_str() {
            "content-length" => length = value.parse().ok(),
            "content-range" => total = value.rsplit('/').next().and_then(|t| t.parse().ok()),
            "transfer-encoding" => chunked = value.eq_ignore_ascii_case("chunked"),
            "content-encoding" => gzip = value.eq_ignore_ascii_case("gzip"),
            "location" => location = Some(value.to_owned()),
            _ => {}
        }
    }

    if matches!(status, 301 | 302 | 303 | 307 | 308) {
        let to = location.ok_or_else(|| NetError::Status(status))?;
        let to = if to.starts_with('/') { format!("http://{}{to}", u.host) } else { to };
        if to.starts_with("https://") {
            return Err(NetError::Other("The server now requires HTTPS, which RuffleVita can't use.".into()));
        }
        return Ok(Outcome::Redirect(to));
    }
    if status != 200 && status != 206 {
        return Err(NetError::Status(status));
    }
    let head = Head { length, total: total.or(if status == 200 { length } else { None }) };

    let body: Box<dyn Read> = if chunked {
        Box::new(Chunked { inner: reader, left: 0, done: false })
    } else if let Some(n) = length {
        Box::new(reader.take(n))
    } else {
        Box::new(reader)
    };
    let mut body: Box<dyn Read> = if gzip { Box::new(flate2::read::GzDecoder::new(body)) } else { body };
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        if cancelled(req) {
            return Err(NetError::Cancelled);
        }
        let n = body.read(&mut buf)?;
        if n == 0 {
            break;
        }
        on_data(&head, &buf[..n])?;
    }
    Ok(Outcome::Done(head))
}

/// Decodes `Transfer-Encoding: chunked`.
struct Chunked<R> {
    inner: R,
    left: u64,
    done: bool,
}

impl<R: BufRead> Read for Chunked<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.done {
            return Ok(0);
        }
        if self.left == 0 {
            let mut line = String::new();
            self.inner.read_line(&mut line)?;
            if line.trim().is_empty() {
                // The CRLF after the previous chunk.
                line.clear();
                self.inner.read_line(&mut line)?;
            }
            let size = line.trim().split(';').next().unwrap_or("");
            self.left = u64::from_str_radix(size, 16)
                .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "bad chunk size"))?;
            if self.left == 0 {
                self.done = true;
                return Ok(0);
            }
        }
        let want = buf.len().min(self.left as usize);
        let n = self.inner.read(&mut buf[..want])?;
        if n == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        self.left -= n as u64;
        Ok(n)
    }
}

/// Fetches a whole (small) body into memory.
pub fn get_bytes(req: &Request) -> Result<Vec<u8>, NetError> {
    let mut out = Vec::new();
    get(req, |_, chunk| {
        out.extend_from_slice(chunk);
        Ok(())
    })?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_urls() {
        let u = parse_url("http://files.silvergames.com/flash/a.swf").unwrap();
        assert_eq!((u.host, u.port, u.path), ("files.silvergames.com", 80, "/flash/a.swf"));
        let u = parse_url("http://localhost:8080").unwrap();
        assert_eq!((u.host, u.port, u.path), ("localhost", 8080, "/"));
        assert!(parse_url("https://example.com/").is_err());
    }

    #[test]
    fn decodes_chunks() {
        let data = b"4\r\nWiki\r\n5\r\npedia\r\n0\r\n\r\n";
        let mut r = Chunked { inner: &data[..], left: 0, done: false };
        let mut s = String::new();
        r.read_to_string(&mut s).unwrap();
        assert_eq!(s, "Wikipedia");
    }
}
