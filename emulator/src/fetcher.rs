//! Background network work for Explore, on two threads of its own so a slow
//! connection never stalls the UI or the library's worker: one for small
//! requests (the catalog, a game's first few KB, cover art) and one for the
//! download in progress.

use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::catalog::{self, Entry, Probe};
use crate::net::{self, NetError, Request};
use crate::swfinfo::{self, Image};
use crate::thumbs::{self, ThumbKind};

/// Bytes read for a game's header info.
const PROBE_BYTES: u64 = 16 * 1024;
/// Small jobs beyond this many are dropped, oldest first, so fast scrolling
/// doesn't leave a long queue behind.
const MAX_QUEUED: usize = 24;

pub enum Job {
    Catalog,
    /// Header info and cover art for a game not probed before.
    Probe { slug: String },
    /// Cover art already on the memory card.
    LoadCover { slug: String },
}

pub enum Done {
    CatalogProgress(u64),
    Catalog(Result<Vec<Entry>, String>),
    Probe { slug: String, probe: Probe, cover: Option<Image> },
    Cover { slug: String, cover: Option<Image> },
    Progress { slug: String, done: u64, total: Option<u64> },
    Downloaded { slug: String, result: Result<PathBuf, String> },
}

struct Queue {
    jobs: Mutex<VecDeque<Job>>,
    wake: Condvar,
}

struct DownloadJob {
    slug: String,
    dest: PathBuf,
}

pub struct Fetcher {
    small: Arc<Queue>,
    download: Arc<(Mutex<Option<DownloadJob>>, Condvar)>,
    cancel: Arc<AtomicBool>,
    results: Receiver<Done>,
}

impl Fetcher {
    pub fn spawn() -> Self {
        let (tx, results) = channel();
        let small = Arc::new(Queue { jobs: Mutex::new(VecDeque::new()), wake: Condvar::new() });
        let download = Arc::new((Mutex::new(None), Condvar::new()));
        let cancel = Arc::new(AtomicBool::new(false));

        let (q, t) = (small.clone(), tx.clone());
        std::thread::Builder::new()
            .name("rufflevita-net".into())
            .stack_size(256 * 1024)
            .spawn(move || small_loop(q, t))
            .expect("couldn't spawn network thread");
        let (d, c) = (download.clone(), cancel.clone());
        std::thread::Builder::new()
            .name("rufflevita-download".into())
            .stack_size(256 * 1024)
            .spawn(move || download_loop(d, c, tx))
            .expect("couldn't spawn download thread");
        Fetcher { small, download, cancel, results }
    }

    /// Queues a small job; the newest runs first. Duplicates are dropped.
    pub fn submit(&self, job: Job) {
        let mut q = self.small.jobs.lock().unwrap();
        let key = |j: &Job| match j {
            Job::Catalog => String::from("\0catalog"),
            Job::Probe { slug } | Job::LoadCover { slug } => slug.clone(),
        };
        let k = key(&job);
        q.retain(|j| key(j) != k);
        q.push_front(job);
        q.truncate(MAX_QUEUED);
        self.small.wake.notify_one();
    }

    pub fn download(&self, slug: String, dest: PathBuf) {
        self.cancel.store(false, Ordering::Relaxed);
        let (lock, wake) = &*self.download;
        *lock.lock().unwrap() = Some(DownloadJob { slug, dest });
        wake.notify_one();
    }

