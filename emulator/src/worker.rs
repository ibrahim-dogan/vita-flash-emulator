//! Background thread for anything slow: reading and inflating movies, SWF
//! analysis, cover extraction and thumbnail I/O. The main thread only ever
//! polls for results, so the UI never stalls on the memory card.

use std::collections::VecDeque;
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};

use ruffle_core::tag_utils::SwfMovie;

use crate::swfinfo::{self, Image, SwfInfo};
use crate::thumbs::{self, ThumbKind};

pub enum Job {
    /// Header metadata and/or a cover pulled out of the SWF.
    Analyze { key: String, path: PathBuf, want_info: bool, want_cover: bool },
    LoadThumb { key: String },
    /// A frame captured in-game (already cropped to the stage).
    SaveScreenshot { key: String, img: Image },
    LoadMovie { path: PathBuf, url: String },
}

pub enum Done {
    Info { key: String, info: Result<SwfInfo, String> },
    /// `None` means there is no thumbnail (or extraction found nothing).
    Thumb { key: String, thumb: Option<(Image, ThumbKind)> },
    /// An `Analyze` job finished; `cover_checked` if extraction was attempted.
    Analyzed { key: String, cover_checked: bool },
    MovieProgress(f32),
    Movie(Result<SwfMovie, String>),
}

impl Job {
    /// Lower is more urgent.
    fn priority(&self) -> u8 {
        match self {
            Job::LoadMovie { .. } => 0,
            Job::SaveScreenshot { .. } => 1,
            Job::LoadThumb { .. } => 2,
            Job::Analyze { .. } => 3,
        }
    }
}

struct Shared {
    queue: Mutex<VecDeque<Job>>,
    wake: Condvar,
    /// While a game runs, background analysis must not compete with it.
    throttle: AtomicBool,
}

pub struct Worker {
    shared: Arc<Shared>,
    results: Receiver<Done>,
}

impl Worker {
    pub fn spawn() -> Self {
        let shared = Arc::new(Shared {
            queue: Mutex::new(VecDeque::new()),
            wake: Condvar::new(),
            throttle: AtomicBool::new(false),
        });
        let (tx, results) = channel();
        let s = shared.clone();
        std::thread::Builder::new()
            .name("rufflevita-worker".into())
            .stack_size(1024 * 1024)
            .spawn(move || run(s, tx))
            .expect("couldn't spawn worker thread");
        Worker { shared, results }
    }

    pub fn submit(&self, job: Job) {
        let mut q = self.shared.queue.lock().unwrap();
        let p = job.priority();
        // Keep the queue sorted by priority, FIFO within a priority.
        let at = q.iter().position(|j| j.priority() > p).unwrap_or(q.len());
        q.insert(at, job);
        self.shared.wake.notify_one();
    }

    /// Drops queued background analysis (e.g. after a rescan).
    pub fn cancel_analysis(&self) {
        self.shared
            .queue
            .lock()
            .unwrap()
            .retain(|j| !matches!(j, Job::Analyze { .. }));
    }

    pub fn set_throttled(&self, throttled: bool) {
        self.shared.throttle.store(throttled, Ordering::Relaxed);
        self.shared.wake.notify_one();
    }

    pub fn poll(&self) -> Vec<Done> {
        self.results.try_iter().collect()
    }
}

fn run(shared: Arc<Shared>, tx: Sender<Done>) {
    #[cfg(target_os = "vita")]
    unsafe {
        // Keep off the main thread's core.
        let id = vitasdk_sys::sceKernelGetThreadId();
        vitasdk_sys::sceKernelChangeThreadCpuAffinityMask(id, vitasdk_sys::SCE_KERNEL_CPU_MASK_USER_2 as _);
    }
    loop {
        let job = {
            let mut q = shared.queue.lock().unwrap();
            loop {
                let throttled = shared.throttle.load(Ordering::Relaxed);
                let ready = q.front().is_some_and(|j| !throttled || j.priority() <= 1);
                if ready {
                    break q.pop_front().unwrap();
                }
                q = shared.wake.wait(q).unwrap();
            }
        };
        let send = |d| {
            let _ = tx.send(d);
        };
        match job {
            Job::Analyze { key, path, want_info, want_cover } => {
                // A broken SWF must not take the worker thread down.
                let info = std::panic::catch_unwind(|| swfinfo::read_info(&path))
                    .unwrap_or_else(|_| Err("The file is damaged".to_owned()));
                let stage = info.as_ref().map(|i| (i.width, i.height)).unwrap_or((550, 400));
                let ok = info.is_ok();
                if want_info {
                    send(Done::Info { key: key.clone(), info });
                }
                let try_cover = want_cover && ok;
                if try_cover {
                    let thumb = std::panic::catch_unwind(|| swfinfo::extract_cover(&path, stage)).ok().flatten().map(|img| {
                        let small = swfinfo::downscale(&img, thumbs::THUMB_W, thumbs::THUMB_H);
                        if let Err(e) = thumbs::save(&key, &small, ThumbKind::Extracted) {
                            tracing::warn!("Couldn't save cover for {key}: {e}");
                        }
                        (small, ThumbKind::Extracted)
                    });
                    if thumb.is_some() {
                        send(Done::Thumb { key: key.clone(), thumb });
                    }
                }
                send(Done::Analyzed { key, cover_checked: try_cover });
            }
            Job::LoadThumb { key } => {
                let thumb = thumbs::load(&key);
                send(Done::Thumb { key, thumb });
            }
            Job::SaveScreenshot { key, img } => {
                let small = swfinfo::downscale(&img, thumbs::THUMB_W, thumbs::THUMB_H);
                drop(img);
                match thumbs::save(&key, &small, ThumbKind::Screenshot) {
                    Ok(()) => send(Done::Thumb { key, thumb: Some((small, ThumbKind::Screenshot)) }),
                    Err(e) => tracing::warn!("Couldn't save screenshot for {key}: {e}"),
                }
            }
            Job::LoadMovie { path, url } => {
                // A broken SWF must not take the worker thread (and the app) down.
                let movie = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| load_movie(&path, url, &send)))
                    .unwrap_or_else(|_| Err(format!("Couldn't open it: {}", crate::crash_message())));
                send(Done::Movie(movie));
            }
        }
    }
}

fn load_movie(path: &std::path::Path, url: String, send: &dyn Fn(Done)) -> Result<SwfMovie, String> {
    let mut file = std::fs::File::open(path).map_err(|e| format!("Couldn't open file: {e}"))?;
    let total = file.metadata().map(|m| m.len()).unwrap_or(0).max(1);
    let mut data = Vec::with_capacity(total as usize);
    let mut chunk = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut chunk).map_err(|e| format!("Couldn't read file: {e}"))?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&chunk[..n]);
        send(Done::MovieProgress((data.len() as f32 / total as f32).min(1.0)));
    }
    drop(chunk);
    SwfMovie::from_data(&data, url, None).map_err(|e| format!("Not a playable SWF: {e}"))
}
