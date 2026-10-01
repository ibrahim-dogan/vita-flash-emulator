//! Desktop-only automation for development: `RUFFLEVITA_SCRIPT` drives the
//! app with synthetic Vita input and writes screenshots, e.g.
//!
//! ```text
//! RUFFLEVITA_SCRIPT="wait 30; shot lib.png; press Cross; sleep 4000; shot game.png; quit"
//! ```

use std::collections::VecDeque;

use crate::input::{Btn, Input, TouchPhase};

enum Step {
    Wait(u32),
    /// Wait until the game has run this many frames (see `Session::frames`).
    At(u64),
    Sleep(u64),
    Press(Btn),
    Hold(Btn, u32),
    Tap(f32, f32),
    /// Text typed on a keyboard; `_` stands for a space.
    Type(String),
    Enter,
    Shot(String),
    Record(Option<String>),
    Quit,
}

pub enum Output {
    None,
    Screenshot(String),
    /// Start recording frames into a directory, or stop (`None`).
    Record(Option<String>),
    Quit,
}

pub struct Script {
    steps: VecDeque<Step>,
    wait: u32,
    release: Option<(Btn, u32)>,
    touch_up: Option<(f32, f32)>,
    until: Option<std::time::Instant>,
    at: Option<u64>,
}

fn parse_btn(s: &str) -> Option<Btn> {
    Btn::ALL.into_iter().find(|b| format!("{b:?}").eq_ignore_ascii_case(s))
}

impl Script {
    pub fn from_env() -> Option<Self> {
        let text = std::env::var("RUFFLEVITA_SCRIPT").ok()?;
        let mut steps = VecDeque::new();
        for cmd in text.split(';').map(str::trim).filter(|c| !c.is_empty()) {
            let parts: Vec<&str> = cmd.split_whitespace().collect();
            let step = match parts.as_slice() {
                ["wait", n] => n.parse().ok().map(Step::Wait),
                ["at", n] => n.parse().ok().map(Step::At),
                ["sleep", ms] => ms.parse().ok().map(Step::Sleep),
                ["press", b] => parse_btn(b).map(Step::Press),
                ["hold", b, n] => parse_btn(b).zip(n.parse().ok()).map(|(b, n)| Step::Hold(b, n)),
                ["tap", x, y] => x.parse().ok().zip(y.parse().ok()).map(|(x, y)| Step::Tap(x, y)),
                ["type", text] => Some(Step::Type(text.replace('_', " "))),
                ["enter"] => Some(Step::Enter),
                ["shot", path] => Some(Step::Shot((*path).to_owned())),
                ["rec", dir] => Some(Step::Record(Some((*dir).to_owned()))),
                ["stoprec"] => Some(Step::Record(None)),
                ["quit"] => Some(Step::Quit),
                _ => None,
            };
            match step {
                Some(s) => steps.push_back(s),
                None => eprintln!("RUFFLEVITA_SCRIPT: ignoring '{cmd}'"),
            }
        }
        Some(Script { steps, wait: 0, release: None, touch_up: None, until: None, at: None })
    }