    pub fn cancel_download(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn poll(&self) -> Vec<Done> {
        self.results.try_iter().collect()
    }
}

fn small_loop(queue: Arc<Queue>, tx: Sender<Done>) {
    loop {
        let job = {
            let mut q = queue.jobs.lock().unwrap();
            loop {
                if let Some(j) = q.pop_front() {
                    break j;
                }
                q = queue.wake.wait(q).unwrap();
            }
        };
        let done = match job {
            Job::Catalog => Done::Catalog(fetch_catalog(&tx)),
            Job::Probe { slug } => {
                let (probe, cover) = probe(&slug);
                Done::Probe { slug, probe, cover }
            }
            Job::LoadCover { slug } => {
                let cover = thumbs::read(&cover_path(&slug)).map(|(img, _)| img);
                Done::Cover { slug, cover }
            }
        };
        if tx.send(done).is_err() {
            return;
        }
    }
}

fn fetch_catalog(tx: &Sender<Done>) -> Result<Vec<Entry>, String> {
    let mut req = Request::get(catalog::listing_url());
    req.gzip = true;
    let mut body = Vec::new();
    let mut last = Instant::now();
    net::get(&req, |_, chunk| {
        body.extend_from_slice(chunk);
        if last.elapsed() > Duration::from_millis(150) {
            last = Instant::now();
            let _ = tx.send(Done::CatalogProgress(body.len() as u64));
        }
        Ok(())
    })
    .map_err(|e| e.to_string())?;
    let entries = catalog::parse_listing(&String::from_utf8_lossy(&body));
    if entries.is_empty() {
        return Err("The catalog page didn't list any games.".into());
    }
    Ok(entries)
}

pub fn cover_path(slug: &str) -> PathBuf {
    catalog::covers_dir().join(format!("{slug}.fvt"))
}

fn probe(slug: &str) -> (Probe, Option<Image>) {
    let mut p = Probe::default();
    let url = catalog::swf_url(slug);
    let mut req = Request::get(&url);
    req.range = Some((0, PROBE_BYTES - 1));
    let mut bytes = Vec::new();
    let result = net::get(&req, |head, chunk| {
        p.total = head.total;
        bytes.extend_from_slice(chunk);
        // A server that ignores the range sends the whole file: stop early.
        if bytes.len() as u64 >= PROBE_BYTES { Err(NetError::Cancelled) } else { Ok(()) }
    });
    match result {
        Ok(_) | Err(NetError::Cancelled) => p.info = Some(swfinfo::read_info_from(&bytes[..])),
        Err(e) => {
            tracing::warn!("Couldn't look at {slug}: {e}");
            p.info = Some(Err(e.to_string()));
        }
    }

    let cover = match net::get_bytes(&Request::get(&catalog::cover_url(slug))) {
        Ok(jpeg) => decode_jpeg(&jpeg),
        Err(NetError::Status(404)) => None,
        Err(e) => {
            tracing::warn!("Couldn't fetch the cover of {slug}: {e}");
            None
        }
    };
    if let Some(img) = &cover {
        let _ = std::fs::create_dir_all(catalog::covers_dir());
        p.has_cover = thumbs::write(&cover_path(slug), img, ThumbKind::Extracted).is_ok();
    }
    (p, cover)
}

fn decode_jpeg(data: &[u8]) -> Option<Image> {
    let mut d = jpeg_decoder::Decoder::new(data);
    let pixels = d.decode().ok()?;
    let info = d.info()?;
    let (w, h) = (info.width as u32, info.height as u32);
    let rgba: Vec<u8> = match info.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => pixels.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        jpeg_decoder::PixelFormat::L8 => pixels.iter().flat_map(|&l| [l, l, l, 255]).collect(),
        _ => return None,
    };
    (rgba.len() == (w * h * 4) as usize).then_some(Image { w, h, rgba })
}

fn download_loop(slot: Arc<(Mutex<Option<DownloadJob>>, Condvar)>, cancel: Arc<AtomicBool>, tx: Sender<Done>) {
    loop {
        let job = {
            let (lock, wake) = &*slot;
            let mut j = lock.lock().unwrap();
            loop {
                if let Some(job) = j.take() {
                    break job;
                }
                j = wake.wait(j).unwrap();
            }
        };
        let result = download(&job, &cancel, &tx);
        if tx.send(Done::Downloaded { slug: job.slug, result }).is_err() {
            return;
        }
    }
}

fn download(job: &DownloadJob, cancel: &AtomicBool, tx: &Sender<Done>) -> Result<PathBuf, String> {
    let url = catalog::swf_url(&job.slug);
    let part = job.dest.with_extension("swf.part");
    let mut file = std::fs::File::create(&part).map_err(|e| format!("Couldn't write to the memory card: {e}"))?;
    let mut req = Request::get(&url);
    req.cancel = Some(cancel);
    let mut done = 0u64;
    let mut last = Instant::now();
    let result = net::get(&req, |head, chunk| {
        file.write_all(chunk).map_err(|e| NetError::Other(format!("Couldn't write to the memory card: {e}")))?;
        done += chunk.len() as u64;
        if last.elapsed() > Duration::from_millis(100) {
            last = Instant::now();
            let _ = tx.send(Done::Progress { slug: job.slug.clone(), done, total: head.length });
        }
        Ok(())
    });
    drop(file);
    match result {
        Ok(head) if head.length.is_none_or(|n| n == done) => {
            std::fs::rename(&part, &job.dest).map_err(|e| format!("Couldn't save the game: {e}"))?;
            tracing::info!("Downloaded {} ({done} bytes)", job.slug);
            Ok(job.dest.clone())
        }
        Ok(_) => {
            let _ = std::fs::remove_file(&part);
            Err("The download stopped early. Try again.".into())
        }
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e.to_string())
        }
    }
}
