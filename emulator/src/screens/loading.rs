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
        ui::background(g);
        let cx = SCREEN_W as f32 * 0.5;
        let card = Rect::new(cx - 250.0, 64.0, 500.0, 230.0);
        g.rounded(card.offset(theme::SHADOW, theme::SHADOW), theme::RADIUS + 2.0, theme::INK);
        g.rounded(card, theme::RADIUS + 2.0, theme::INK);
        let inner = card.inset(theme::BORDER);
        match thumbs.get(&self.key) {
            Some(tex) => {
                g.image_contain(tex, inner, Color::hex(0xFFFFFF));
            }
            None => {
                g.push_clip(inner);
                ui::generated_cover(g, inner, &self.name, ui::cover_color(&self.key), true);
                g.pop_clip();
            }
        }
        g.round_corners(inner, theme::RADIUS - 1.0, theme::INK);

        let name = g.ellipsize(FontId::Display, 32.0, &self.name, 760.0);
        ui::shadow_text_center(g, FontId::Display, 32.0, cx, 340.0, theme::PAPER, &name);

        if let Some(err) = &self.error {
            let msg = Rect::new(cx - 330.0, 374.0, 660.0, 96.0);
            ui::card(g, msg, 12.0, theme::TOMATO);
            g.text_mid_center(FontId::Bold, 17.0, cx, msg.y + 26.0, theme::INK, "This game couldn't be started");
            let lines = g.wrap(FontId::Regular, 14.0, err, msg.w - 40.0, 2);
            for (i, l) in lines.iter().enumerate() {
                g.text_mid_center(FontId::Regular, 14.0, cx, msg.y + 52.0 + i as f32 * 20.0, theme::INK, l);
            }
            ui::footer(g, &[(Btn::Cross, "Back to library")]);
            return;
        }

        let p = self.progress.clamp(0.0, 1.0);
        let status = if p < 1.0 { "Loading\u{2026}" } else { "Starting\u{2026}" };
        ui::progress_bar(g, Rect::new(cx - 200.0, 380.0, 400.0, 34.0), p, status, &format!("{:.0}%", p * 100.0));
        let t = self.started.elapsed().as_secs_f32();
        if p >= 1.0 {
            ui::spinner(g, cx + 228.0, 397.0, 26.0, theme::PAPER, t);
        }
        g.text_mid_center(
            FontId::Bold,
            13.5,
            cx,
            SCREEN_H as f32 - 30.0,
            theme::ON_BLUE_DIM,
            "Tip: press SELECT in-game for the menu \u{00b7} L + R + START always works",
        );
    }
}