    /// Runs once per frame, before the app consumes input. `game_frame` is
    /// the running game's frame count, if a game is running.
    pub fn step(&mut self, input: &mut Input, game_frame: Option<u64>) -> Output {
        if let Some((b, n)) = self.release.take() {
            if n == 0 {
                input.inject(b, false);
            } else {
                self.release = Some((b, n - 1));
            }
        }
        if let Some((x, y)) = self.touch_up.take() {
            input.inject_touch(TouchPhase::Up, x, y);
        }
        if self.wait > 0 {
            self.wait -= 1;
            return Output::None;
        }
        if let Some(t) = self.until {
            if std::time::Instant::now() < t {
                return Output::None;
            }
            self.until = None;
        }
        if let Some(n) = self.at {
            if game_frame.is_none_or(|f| f < n) {
                return Output::None;
            }
            self.at = None;
        }
        match self.steps.pop_front() {
            Some(Step::Wait(n)) => self.wait = n,
            Some(Step::At(n)) => self.at = Some(n),
            Some(Step::Sleep(ms)) => {
                self.until = Some(std::time::Instant::now() + std::time::Duration::from_millis(ms))
            }
            Some(Step::Press(b)) => {
                input.inject(b, true);
                self.release = Some((b, 1));
                self.wait = 2;
            }
            Some(Step::Hold(b, n)) => {
                input.inject(b, true);
                self.release = Some((b, n));
                self.wait = n + 1;
            }
            Some(Step::Tap(x, y)) => {
                input.inject_touch(TouchPhase::Down, x, y);
                self.touch_up = Some((x, y));
                self.wait = 2;
            }
            Some(Step::Type(text)) => {
                input.inject_event(crate::input::InputEvent::Text(text));
                self.wait = 2;
            }
            Some(Step::Enter) => {
                input.inject_event(crate::input::InputEvent::Enter);
                self.wait = 2;
            }
            Some(Step::Shot(p)) => return Output::Screenshot(p),
            Some(Step::Record(d)) => return Output::Record(d),
            Some(Step::Quit) => return Output::Quit,
            None => {}
        }
        Output::None
    }
}

/// Captures frames at ~30 fps for trailers; writes an ffmpeg concat list
/// with real frame durations when stopped.
pub struct Recorder {
    dir: std::path::PathBuf,
    frames: Vec<(String, std::time::Instant)>,
}

impl Recorder {
    pub fn new(dir: &str) -> Self {
        let _ = std::fs::create_dir_all(dir);
        Recorder { dir: dir.into(), frames: Vec::new() }
    }

    pub fn due(&self) -> bool {
        self.frames
            .last()
            .is_none_or(|(_, t)| t.elapsed() >= std::time::Duration::from_millis(33))
    }

    pub fn capture(&mut self, gl: &glow::Context, w: u32, h: u32) {
        let name = format!("f{:05}.png", self.frames.len());
        let path = self.dir.join(&name);
        save_png(gl, w, h, &path.to_string_lossy(), png::Compression::Fast, false);
        self.frames.push((name, std::time::Instant::now()));
    }

    pub fn finish(self) {
        let mut list = String::new();
        for (i, (name, t)) in self.frames.iter().enumerate() {
            let dur = self.frames.get(i + 1).map(|(_, n)| (*n - *t).as_secs_f64()).unwrap_or(0.033);
            list.push_str(&format!("file '{name}'\nduration {dur:.4}\n"));
        }
        if let Some((name, _)) = self.frames.last() {
            list.push_str(&format!("file '{name}'\n"));
        }
        let _ = std::fs::write(self.dir.join("frames.txt"), list);
        eprintln!("recorded {} frames to {}", self.frames.len(), self.dir.display());
    }
}

/// Saves the current back buffer as a PNG.
pub fn save_screenshot(gl: &glow::Context, w: u32, h: u32, path: &str) {
    save_png(gl, w, h, path, png::Compression::Balanced, true);
}

fn save_png(gl: &glow::Context, w: u32, h: u32, path: &str, level: png::Compression, verbose: bool) {
    use glow::HasContext;
    let mut data = vec![0u8; (w * h * 4) as usize];
    unsafe {
        gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
        gl.read_pixels(
            0,
            0,
            w as i32,
            h as i32,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(Some(&mut data)),
        );
    }
    let stride = (w * 4) as usize;
    let mut flipped = Vec::with_capacity(data.len());
    for row in (0..h as usize).rev() {
        flipped.extend_from_slice(&data[row * stride..(row + 1) * stride]);
    }
    for px in flipped.chunks_exact_mut(4) {
        px[3] = 255;
    }
    let file = match std::fs::File::create(path) {
        Ok(f) => f,
        Err(e) => return tracing::warn!("screenshot {path}: {e}"),
    };
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(level);
    match enc.write_header().and_then(|mut wr| wr.write_image_data(&flipped)) {
        Ok(()) if verbose => tracing::info!("screenshot saved to {path}"),
        Ok(()) => {}
        Err(e) => tracing::warn!("screenshot {path}: {e}"),
    }
}
