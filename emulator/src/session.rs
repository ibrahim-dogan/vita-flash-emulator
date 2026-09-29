//! A running game: the Ruffle player plus the translation from Vita input
//! to the keyboard/mouse events Flash content expects.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use glow::HasContext;
use ruffle_core::backend::navigator::NullExecutor;
use ruffle_core::config::Letterbox;
use ruffle_core::events::{MouseButton, PlayerEvent, TextControlCode};
use ruffle_core::tag_utils::SwfMovie;
use ruffle_core::{Player, PlayerBuilder, PlayerRuntime, StageScaleMode};
use ruffle_render_glow::GlowRenderBackend;

use crate::backends::audio::SdlAudioBackend;
use crate::backends::log::TracingLogBackend;
use crate::backends::navigator::{LocalNavigator, MissingFiles};
use crate::backends::storage::DiskStorageBackend;
use crate::backends::ui::SdlUiBackend;
use crate::config::{Action, Profile, ScaleMode, StickMode};
use crate::input::{Btn, Input, InputEvent, Panel, TouchPhase};
use crate::keys;
use crate::platform::{self, SCREEN_H, SCREEN_W};
use crate::swfinfo::Image;
use crate::ui::{Gfx, Icon, theme};

/// Something the game loop must act on.
pub enum Signal {
    None,
    OpenMenu,
}

const MOUSE_FROM_BUTTON: u8 = 1;
const MOUSE_FROM_TOUCH: u8 = 2;
/// How long a single script may run before Ruffle stops it. Flash Player
/// allows 15 s on a desktop PC; the Vita is dozens of times slower, and games
/// that build a level in one go (Happy Wheels) need far longer. It only
/// matters for scripts stuck in a loop, where the app would wait this long.
const MAX_SCRIPT_DURATION: Duration =
    Duration::from_secs(if cfg!(target_os = "vita") { 120 } else { 15 });
/// How long the mouse stays on the stage after its last use.
const MOUSE_IDLE: Duration = Duration::from_secs(5);

struct RearTouch {
    finger: i64,
    last: (f32, f32),
    start: Instant,
    travel: f32,
}

pub struct Session {
    pub key: String,
    pub name: String,
    pub profile: Profile,
    player: Arc<Mutex<Player>>,
    executor: NullExecutor,
    movie_size: (f32, f32),
    last_tick: Instant,
    pending: Vec<PlayerEvent>,

    key_counts: HashMap<u16, u32>,
    /// Action started by each button while in-game, released on button up.
    active: [Option<Action>; 12],
    /// Keys held by stick directions: [left, right] x [up, down, left, right].
    stick_keys: [[Option<u16>; 4]; 2],
    mouse_sources: u8,
    /// Last mouse activity (cursor, touch, click). While the mouse is on the
    /// stage Ruffle hit-tests the whole display list after every update, so
    /// it is taken off the stage when idle.
    mouse_used: Option<Instant>,
    cursor: (f32, f32),
    cursor_shown_until: Instant,
    front_finger: Option<i64>,
    rear: Option<RearTouch>,

    fps_frames: u32,
    fps_since: Instant,
    running: bool,
    pub fps: f32,
    perf: PerfTotals,
    /// Per-frame breakdown shown under the FPS counter.
    perf_line: String,
    perf_logged: Instant,
    pub started: Instant,
    pub auto_cover_taken: bool,
    missing: MissingFiles,
}

impl Session {
    pub fn new(
        gl: Arc<glow::Context>,
        audio: &sdl2::AudioSubsystem,
        window: &sdl2::video::Window,
        movie: SwfMovie,
        movie_url: &str,
        key: String,
        name: String,
        profile: Profile,
    ) -> Result<Self, String> {
        let movie_size = (movie.width().to_pixels() as f32, movie.height().to_pixels() as f32);
        let renderer = GlowRenderBackend::new(gl, false, profile.quality.stage_quality())
            .map_err(|e| format!("Renderer: {e}"))?;
        let audio = SdlAudioBackend::new(audio).map_err(|e| format!("Audio: {e}"))?;
        let executor = NullExecutor::new();
        let missing = MissingFiles::default();
        let navigator = LocalNavigator::new(&executor, movie_url, missing.clone());
        let storage = DiskStorageBackend::new(platform::saves_dir());

        let (scale, force) = scale_mode(profile.scale);
        let player = PlayerBuilder::new()
            .with_renderer(renderer)
            .with_audio(audio)
            .with_ui(SdlUiBackend::new(Box::new(window.clone())))
            .with_storage(Box::new(storage))
            .with_navigator(navigator)
            .with_log(TracingLogBackend)
            .with_movie(movie)
            .with_viewport_dimensions(SCREEN_W, SCREEN_H, 1.0)
            .with_letterbox(Letterbox::On)
            .with_scale_mode(scale, force)
            .with_quality(profile.quality.stage_quality())
            .with_player_runtime(PlayerRuntime::FlashPlayer)
            .with_max_execution_duration(MAX_SCRIPT_DURATION)
            .with_autoplay(true)
            .build();

        let now = Instant::now();
        Ok(Self {
            key,
            name,
            profile,
            player,
            executor,
            movie_size,
            last_tick: now,
            pending: Vec::new(),
            key_counts: HashMap::new(),
            active: [None; 12],
            stick_keys: [[None; 4]; 2],
            mouse_sources: 0,
            mouse_used: None,
            cursor: (SCREEN_W as f32 * 0.5, SCREEN_H as f32 * 0.5),
            cursor_shown_until: now,
            front_finger: None,
            rear: None,
            fps_frames: 0,
            fps_since: now,
            running: true,
            fps: 0.0,
            perf: PerfTotals::default(),
            perf_line: String::new(),
            perf_logged: now,
            started: now,
            auto_cover_taken: false,
            missing,
        })
    }

