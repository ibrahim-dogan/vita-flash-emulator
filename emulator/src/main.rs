//! RuffleVita: a Flash player for PS Vita built on Ruffle.
//!
//! The app is a small state machine: Library (with its Explore tab) ->
//! Loading -> Playing, with modal overlays (pause menu, settings, key picker)
//! on top. Slow work runs on a worker thread and network work on two more;
//! the main thread only handles input, ticks Ruffle and draws.

#![feature(alloc_error_hook)]

#[cfg(not(target_os = "vita"))]
mod assets;
mod backends;
mod catalog;
mod config;
mod fetcher;
mod input;
mod keys;
mod library;
mod net;
mod platform;
mod screens;
mod script;
mod session;
mod swfinfo;
mod thumbs;
mod ui;
mod worker;

use std::sync::Arc;
use std::time::{Duration, Instant};

use sdl2::controller::GameController;
use sdl2::event::Event;

use config::{Settings, Sort};
use input::{Input, InputEvent};
use library::Library;
use platform::{SCREEN_H, SCREEN_W};
use fetcher::Fetcher;
use screens::explore::{ExploreAction, ExploreScreen};
use screens::library::{LibAction, LibraryScreen};
use screens::{Tab, ThumbCache};
use screens::loading::LoadingScreen;
use screens::pause::{PauseAction, PauseMenu};
use screens::settings::{SettingsResult, SettingsScreen};
use session::{Session, Signal};
use thumbs::ThumbKind;
use ui::{Gfx, Toast};
use worker::{Done, Job, Worker};

