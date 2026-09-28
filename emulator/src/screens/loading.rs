//! Shown while a movie is read and inflated on the worker thread.

use std::time::Instant;

use crate::input::{Btn, InputEvent};
use crate::platform::{SCREEN_H, SCREEN_W};
use crate::screens::ThumbCache;
use crate::ui::{self, Color, FontId, Gfx, Rect, theme};

pub struct LoadingScreen {
    pub key: String,
    pub name: String,
    pub progress: f32,
    pub error: Option<String>,
    started: Instant,
}

impl LoadingScreen {
    pub fn new(key: String, name: String) -> Self {
        Self { key, name, progress: 0.0, error: None, started: Instant::now() }
    }

    /// Returns true when the user dismisses an error.
    pub fn update(&mut self, events: &[InputEvent]) -> bool {
        self.error.is_some()
            && events.iter().any(|e| {
                matches!(e, InputEvent::Button(Btn::Cross | Btn::Circle, true))
                    || matches!(e, InputEvent::Touch(t) if t.phase == crate::input::TouchPhase::Up)
            })
    }

    pub fn draw(&self, g: &mut Gfx, thumbs: &ThumbCache) {
        let full = Rect::new(0.0, 0.0, SCREEN_W as f32, SCREEN_H as f32);
        g.rect(full, Color::hex(0x05070C));
        if let Some(tex) = thumbs.get(&self.key) {
            g.image_cover(tex, full, Color::hex(0xFFFFFF).alpha(0.22));
        }
        g.rect_v(full, Color::hex(0x05070C).alpha(0.2), Color::hex(0x05070C).alpha(0.9));

        let cx = SCREEN_W as f32 * 0.5;
        let name = g.ellipsize(FontId::Bold, 26.0, &self.name, 760.0);
        g.text_mid_center(FontId::Bold, 26.0, cx, 300.0, theme::TEXT, &name);

        if let Some(err) = &self.error {
            g.text_mid_center(FontId::Bold, 16.0, cx, 340.0, theme::DANGER, "This game couldn't be started");
            let lines = g.wrap(FontId::Regular, 14.0, err, 700.0, 3);
            for (i, l) in lines.iter().enumerate() {
                g.text_mid_center(FontId::Regular, 14.0, cx, 368.0 + i as f32 * 20.0, theme::DIM, l);
            }
            ui::footer(g, &[(Btn::Cross, "Back to library")]);
            return;
        }

        let t = self.started.elapsed().as_secs_f32();
        ui::spinner(g, cx, 230.0, 40.0, theme::ACCENT, t);
        let bar = Rect::new(cx - 160.0, 340.0, 320.0, 6.0);
        g.rounded(bar, 3.0, theme::PANEL_HI);
        let p = self.progress.clamp(0.0, 1.0);
        if p > 0.0 {
            g.rounded(Rect::new(bar.x, bar.y, (bar.w * p).max(6.0), bar.h), 3.0, theme::ACCENT);
        }
        let status = if p < 1.0 { "Loading\u{2026}" } else { "Starting\u{2026}" };
        g.text_mid_center(FontId::Regular, 14.0, cx, 368.0, theme::DIM, status);
        g.text_mid_center(
            FontId::Regular,
            13.0,
            cx,
            SCREEN_H as f32 - 30.0,
            theme::FAINT,
            "Tip: press SELECT in-game for the menu \u{00b7} L + R + START always works",
        );
    }
}