    // ------------------------------------------------------------ settings

    pub fn apply_profile(&mut self, profile: Profile) {
        let old = std::mem::replace(&mut self.profile, profile);
        let mut player = self.player.lock().unwrap();
        if old.scale != self.profile.scale {
            let (mode, force) = scale_mode(self.profile.scale);
            player.set_forced_scale_mode(false);
            player.set_scale_mode(mode);
            player.set_forced_scale_mode(force);
        }
        if old.quality != self.profile.quality {
            player.set_quality(self.profile.quality.stage_quality());
        }
    }

    /// Where the stage lands on screen, used to crop screenshots.
    pub fn stage_rect(&self) -> (u32, u32, u32, u32) {
        let (sw, sh) = (SCREEN_W as f32, SCREEN_H as f32);
        match self.profile.scale {
            ScaleMode::Stretch | ScaleMode::Zoom => (0, 0, SCREEN_W, SCREEN_H),
            ScaleMode::Fit | ScaleMode::Native => {
                let (mw, mh) = (self.movie_size.0.max(1.0), self.movie_size.1.max(1.0));
                let s = (sw / mw).min(sh / mh);
                let (w, h) = ((mw * s).round(), (mh * s).round());
                (((sw - w) * 0.5) as u32, ((sh - h) * 0.5) as u32, w as u32, h as u32)
            }
        }
    }

    /// Files the game tried to load that don't exist (reported once each).
    pub fn take_missing(&self) -> Vec<String> {
        let mut m = self.missing.lock().unwrap();
        let mut out: Vec<String> = m.drain(..).collect();
        out.dedup();
        out
    }

    // --------------------------------------------------------------- input

    fn send(&mut self, ev: PlayerEvent) {
        self.pending.push(ev);
    }

    fn press_key(&mut self, k: u16) {
        let n = self.key_counts.entry(k).or_insert(0);
        *n += 1;
        if *n == 1 {
            self.send(PlayerEvent::KeyDown { key: keys::get(k).descriptor() });
        }
    }