#[cfg(target_os = "vita")]
#[link(name = "SDL2", kind = "static")]
#[link(name = "vitaGL", kind = "static")]
#[link(name = "stdc++", kind = "static")]
#[link(name = "vitashark", kind = "static")]
#[link(name = "SceShaccCg_stub", kind = "static")]
#[link(name = "mathneon", kind = "static")]
#[link(name = "SceShaccCgExt", kind = "static")]
#[link(name = "taihen_stub", kind = "static")]
#[link(name = "SceKernelDmacMgr_stub", kind = "static")]
#[link(name = "SceIme_stub", kind = "static")]
unsafe extern "C" {
    fn vglInitWithCustomThreshold(
        pool_size: i32,
        width: i32,
        height: i32,
        ram_threshold: i32,
        cdram_threshold: i32,
        phycont_threshold: i32,
        cdlg_threshold: i32,
        msaa: u32,
    ) -> bool;
    fn vglSetSemanticBindingMode(mode: u32);
    fn vglSetParamBufferSize(size: u32);
    fn vglSetVertexAttribPoolSize(main_size: u32, aux_size: u32);
    fn vglUseCachedMem(r#use: bool);
    fn vglUseTripleBuffering(usage: bool);
}

/// Most of the Vita's memory goes to newlib's heap (Ruffle + game assets).
#[cfg(target_os = "vita")]
#[used]
#[unsafe(export_name = "_newlib_heap_size_user")]
pub static NEWLIB_HEAP_SIZE_USER: u32 = 240 * 1024 * 1024;

fn main() {
    let body = || {
        if let Err(e) = run() {
            tracing::error!("Fatal: {e:#}");
            eprintln!("Fatal: {e:#}");
        }
    };
    // Ruffle recurses deeply on complex display lists and AVM call chains;
    // the Vita's default main-thread stack is far too small for that.
    // (Desktop keeps the real main thread: macOS only allows windows there.)
    #[cfg(target_os = "vita")]
    {
        // Threads spawned by libraries without an explicit stack size would
        // otherwise ask for 2 MiB each, which the Vita often can't provide.
        unsafe { std::env::set_var("RUST_MIN_STACK", "262144") };
        let handle = std::thread::Builder::new()
            .name("rufflevita-main".into())
            .stack_size(4 * 1024 * 1024)
            .spawn(body)
            .expect("couldn't spawn main thread");
        let _ = handle.join();
    }
    #[cfg(not(target_os = "vita"))]
    body();
}

/// Collapses runs of identical log lines (ignoring the timestamp) into one
/// line plus a repeat count. Some games trigger the same error every frame,
/// and each line is a memory card write.
#[cfg(target_os = "vita")]
struct DedupWriter<W: std::io::Write> {
    inner: W,
    last: String,
    repeats: u32,
}

#[cfg(target_os = "vita")]
impl<W: std::io::Write> DedupWriter<W> {
    fn new(inner: W) -> Self {
        Self { inner, last: String::new(), repeats: 0 }
    }
}

#[cfg(target_os = "vita")]
impl<W: std::io::Write> std::io::Write for DedupWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let line = String::from_utf8_lossy(buf);
        // Everything after the timestamp identifies the message.
        let body = line.split_once(' ').map(|(_, rest)| rest).unwrap_or(&line).to_owned();
        if body == self.last {
            self.repeats += 1;
            return Ok(buf.len());
        }
        if self.repeats > 0 {
            writeln!(self.inner, "    (previous line repeated {} more times)", self.repeats)?;
            self.repeats = 0;
        }
        self.last = body;
        self.inner.write_all(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn init_logging() {
    use tracing_subscriber::EnvFilter;
    #[cfg(target_os = "vita")]
    {
        // Warnings and our own messages only: every log line is a memory card write.
        let filter = EnvFilter::builder().parse_lossy("error,rufflevita=info");
        let path = platform::data_dir().join("log.txt");
        if let Ok(file) = std::fs::File::create(path) {
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_ansi(false)
                .with_writer(std::sync::Mutex::new(DedupWriter::new(file)))
                .init();
        }
    }
    #[cfg(not(target_os = "vita"))]
    {
        let spec = std::env::var("RUFFLEVITA_LOG")
            .unwrap_or_else(|_| "warn,rufflevita=info,avm_trace=info,avm_stub=off".into());
        tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::builder().parse_lossy(spec))
            .init();
    }
    std::panic::set_hook(Box::new(|info| {
        tracing::error!("Panic: {info}");
        eprintln!("Panic: {info}");
    }));
}

fn run() -> anyhow::Result<()> {
    let migrated = platform::migrate_old_data();
    platform::ensure_dirs();
    let autorun = platform::apply_autorun();
    init_logging();
    if !autorun.is_empty() {
        tracing::info!("autorun.txt set {}", autorun.join(", "));
    }
    // Out of memory aborts the app; say so in the log first.
    std::alloc::set_alloc_error_hook(|layout| {
        let memory = platform::memory_summary().unwrap_or_default();
        tracing::error!("Out of memory allocating {} bytes \u{b7} {memory}", layout.size());
    });
    let _ = ruffle_render_glow::MEMORY_PROBE.set(platform::memory_summary);
    platform::init_hardware();
    tracing::info!("RuffleVita {} starting", env!("CARGO_PKG_VERSION"));
    if let Some(migrated) = migrated {
        tracing::info!("{migrated}");
    }

    sdl2::hint::set("SDL_TOUCH_MOUSE_EVENTS", "0");
    #[cfg(not(target_os = "vita"))]
    let hidden = std::env::var_os("RUFFLEVITA_HIDDEN").is_some();
    #[cfg(not(target_os = "vita"))]
    if hidden {
        // Benchmarks and scripted runs: no window, no Dock icon, no focus
        // stealing. Keep the main thread on the performance cores so the
        // numbers match a visible window.
        sdl2::hint::set("SDL_MAC_BACKGROUND_APP", "1");
        platform::keep_foreground_priority();
    }
    sdl2::hint::set("SDL_MOUSE_TOUCH_EVENTS", "0");
    let sdl = sdl2::init().map_err(anyhow::Error::msg)?;
    let video = sdl.video().map_err(anyhow::Error::msg)?;
    let audio = sdl.audio().map_err(anyhow::Error::msg)?;
    let gc = sdl.game_controller().map_err(anyhow::Error::msg)?;

    #[cfg(target_os = "vita")]
    unsafe {
        // SDL's default vitaGL setup doesn't suit Ruffle; configure it first.
        vglSetSemanticBindingMode(2); // VGL_MODE_POSTPONED
        vglUseCachedMem(false);
        vglUseTripleBuffering(false);
        vglSetParamBufferSize(4 * 1024 * 1024);
        // Every VAO gets its own pool for constant attribute values, 64 KiB by
        // default. The renderer makes one VAO per shape draw and never sets
        // constant attributes (reset_vao only reserves 256 bytes), so at the
        // default a game with thousands of shapes ran the Vita out of memory.
        vglSetVertexAttribPoolSize(256 * 1024, 1024);
        vglInitWithCustomThreshold(0, SCREEN_W as i32, SCREEN_H as i32, 4 * 1024 * 1024, 0, 0, 0, 0);
    }

    let gl_attr = video.gl_attr();
    #[cfg(target_os = "macos")]
    {
        gl_attr.set_context_profile(sdl2::video::GLProfile::Core);
        gl_attr.set_context_version(3, 2);
        gl_attr.set_context_flags().forward_compatible().set();
    }
    #[cfg(not(target_os = "macos"))]
    {
        gl_attr.set_context_profile(sdl2::video::GLProfile::GLES);
        gl_attr.set_context_version(2, 0);
    }
    gl_attr.set_stencil_size(8);
    gl_attr.set_double_buffer(true);

    let mut builder = video.window("RuffleVita", SCREEN_W, SCREEN_H);
    builder.opengl().position_centered();
    #[cfg(not(target_os = "vita"))]
    if hidden {
        builder.hidden();
    }
    let window = builder.build()?;
    let gl_context = window.gl_create_context().map_err(anyhow::Error::msg)?;
    window.gl_make_current(&gl_context).map_err(anyhow::Error::msg)?;
    let gl = Arc::new(unsafe {
        glow::Context::from_loader_function(|s| video.gl_get_proc_address(s) as *const _)
    });

    let settings = Settings::load();
    let _ = video.gl_set_swap_interval(if settings.vsync { 1 } else { 0 });
    if std::env::var_os("RUFFLEVITA_BENCH").is_some() {
        let _ = video.gl_set_swap_interval(0);
    }

    let mut gfx = Gfx::new(gl.clone(), SCREEN_W, SCREEN_H)?;
    #[cfg(not(target_os = "vita"))]
    if let Some(dir) = std::env::var_os("RUFFLEVITA_RENDER_ASSETS") {
        assets::render_all(&mut gfx, &gl, std::path::Path::new(&dir));
        return Ok(());
    }
    let mut controllers = Vec::new();
    for i in 0..gc.num_joysticks().unwrap_or(0) {
        if gc.is_game_controller(i) {
            if let Ok(c) = gc.open(i) {
                controllers.push(c);
            }
        }
    }

    let mut event_pump = sdl.event_pump().map_err(anyhow::Error::msg)?;
    let mut app = App::new(gl, gfx, settings, window, video, audio, gc, controllers);
    // Development: open a game straight away (`RUFFLEVITA_AUTOSTART=<file in the games folder>`).
    if let Ok(game) = std::env::var("RUFFLEVITA_AUTOSTART") {
        match app.lib.index_of(&game) {
            Some(i) => app.start_game(i),
            None => tracing::warn!("RUFFLEVITA_AUTOSTART: no game {game:?}"),
        }
    }
    app.run(&mut event_pump);
    drop(gl_context);
    Ok(())
}

enum Mode {
    Library,
    Loading(LoadingScreen),
    Playing,
}

enum Modal {
    Pause(PauseMenu),
    Settings { screen: SettingsScreen, key: String, original: config::Profile, from_pause: bool },
}

struct App {
    gl: Arc<glow::Context>,
    gfx: Gfx,
    window: sdl2::video::Window,
    video: sdl2::VideoSubsystem,
    audio: sdl2::AudioSubsystem,
    gc: sdl2::GameControllerSubsystem,
    controllers: Vec<GameController>,
    input: Input,
    worker: Worker,
    lib: Library,
    settings: Settings,
    thumbs: ThumbCache,
    lib_screen: LibraryScreen,
    explore: ExploreScreen,
    /// Started the first time Explore opens.
    fetcher: Option<Fetcher>,
    tab: Tab,
    mode: Mode,
    session: Option<Session>,
    modal: Option<Modal>,
    toast: Option<Toast>,
    analyzing: usize,
    capture_cover: bool,
    /// Keep drawing the UI at full rate until this instant.
    busy_until: Instant,
    last_draw: Instant,
    script: Option<script::Script>,
    recorder: Option<script::Recorder>,
}

impl App {
    #[allow(clippy::too_many_arguments)]
    fn new(
        gl: Arc<glow::Context>,
        gfx: Gfx,
        settings: Settings,
        window: sdl2::video::Window,
        video: sdl2::VideoSubsystem,
        audio: sdl2::AudioSubsystem,
        gc: sdl2::GameControllerSubsystem,
        controllers: Vec<GameController>,
    ) -> Self {
        let worker = Worker::spawn();
        let mut lib = Library::load();
        lib.sort(settings.sort);
        let mut app = App {
            gl,
            gfx,
            window,
            video,
            audio,
            gc,
            controllers,
            input: Input::new(),
            worker,
            lib,
            settings,
            thumbs: ThumbCache::new(),
            lib_screen: LibraryScreen::new(),
            explore: ExploreScreen::new(),
            fetcher: None,
            tab: Tab::Library,
            mode: Mode::Library,
            session: None,
            modal: None,
            toast: None,
            analyzing: 0,
            capture_cover: false,
            busy_until: Instant::now() + Duration::from_secs(1),
            last_draw: Instant::now(),
            script: script::Script::from_env(),
            recorder: None,
        };
        if let Some(i) = app.settings.last_game.as_deref().and_then(|k| app.lib.index_of(k)) {
            app.lib_screen.select(i, &app.lib);
        }
        app.queue_analysis();
        app
    }

    fn queue_analysis(&mut self) {
        self.worker.cancel_analysis();
        self.lib.queue_analysis(&self.worker);
        self.analyzing = self
            .lib
            .games
            .iter()
            .filter(|g| {
                (g.db.info.is_none() && g.db.error.is_none())
                    || (!g.db.has_thumb && !g.db.cover_checked && g.db.error.is_none())
            })
            .count();
    }

    fn run(&mut self, pump: &mut sdl2::EventPump) {
        let mut last = Instant::now();
        loop {
            let now = Instant::now();
            let dt = (now - last).as_secs_f32().min(0.1);
            last = now;

            for event in pump.poll_iter() {
                match event {
                    Event::ControllerDeviceAdded { which, .. } => {
                        if let Ok(c) = self.gc.open(which) {
                            self.controllers.push(c);
                        }
                    }
                    Event::ControllerDeviceRemoved { which, .. } => {
                        self.controllers.retain(|c| c.instance_id() != which);
                    }
                    _ => {}
                }
                self.input.handle_sdl(&event);
            }

            let mut shot = None;
            if let Some(s) = &mut self.script {
                let game_frame = self.session.as_ref().filter(|_| matches!(self.mode, Mode::Playing)).map(|s| s.frames);
                match s.step(&mut self.input, game_frame) {
                    script::Output::Screenshot(p) => shot = Some(p),
                    script::Output::Record(Some(d)) => self.recorder = Some(script::Recorder::new(&d)),
                    script::Output::Record(None) => {
                        if let Some(r) = self.recorder.take() {
                            r.finish();
                        }
                    }
                    script::Output::Quit => self.input.quit = true,
                    script::Output::None => {}
                }
                self.busy_until = Instant::now() + Duration::from_secs(1);
            }

            if self.input.quit {
                break;
            }
            let got_results = self.handle_worker_results();

            let live = matches!(self.mode, Mode::Playing) && self.modal.is_none();
            let events = self.input.drain(!live);
            if live {
                self.frame_game(&events, dt);
                let shot = match shot {
                    None if self.recorder.as_ref().is_some_and(|r| r.due()) => Some(String::new()),
                    s => s,
                };
                if let Some(p) = shot {
                    // The game frame may not have been redrawn this iteration.
                    if let Some(s) = &mut self.session {
                        s.render();
                        s.draw_overlay(&mut self.gfx);
                        self.gfx.flush();
                    }
                    if p.is_empty() {
                        if let Some(r) = &mut self.recorder {
                            r.capture(&self.gl, SCREEN_W, SCREEN_H);
                        }
                    } else {
                        script::save_screenshot(&self.gl, SCREEN_W, SCREEN_H, &p);
                    }
                }
                continue;
            }

            if !events.is_empty() || got_results {
                self.busy_until = Instant::now() + Duration::from_millis(600);
            }
            self.update_ui(&events, dt);

            // In the library with nothing moving, drop to ~2 fps (clock and
            // battery still update) instead of burning battery at 60.
            let screen_busy = match self.tab {
                Tab::Library => self.lib_screen.animating(),
                Tab::Explore => self.explore.animating() || self.explore.searching,
            };
            let idle = matches!(self.mode, Mode::Library)
                && self.modal.is_none()
                && !screen_busy
                && self.toast.is_none()
                && Instant::now() > self.busy_until;
            let idle = idle && self.recorder.is_none();
            if idle && self.last_draw.elapsed() < Duration::from_millis(500) {
                std::thread::sleep(Duration::from_millis(16));
                continue;
            }
            self.draw_ui();
            if let Some(p) = shot {
                script::save_screenshot(&self.gl, SCREEN_W, SCREEN_H, &p);
            }
            if let Some(r) = self.recorder.as_mut().filter(|r| r.due()) {
                r.capture(&self.gl, SCREEN_W, SCREEN_H);
            }
            self.window.gl_swap_window();
            self.last_draw = Instant::now();
        }
        self.session = None;
        self.lib.save_if_dirty();
        self.settings.save();
        self.explore.save();
    }

    // ------------------------------------------------------------ worker

    fn handle_worker_results(&mut self) -> bool {
        let net = self.fetcher.as_ref().map(|f| f.poll()).unwrap_or_default();
        let mut any = !net.is_empty();
        for done in net {
            let downloaded = matches!(&done, fetcher::Done::Downloaded { result: Ok(_), .. });
            if let Some(msg) = self.explore.on_result(done, &self.gfx, &self.lib) {
                self.toast = Some(Toast::lasting(msg, 3000));
            }
            if downloaded {
                self.rescan_library();
                self.explore.refresh_owned(&self.lib);
                self.explore.save();
            }
        }
        let results = self.worker.poll();
        any |= !results.is_empty();
        for done in results {
            match done {
                Done::Info { key, info } => self.lib.update(&key, |db| match info {
                    Ok(i) => db.info = Some(i),
                    Err(e) => db.error = Some(e),
                }),
                Done::Analyzed { key, cover_checked } => {
                    self.analyzing = self.analyzing.saturating_sub(1);
                    if cover_checked {
                        self.lib.update(&key, |db| db.cover_checked = true);
                    }
                    if self.analyzing == 0 {
                        self.lib.save_if_dirty();
                    }
                }
                Done::Thumb { key, thumb } => {
                    let kind = thumb.as_ref().map(|(_, k)| *k);
                    self.lib.update(&key, |db| {
                        db.has_thumb = kind.is_some();
                        db.screenshot = kind == Some(ThumbKind::Screenshot);
                    });
                    self.thumbs.insert(&self.gfx, &key, thumb);
                }
                Done::MovieProgress(p) => {
                    if let Mode::Loading(ls) = &mut self.mode {
                        ls.progress = p;
                    }
                }
                Done::Movie(result) => self.movie_loaded(result),
            }
        }
        any
    }

    // ------------------------------------------------------------- flow

    fn start_game(&mut self, index: usize) {
        let Some(game) = self.lib.games.get(index) else { return };
        let (key, name, path) = (game.key.clone(), game.title().to_owned(), game.path.clone());
        // Free the previous game's memory before loading the next one.
        self.session = None;
        self.modal = None;
        self.settings.last_game = Some(key.clone());
        self.settings.save();
        let now = platform::now_unix();
        self.lib.update(&key, |db| {
            db.last_played = now;
            db.play_count += 1;
        });
        self.lib.save_if_dirty();
        self.worker.set_throttled(true);
        let url = backends::navigator::path_to_url(&path);
        self.worker.submit(Job::LoadMovie { path, url });
        self.mode = Mode::Loading(LoadingScreen::new(key, name));
        self.input.clear_events();
    }

    fn movie_loaded(&mut self, result: Result<ruffle_core::tag_utils::SwfMovie, String>) {
        let Mode::Loading(ls) = &mut self.mode else { return };
        let movie = match result {
            Ok(m) => m,
            Err(e) => {
                tracing::error!("Couldn't load {}: {e}", ls.key);
                ls.error = Some(e);
                return;
            }
        };
        let key = ls.key.clone();
        let name = ls.name.clone();
        let url = self
            .lib
            .games
            .iter()
            .find(|g| g.key == key)
            .map(|g| backends::navigator::path_to_url(&g.path))
            .unwrap_or_default();
        let profile = self.settings.load_profile(&key);
        match Session::new(self.gl.clone(), &self.audio, &self.window, movie, &url, key.clone(), name, profile) {
            Ok(mut s) => {
                // Don't overwrite a cover the user picked.
                s.auto_cover_taken = self.lib.games.iter().any(|g| g.key == key && g.db.screenshot);
                self.session = Some(s);
                self.mode = Mode::Playing;
                self.input.clear_events();
            }
            Err(e) => {
                tracing::error!("Couldn't start {key}: {e}");
                ls.error = Some(e);
            }
        }
    }

    fn quit_to_library(&mut self) {
        let key = self.session.as_ref().map(|s| s.key.clone());
        self.session = None;
        self.modal = None;
        self.mode = Mode::Library;
        self.worker.set_throttled(false);
        self.lib.sort(self.settings.sort);
        if let Some(i) = key.and_then(|k| self.lib.index_of(&k)) {
            self.lib_screen.select(i, &self.lib);
        }
        self.lib.save_if_dirty();
        self.input.clear_events();
    }

    fn open_pause(&mut self) {
        if let Some(s) = &mut self.session {
            s.release_all();
            s.set_running(false);
        }
        self.modal = Some(Modal::Pause(PauseMenu::new()));
        self.input.clear_events();
    }

    fn resume(&mut self) {
        self.modal = None;
        if let Some(s) = &mut self.session {
            s.set_running(true);
        }
        self.input.clear_events();
    }

    fn open_settings(&mut self, key: String, name: String, from_pause: bool) {
        let profile = match &self.session {
            Some(s) if s.key == key => s.profile.clone(),
            _ => self.settings.load_profile(&key),
        };
        self.modal = Some(Modal::Settings {
            screen: SettingsScreen::new(name, profile.clone(), self.settings.vsync),
            key,
            original: profile,
            from_pause,
        });
    }

    fn set_vsync(&mut self, on: bool) {
        if self.settings.vsync != on {
            self.settings.vsync = on;
            self.settings.save();
            let _ = self.video.gl_set_swap_interval(if on { 1 } else { 0 });
        }
    }

    // ------------------------------------------------------------- game

    fn frame_game(&mut self, events: &[InputEvent], dt: f32) {
        let Some(session) = self.session.as_mut() else { return };
        let mut open_menu = false;
        for ev in events {
            if let Signal::OpenMenu = session.handle(ev, &self.input) {
                open_menu = true;
            }
        }
        session.update_sticks(&self.input, dt);
        if open_menu {
            self.open_pause();
            return;
        }
        session.tick();
        let missing = session.take_missing();
        if !missing.is_empty() {
            tracing::warn!("{} asked for missing files: {missing:?}", session.key);
            self.toast = Some(Toast::lasting(
                format!("Missing {} \u{2014} copy it next to this game", missing.join(", ")),
                6000,
            ));
        }

        let toast = self.toast.as_ref().is_some_and(|t| t.alive());
        if session.needs_render() || session.cursor_visible() || toast {
            session.render();
            if !session.auto_cover_taken && session.started.elapsed() > Duration::from_secs(25) {
                session.auto_cover_taken = true;
                if let Some(img) = session.capture(&self.gl) {
                    self.worker.submit(Job::SaveScreenshot { key: session.key.clone(), img });
                }
            }
            let present = Instant::now();
            let present_zone = ruffle_core::rv_prof::zone(ruffle_core::rv_prof::Zone::Present);
            session.draw_overlay(&mut self.gfx);
            if let Some(t) = &self.toast {
                t.draw(&mut self.gfx);
            }
            self.gfx.flush();
            self.window.gl_swap_window();
            drop(present_zone);
            session.add_present_time(present.elapsed());
            if !toast {
                self.toast = None;
            }
        } else if session.benchmarking() {
            // Timedemo: next frame right away.
        } else {
            // Always yield at least 1 ms: games with 0-1 ms setInterval timers
            // would otherwise keep us spinning. Cap at 8 ms for input latency.
            let wait = session
                .time_til_next_frame()
                .clamp(Duration::from_millis(1), Duration::from_millis(8));
            std::thread::sleep(wait);
        }
    }

    // --------------------------------------------------------------- UI

    fn update_ui(&mut self, events: &[InputEvent], dt: f32) {
        if self.toast.as_ref().is_some_and(|t| !t.alive()) {
            self.toast = None;
        }
        match self.modal.take() {
            Some(Modal::Pause(mut menu)) => match menu.update(events) {
                PauseAction::None => self.modal = Some(Modal::Pause(menu)),
                PauseAction::Resume => self.resume(),
                PauseAction::Settings => {
                    if let Some(s) = &self.session {
                        let (k, n) = (s.key.clone(), s.name.clone());
                        self.open_settings(k, n, true);
                    }
                }
                PauseAction::SetCover => {
                    self.capture_cover = true;
                    self.modal = Some(Modal::Pause(menu));
                }
                PauseAction::Restart => {
                    let key = self.session.as_ref().map(|s| s.key.clone());
                    if let Some(i) = key.and_then(|k| self.lib.index_of(&k)) {
                        self.start_game(i);
                    }
                }
                PauseAction::Quit => self.quit_to_library(),
            },
            Some(Modal::Settings { mut screen, key, original, from_pause }) => {
                let result = screen.update(events, dt);
                if let Some(s) = self.session.as_mut().filter(|s| s.key == key) {
                    if matches!(result, SettingsResult::Changed | SettingsResult::Close) {
                        s.apply_profile(screen.profile.clone());
                    }
                }
                match result {
                    SettingsResult::Close => {
                        if screen.profile != original {
                            self.settings.save_profile(&key, &screen.profile);
                            self.lib_screen.invalidate_profile(&key);
                        }
                        self.set_vsync(screen.vsync);
                        self.modal = from_pause.then(|| Modal::Pause(PauseMenu::new()));
                        self.input.clear_events();
                    }
                    SettingsResult::MakeDefault => {
                        self.settings.default_profile = screen.profile.clone();
                        self.settings.save();
                        self.lib_screen.invalidate_all_profiles();
                        self.toast = Some(Toast::new("Saved as the default for new games"));
                        self.modal = Some(Modal::Settings { screen, key, original, from_pause });
                    }
                    _ => self.modal = Some(Modal::Settings { screen, key, original, from_pause }),
                }
            }
            None => match &mut self.mode {
                Mode::Library if self.tab == Tab::Explore => self.update_explore(events, dt),
                Mode::Library => match self.lib_screen.update(events, &self.lib, dt) {
                    LibAction::None => {}
                    LibAction::Play(i) => self.start_game(i),
                    LibAction::Settings(i) => {
                        let g = &self.lib.games[i];
                        let (k, n) = (g.key.clone(), g.title().to_owned());
                        self.open_settings(k, n, false);
                    }
                    LibAction::ToggleSort => {
                        let key = self.lib.games.get(self.lib_screen.selected).map(|g| g.key.clone());
                        self.settings.sort = match self.settings.sort {
                            Sort::Recent => Sort::Name,
                            Sort::Name => Sort::Recent,
                        };
                        self.settings.save();
                        self.lib.sort(self.settings.sort);
                        if let Some(i) = key.and_then(|k| self.lib.index_of(&k)) {
                            self.lib_screen.select(i, &self.lib);
                        }
                    }
                    LibAction::SwitchTab(tab) => self.switch_tab(tab),
                    LibAction::Rescan => {
                        self.rescan_library();
                        let n = self.lib.games.len();
                        self.toast = Some(Toast::new(match n {
                            0 => "No games found".to_owned(),
                            1 => "Found 1 game".to_owned(),
                            n => format!("Found {n} games"),
                        }));
                    }
                },
                Mode::Loading(ls) => {
                    if ls.update(events) {
                        self.mode = Mode::Library;
                        self.worker.set_throttled(false);
                        self.input.clear_events();
                    }
                }
                Mode::Playing => {}
            },
        }
    }

    /// Re-reads the games folder, keeping the selection.
    fn rescan_library(&mut self) {
        let key = self.lib.games.get(self.lib_screen.selected).map(|g| g.key.clone());
        self.lib.rescan();
        self.lib.sort(self.settings.sort);
        self.queue_analysis();
        let i = key.and_then(|k| self.lib.index_of(&k)).unwrap_or(0);
        self.lib_screen.select(i, &self.lib);
    }

    fn switch_tab(&mut self, tab: Tab) {
        if tab == Tab::Explore {
            let fetcher = self.fetcher.get_or_insert_with(Fetcher::spawn);
            self.explore.open(fetcher, &self.lib);
        }
        self.tab = tab;
        self.input.clear_events();
    }

    fn update_explore(&mut self, events: &[InputEvent], dt: f32) {
        let Some(fetcher) = self.fetcher.as_ref() else { return };
        if self.explore.searching {
            // The on-screen keyboard owns the buttons while it's open.
            #[cfg(target_os = "vita")]
            if !self.video.text_input().is_screen_keyboard_shown(&self.window) {
                self.explore.searching = false;
            }
            let typed: Vec<InputEvent> = events
                .iter()
                .filter(|e| matches!(e, InputEvent::Text(_) | InputEvent::Backspace | InputEvent::Enter))
                .cloned()
                .collect();
            self.explore.update(&typed, fetcher, dt);
            if !self.explore.searching {
                self.video.text_input().stop();
                self.input.typing = false;
                self.input.clear_events();
            }
            return;
        }
        match self.explore.update(events, fetcher, dt) {
            ExploreAction::None => {}
            ExploreAction::SwitchTab(tab) => self.switch_tab(tab),
            ExploreAction::StartSearch => {
                self.video.text_input().start();
                self.input.typing = true;
            }
            ExploreAction::Download { slug, file_name } => {
                fetcher.download(slug, platform::games_dir().join(file_name));
            }
            ExploreAction::CancelDownload => fetcher.cancel_download(),
            ExploreAction::Play(file_name) => {
                let found = self.lib.games.iter().position(|g| {
                    g.path.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(&file_name))
                });
                match found {
                    Some(i) => {
                        self.tab = Tab::Library;
                        self.lib_screen.select(i, &self.lib);
                        self.start_game(i);
                    }
                    None => self.toast = Some(Toast::new("Refresh the library to see this game")),
                }
            }
        }
    }

    fn draw_ui(&mut self) {
        let g = &mut self.gfx;
        match &self.mode {
            Mode::Playing => {
                // The paused game stays visible behind menus (and previews
                // display changes live).
                if let Some(s) = &mut self.session {
                    s.render();
                    if self.capture_cover {
                        self.capture_cover = false;
                        match s.capture(&self.gl) {
                            Some(img) => {
                                s.auto_cover_taken = true;
                                self.worker.submit(Job::SaveScreenshot { key: s.key.clone(), img });
                                self.toast = Some(Toast::new("Cover image updated"));
                            }
                            None => self.toast = Some(Toast::new("That screen is blank; try another")),
                        }
                    }
                }
            }
            Mode::Library => {
                g.clear(ui::theme::BLUE);
                match (self.tab, &self.fetcher) {
                    (Tab::Explore, Some(f)) => self.explore.draw(g, f),
                    _ => self.lib_screen.draw(g, &self.lib, &self.settings, &mut self.thumbs, &self.worker, self.analyzing > 0),
                }
            }
            Mode::Loading(ls) => {
                g.clear(ui::theme::BLUE);
                ls.draw(g, &self.thumbs);
            }
        }
        let g = &mut self.gfx;
        match &mut self.modal {
            Some(Modal::Pause(menu)) => {
                let (name, fps) = self.session.as_ref().map(|s| (s.name.clone(), s.fps)).unwrap_or_default();
                menu.draw(g, &name, fps);
            }
            Some(Modal::Settings { screen, .. }) => screen.draw(g),
            None => {}
        }
        if let Some(t) = &self.toast {
            t.draw(g);
        }
        g.flush();
    }
}