    fn release_key(&mut self, k: u16) {
        if let Some(n) = self.key_counts.get_mut(&k) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                self.key_counts.remove(&k);
                self.send(PlayerEvent::KeyUp { key: keys::get(k).descriptor() });
            }
        }
    }

    fn mouse_move(&mut self) {
        self.mouse_used = Some(Instant::now());
        let (x, y) = self.cursor;
        self.send(PlayerEvent::MouseMove { x: x as f64, y: y as f64 });
    }

    fn mouse_press(&mut self, source: u8) {
        let was = self.mouse_sources;
        self.mouse_sources |= source;
        if was == 0 {
            self.mouse_move();
            let (x, y) = self.cursor;
            self.send(PlayerEvent::MouseDown {
                x: x as f64,
                y: y as f64,
                button: MouseButton::Left,
                index: None,
            });
        }
    }

    fn mouse_release(&mut self, source: u8) {
        if self.mouse_sources & source == 0 {
            return;
        }
        self.mouse_sources &= !source;
        self.mouse_used = Some(Instant::now());
        if self.mouse_sources == 0 {
            let (x, y) = self.cursor;
            self.send(PlayerEvent::MouseUp { x: x as f64, y: y as f64, button: MouseButton::Left });
        }
    }

    fn show_cursor(&mut self) {
        self.cursor_shown_until = Instant::now() + Duration::from_secs(4);
    }

    pub fn cursor_visible(&self) -> bool {
        Instant::now() < self.cursor_shown_until
    }

    pub fn handle(&mut self, ev: &InputEvent, input: &Input) -> Signal {
        match ev {
            InputEvent::Button(b, true) => {
                // L + R + Start always opens the menu, whatever the bindings.
                let combo = [Btn::L, Btn::R, Btn::Start];
                if combo.contains(b) && combo.iter().all(|c| input.held(*c)) {
                    return Signal::OpenMenu;
                }
                let action = self.profile.action(*b);
                match action {
                    Action::Menu => return Signal::OpenMenu,
                    Action::None => {}
                    Action::Key(k) => self.press_key(k),
                    Action::MouseLeft => self.mouse_press(MOUSE_FROM_BUTTON),
                }
                self.active[*b as usize] = Some(action);
            }
            InputEvent::Button(b, false) => match self.active[*b as usize].take() {
                Some(Action::Key(k)) => self.release_key(k),
                Some(Action::MouseLeft) => self.mouse_release(MOUSE_FROM_BUTTON),
                _ => {}
            },
            InputEvent::Touch(t) if t.panel == Panel::Front => match t.phase {
                TouchPhase::Down if self.front_finger.is_none() => {
                    self.front_finger = Some(t.finger);
                    self.cursor = (t.x, t.y);
                    self.cursor_shown_until = Instant::now();
                    self.mouse_press(MOUSE_FROM_TOUCH);
                }
                TouchPhase::Move if self.front_finger == Some(t.finger) => {
                    self.cursor = (t.x, t.y);
                    self.mouse_move();
                }
                TouchPhase::Up if self.front_finger == Some(t.finger) => {
                    self.front_finger = None;
                    self.cursor = (t.x, t.y);
                    self.mouse_move();
                    self.mouse_release(MOUSE_FROM_TOUCH);
                }
                _ => {}
            },
            InputEvent::Touch(t) if self.profile.rear_touch => self.handle_rear(t.phase, t.finger, t.x, t.y),
            InputEvent::Text(s) => {
                for codepoint in s.chars() {
                    self.send(PlayerEvent::TextInput { codepoint });
                }
            }
            InputEvent::Backspace => self.send(PlayerEvent::TextControl { code: TextControlCode::Backspace }),
            _ => {}
        }
        Signal::None
    }

    /// Rear pad as a laptop-style trackpad: drag moves, tap clicks.
    fn handle_rear(&mut self, phase: TouchPhase, finger: i64, x: f32, y: f32) {
        match phase {
            TouchPhase::Down if self.rear.is_none() => {
                self.rear = Some(RearTouch { finger, last: (x, y), start: Instant::now(), travel: 0.0 });
            }
            TouchPhase::Move => {
                let Some(r) = self.rear.as_mut().filter(|r| r.finger == finger) else { return };
                let (dx, dy) = (x - r.last.0, y - r.last.1);
                r.last = (x, y);
                r.travel += dx.abs() + dy.abs();
                self.cursor.0 = (self.cursor.0 + dx * 1.3).clamp(0.0, SCREEN_W as f32 - 1.0);
                self.cursor.1 = (self.cursor.1 + dy * 1.3).clamp(0.0, SCREEN_H as f32 - 1.0);
                self.show_cursor();
                self.mouse_move();
            }
            TouchPhase::Up => {
                let Some(r) = self.rear.take_if(|r| r.finger == finger) else { return };
                if r.travel < 12.0 && r.start.elapsed() < Duration::from_millis(250) {
                    self.mouse_press(MOUSE_FROM_TOUCH);
                    self.mouse_release(MOUSE_FROM_TOUCH);
                }
            }
            _ => {}
        }
    }

    /// Per-frame analog stick handling.
    pub fn update_sticks(&mut self, input: &Input, dt: f32) {
        for (i, (stick, mode)) in [(input.left, self.profile.left_stick), (input.right, self.profile.right_stick)]
            .into_iter()
            .enumerate()
        {
            let s = stick.filtered();
            let dirs: Option<[&str; 4]> = match mode {
                StickMode::Arrows => Some(["Up", "Down", "Left", "Right"]),
                StickMode::Wasd => Some(["W", "S", "A", "D"]),
                StickMode::Mouse => {
                    let mag = (s.x * s.x + s.y * s.y).sqrt();
                    if mag > 0.0 {
                        let speed = 250.0 + self.profile.cursor_speed as f32 * 90.0;
                        let v = speed * mag.powf(1.6) / mag;
                        self.cursor.0 = (self.cursor.0 + s.x * v * dt).clamp(0.0, SCREEN_W as f32 - 1.0);
                        self.cursor.1 = (self.cursor.1 + s.y * v * dt).clamp(0.0, SCREEN_H as f32 - 1.0);
                        self.show_cursor();
                        self.mouse_move();
                    }
                    None
                }
                StickMode::Off => None,
            };
            // Hysteresis so a stick resting near the threshold doesn't chatter.
            let values = [-s.y, s.y, -s.x, s.x];
            for d in 0..4 {
                let want = dirs.map(|names| keys::key(names[d]));
                let held = self.stick_keys[i][d];
                let on = match held {
                    Some(_) => values[d] > 0.35,
                    None => values[d] > 0.5,
                };
                match (held, on && want.is_some()) {
                    (None, true) => {
                        let k = want.unwrap();
                        self.press_key(k);
                        self.stick_keys[i][d] = Some(k);
                    }
                    (Some(k), false) => {
                        self.release_key(k);
                        self.stick_keys[i][d] = None;
                    }
                    (Some(k), true) if Some(k) != want => {
                        self.release_key(k);
                        self.stick_keys[i][d] = None;
                    }
                    _ => {}
                }
            }
        }
    }

    /// Lets go of everything, e.g. before showing the pause menu, so no key
    /// stays stuck down in the game.
    pub fn release_all(&mut self) {
        for b in 0..12 {
            match self.active[b].take() {
                Some(Action::Key(k)) => self.release_key(k),
                Some(Action::MouseLeft) => self.mouse_release(MOUSE_FROM_BUTTON),
                _ => {}
            }
        }
        for i in 0..2 {
            for d in 0..4 {
                if let Some(k) = self.stick_keys[i][d].take() {
                    self.release_key(k);
                }
            }
        }
        let held: Vec<u16> = self.key_counts.keys().copied().collect();
        for k in held {
            self.key_counts.insert(k, 1);
            self.release_key(k);
        }
        self.front_finger = None;
        self.rear = None;
        self.mouse_release(MOUSE_FROM_TOUCH | MOUSE_FROM_BUTTON);
        self.flush_events();
    }

    fn flush_events(&mut self) {
        let mouse_active =
            self.mouse_sources != 0 || self.mouse_used.is_some_and(|t| t.elapsed() < MOUSE_IDLE);
        let mut player = self.player.lock().unwrap();
        // Set before handling events so the first click after idling still hits.
        player.set_mouse_in_stage(mouse_active);
        for ev in self.pending.drain(..) {
            player.handle_event(ev);
        }
    }

    // ------------------------------------------------------------- running

    pub fn set_running(&mut self, running: bool) {
        self.running = running;
        self.fps_frames = 0;
        self.fps_since = Instant::now();
        self.perf = PerfTotals::default();
        let mut player = self.player.lock().unwrap();
        player.set_is_playing(running);
        if !running {
            player.flush_shared_objects();
        }
        drop(player);
        self.last_tick = Instant::now();
    }

    /// Delivers input and advances the movie by the real time elapsed.
    pub fn tick(&mut self) {
        self.flush_events();
        let now = Instant::now();
        // Clamp long gaps (loading, suspend) so the game doesn't fast-forward.
        let dt = (now - self.last_tick).as_secs_f64().min(0.1) * 1000.0;
        self.last_tick = now;
        self.executor.run();
        self.player.lock().unwrap().tick(dt);
        self.perf.tick += now.elapsed();
    }

    pub fn needs_render(&self) -> bool {
        self.player.lock().unwrap().needs_render()
    }

    pub fn time_til_next_frame(&self) -> Duration {
        self.player.lock().unwrap().time_til_next_frame()
    }

    pub fn render(&mut self) {
        let start = Instant::now();
        self.player.lock().unwrap().render();
        self.perf.render += start.elapsed();
        if !self.running {
            return;
        }
        self.fps_frames += 1;
        let el = self.fps_since.elapsed().as_secs_f32();
        if el >= 1.0 {
            self.fps = self.fps_frames as f32 / el;
            self.update_perf_line();
            self.fps_frames = 0;
            self.fps_since = Instant::now();
        }
    }

    /// Time spent flushing the overlay and swapping buffers, which is where
    /// vitaGL waits for the GPU to finish the previous frame.
    pub fn add_present_time(&mut self, d: Duration) {
        self.perf.present += d;
    }

    fn update_perf_line(&mut self) {
        let frames = self.fps_frames.max(1) as f32;
        let ms = |d: Duration| d.as_secs_f32() * 1000.0 / frames;
        let p = std::mem::take(&mut self.perf);
        let draws = ruffle_render_glow::take_draw_calls() as f32 / frames;
        self.perf_line = format!(
            "tick {:.1} \u{b7} render {:.1} \u{b7} present {:.1} ms \u{b7} {draws:.0} draws",
            ms(p.tick),
            ms(p.render),
            ms(p.present),
        );
        if self.profile.show_fps && self.perf_logged.elapsed() >= Duration::from_secs(10) {
            self.perf_logged = Instant::now();
            let mem = crate::platform::memory_summary().unwrap_or_default();
            tracing::info!("{}: {:.1} FPS \u{b7} {} \u{b7} {mem}", self.key, self.fps, self.perf_line);
        }
    }

    /// Draws the virtual cursor and FPS counter over the frame.
    pub fn draw_overlay(&self, g: &mut Gfx) {
        if self.cursor_visible() {
            let (x, y) = self.cursor;
            let size = 30.0;
            // Icon art has its hotspot at (0.2, 0.08) of the box.
            let (ox, oy) = (x - size * 0.2, y - size * 0.08);
            g.icon(Icon::CursorOutline, ox, oy, size, crate::ui::Color::hex(0x000000).alpha(0.85));
            g.icon(Icon::CursorFill, ox, oy, size, crate::ui::Color::hex(0xFFFFFF));
        }
        if self.profile.show_fps {
            let text = format!("{:.0} FPS", self.fps);
            let w = g.measure(crate::ui::FontId::Bold, 13.0, &text) + 16.0;
            let r = crate::ui::Rect::new(8.0, 8.0, w, 22.0);
            g.rounded(r, 11.0, crate::ui::Color::hex(0x000000).alpha(0.55));
            let color = if self.fps >= 50.0 {
                theme::OK
            } else if self.fps >= 24.0 {
                theme::WARN
            } else {
                theme::DANGER
            };
            g.text_mid(crate::ui::FontId::Bold, 13.0, r.x + 8.0, r.center_y(), color, &text);

            let mem = crate::platform::memory_summary();
            let lines = std::iter::once(self.perf_line.as_str()).chain(mem.as_deref());
            for (i, line) in lines.filter(|l| !l.is_empty()).enumerate() {
                let w = g.measure(crate::ui::FontId::Regular, 12.0, line) + 16.0;
                let r = crate::ui::Rect::new(8.0, 34.0 + i as f32 * 22.0, w, 20.0);
                g.rounded(r, 10.0, crate::ui::Color::hex(0x000000).alpha(0.55));
                g.text_mid(crate::ui::FontId::Regular, 12.0, r.x + 8.0, r.center_y(), theme::TEXT, line);
            }
        }
    }

    /// Reads the stage area of the current back buffer (call right after
    /// `render`, before drawing any overlay).
    pub fn capture(&self, gl: &glow::Context) -> Option<Image> {
        let (x, y, w, h) = self.stage_rect();
        if w == 0 || h == 0 {
            return None;
        }
        let mut data = vec![0u8; (w * h * 4) as usize];
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
            gl.read_pixels(
                x as i32,
                (SCREEN_H - y - h) as i32,
                w as i32,
                h as i32,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut data)),
            );
        }
        // GL rows are bottom-up.
        let stride = (w * 4) as usize;
        let mut flipped = vec![0u8; data.len()];
        for row in 0..h as usize {
            let src = (h as usize - 1 - row) * stride;
            flipped[row * stride..(row + 1) * stride].copy_from_slice(&data[src..src + stride]);
        }
        for px in flipped.chunks_exact_mut(4) {
            px[3] = 255;
        }
        // Reject blank frames (all one colour) as covers.
        let first = [flipped[0], flipped[1], flipped[2]];
        let varied = flipped.chunks_exact(4).step_by(97).any(|p| {
            (p[0] as i32 - first[0] as i32).abs() + (p[1] as i32 - first[1] as i32).abs()
                + (p[2] as i32 - first[2] as i32).abs()
                > 24
        });
        varied.then_some(Image { w, h, rgba: flipped })
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Ok(mut player) = self.player.lock() {
            player.flush_shared_objects();
        }
    }
}

fn scale_mode(mode: ScaleMode) -> (StageScaleMode, bool) {
    match mode {
        ScaleMode::Fit => (StageScaleMode::ShowAll, true),
        ScaleMode::Stretch => (StageScaleMode::ExactFit, true),
        ScaleMode::Zoom => (StageScaleMode::NoBorder, true),
        ScaleMode::Native => (StageScaleMode::ShowAll, false),
    }
}

/// Time accumulated per phase since the FPS counter last updated.
#[derive(Default)]
struct PerfTotals {
    tick: Duration,
    render: Duration,
    present: Duration,
}
