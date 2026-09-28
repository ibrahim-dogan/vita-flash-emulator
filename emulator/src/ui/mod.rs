//! FlashVita's UI toolkit: theme, shared widgets and animation helpers.

pub mod gfx;
pub mod icons;

pub use gfx::{Color, FontId, Gfx, Rect, Texture};
pub use icons::Icon;

use crate::input::Btn;
use crate::platform::{SCREEN_H, SCREEN_W};

pub mod theme {
    use super::Color;

    pub const BG_TOP: Color = Color::hex(0x121829);
    pub const BG_BOTTOM: Color = Color::hex(0x080B13);
    pub const PANEL: Color = Color::hex(0x161D2E);
    pub const PANEL_HI: Color = Color::hex(0x1E2740);
    pub const LINE: Color = Color::hex(0x273150);
    pub const TEXT: Color = Color::hex(0xF1F4FB);
    pub const DIM: Color = Color::hex(0x98A2B9);
    pub const FAINT: Color = Color::hex(0x5C6680);
    pub const ACCENT: Color = Color::hex(0x5B8CFF);
    pub const ACCENT2: Color = Color::hex(0x9A6BFF);
    pub const OK: Color = Color::hex(0x3DD68C);
    pub const WARN: Color = Color::hex(0xFFB547);
    pub const DANGER: Color = Color::hex(0xFF5C7A);
    pub const SHADOW: Color = Color { r: 0.0, g: 0.0, b: 0.0, a: 0.45 };

    // PlayStation face button colours.
    pub const CROSS: Color = Color::hex(0x7FA8FF);
    pub const CIRCLE: Color = Color::hex(0xFF6F86);
    pub const SQUARE: Color = Color::hex(0xF28FD9);
    pub const TRIANGLE: Color = Color::hex(0x42D9B4);

    pub const RADIUS: f32 = 12.0;
}

/// Exponential smoothing towards a target, frame-rate independent.
pub fn approach(current: f32, target: f32, dt: f32, speed: f32) -> f32 {
    let t = 1.0 - (-speed * dt).exp();
    let v = current + (target - current) * t;
    if (v - target).abs() < 0.05 { target } else { v }
}

/// The app's backdrop: a gradient with two soft coloured glows.
pub fn background(g: &mut Gfx) {
    let full = Rect::new(0.0, 0.0, SCREEN_W as f32, SCREEN_H as f32);
    g.rect_v(full, theme::BG_TOP, theme::BG_BOTTOM);
    g.glow(120.0, 0.0, 420.0, 220.0, theme::ACCENT.alpha(0.16));
    g.glow(900.0, 560.0, 380.0, 220.0, theme::ACCENT2.alpha(0.13));
}

/// Author credit, shown in the library and on the LiveArea.
pub const CREDIT: &str = "by \u{0130}brahim Do\u{011f}an";

/// The FlashVita mark: a blue-to-violet rounded tile with a play symbol.
pub fn logo(g: &mut Gfx, r: Rect) {
    let rad = (r.w * 0.28).round();
    g.shadow(r, (r.w * 0.25).max(8.0), theme::ACCENT.alpha(0.35));
    // Solid rounded ends, a gradient strip between their centres.
    g.rounded(Rect::new(r.x, r.y, 2.0 * rad, r.h), rad, theme::ACCENT);
    g.rounded(Rect::new(r.right() - 2.0 * rad, r.y, 2.0 * rad, r.h), rad, theme::ACCENT2);
    g.rect_h(Rect::new(r.x + rad, r.y, r.w - 2.0 * rad, r.h), theme::ACCENT, theme::ACCENT2);
    let s = (r.w * 0.56).round();
    let (x, y) = (r.x + (r.w - s) * 0.5 + r.w * 0.03, r.y + (r.h - s) * 0.5);
    g.icon(Icon::Play, x, y + r.h * 0.03, s, Color::hex(0x1A1040).alpha(0.35));
    g.icon(Icon::Play, x, y, s, Color::hex(0xFFFFFF));
}

/// A raised card.
pub fn panel(g: &mut Gfx, r: Rect) {
    g.shadow(r, 16.0, theme::SHADOW);
    g.rounded(r, theme::RADIUS, theme::PANEL);
}

/// Draws the glyph for a Vita button centred at (`cx`, `cy`); returns its width.
pub fn button_glyph(g: &mut Gfx, b: Btn, cx: f32, cy: f32, size: f32) -> f32 {
    let face = |g: &mut Gfx, icon: Icon, color: Color| {
        g.circle(cx, cy, size * 0.5, Color::hex(0x0C0F18).alpha(0.9));
        g.icon(icon, cx - size * 0.5, cy - size * 0.5, size, color);
        size
    };
    match b {
        Btn::Cross => face(g, Icon::Cross, theme::CROSS),
        Btn::Circle => face(g, Icon::CircleBtn, theme::CIRCLE),
        Btn::Square => face(g, Icon::Square, theme::SQUARE),
        Btn::Triangle => face(g, Icon::Triangle, theme::TRIANGLE),
        Btn::Up | Btn::Down | Btn::Left | Btn::Right => {
            g.icon(Icon::DPad, cx - size * 0.5, cy - size * 0.5, size, theme::DIM);
            // Highlight the relevant arm.
            let (dx, dy) = match b {
                Btn::Up => (0.0, -0.28),
                Btn::Down => (0.0, 0.28),
                Btn::Left => (-0.28, 0.0),
                _ => (0.28, 0.0),
            };
            let s = size * 0.2;
            g.rounded(
                Rect::new(cx + dx * size - s * 0.5, cy + dy * size - s * 0.5, s, s),
                4.0,
                theme::TEXT,
            );
            size
        }
        Btn::L | Btn::R | Btn::Start | Btn::Select => {
            let label = match b {
                Btn::L => "L",
                Btn::R => "R",
                Btn::Start => "START",
                _ => "SELECT",
            };
            let px = if label.len() > 1 { size * 0.42 } else { size * 0.55 };
            let tw = g.measure(FontId::Bold, px, label);
            let w = (tw + size * 0.6).max(size * 1.1);
            let r = Rect::new(cx - w * 0.5, cy - size * 0.42, w, size * 0.84);
            g.rounded(r, size * 0.42, Color::hex(0x0C0F18).alpha(0.9));
            g.rounded_outline(r, size * 0.42, 1.0, theme::FAINT, Color::hex(0x0C0F18).alpha(0.0));
            g.text_mid_center(FontId::Bold, px, cx, cy, theme::DIM, label);
            w
        }
    }
}

/// Width a glyph will take, for right-aligned layouts.
pub fn button_glyph_width(g: &mut Gfx, b: Btn, size: f32) -> f32 {
    match b {
        Btn::L | Btn::R | Btn::Start | Btn::Select => {
            let label = match b {
                Btn::L => "L",
                Btn::R => "R",
                Btn::Start => "START",
                _ => "SELECT",
            };
            let px = if label.len() > 1 { size * 0.42 } else { size * 0.55 };
            (g.measure(FontId::Bold, px, label) + size * 0.6).max(size * 1.1)
        }
        _ => size,
    }
}

/// A footer hint: glyph + label. Returns the total width drawn.
pub fn hint(g: &mut Gfx, x: f32, cy: f32, b: Btn, label: &str) -> f32 {
    let size = 22.0;
    let gw = button_glyph_width(g, b, size);
    button_glyph(g, b, x + gw * 0.5, cy, size);
    let tw = g.text_mid(FontId::Regular, 15.0, x + gw + 8.0, cy, theme::DIM, label);
    gw + 8.0 + tw
}

pub fn hint_width(g: &mut Gfx, b: Btn, label: &str) -> f32 {
    button_glyph_width(g, b, 22.0) + 8.0 + g.measure(FontId::Regular, 15.0, label)
}

/// Bottom bar with hints laid out from the right. Returns the tap targets.
pub fn footer(g: &mut Gfx, hints: &[(Btn, &str)]) -> Vec<(Rect, Btn)> {
    let bar = Rect::new(0.0, SCREEN_H as f32 - 44.0, SCREEN_W as f32, 44.0);
    g.rect(bar, Color::hex(0x06080E).alpha(0.55));
    g.rect(Rect::new(0.0, bar.y, bar.w, 1.0), theme::LINE.alpha(0.6));
    let mut x = SCREEN_W as f32 - 24.0;
    let mut targets = Vec::new();

    for (b, label) in hints.iter().rev() {
        let w = hint_width(g, *b, label);
        x -= w;
        hint(g, x, bar.center_y(), *b, label);
        targets.push((Rect::new(x - 6.0, bar.y, w + 12.0, bar.h), *b));
        x -= 26.0;
    }
    targets
}

/// "FlashVita 1.0.0 · by İbrahim Doğan" at the left of the footer bar.
pub fn footer_credit(g: &mut Gfx) {
    let cy = SCREEN_H as f32 - 22.0;
    let credit = format!("FlashVita {} \u{00b7} {CREDIT}", env!("CARGO_PKG_VERSION"));
    g.text_mid(FontId::Regular, 13.0, 24.0, cy, theme::FAINT, &credit);
}

/// Status cluster for the top-right corner: clock and battery.
pub fn status(g: &mut Gfx, right: f32, cy: f32) {
    let (h, m) = crate::platform::local_time();
    let clock = format!("{h:02}:{m:02}");
    let mut x = right;
    if let Some((pct, charging)) = crate::platform::battery() {
        let bw = 26.0;
        let body = Rect::new(x - bw - 3.0, cy - 6.5, bw, 13.0);
        g.rounded_outline(body, 4.0, 1.5, theme::DIM, Color::hex(0).alpha(0.0));
        g.rect(Rect::new(body.right(), cy - 3.0, 2.5, 6.0), theme::DIM);
        let fill = if pct <= 15 && !charging { theme::DANGER } else if charging { theme::OK } else { theme::TEXT };
        let inner = body.inset(3.0);
        g.rect(Rect::new(inner.x, inner.y, (inner.w * pct as f32 / 100.0).round(), inner.h), fill);
        x = body.x - 10.0;
        let label = format!("{pct}%");
        let w = g.text_mid_right(FontId::Regular, 14.0, x, cy, theme::DIM, &label);
        x -= w + 14.0;
    }
    g.text_mid_right(FontId::Bold, 15.0, x, cy, theme::TEXT, &clock);
}

/// Formats a byte count as "4.2 MB".
pub fn human_size(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1024.0 * 1024.0 {
        format!("{:.1} MB", b / (1024.0 * 1024.0))
    } else {
        format!("{:.0} KB", (b / 1024.0).max(1.0))
    }
}

/// "3 days ago"-style relative time.
pub fn relative_time(then: i64, now: i64) -> String {
    let d = (now - then).max(0);
    match d {
        0..=89 => "Just now".into(),
        90..=3599 => format!("{} min ago", d / 60),
        3600..=86399 => {
            let h = d / 3600;
            if h == 1 { "1 hour ago".into() } else { format!("{h} hours ago") }
        }
        _ => {
            let days = d / 86400;
            match days {
                1 => "Yesterday".into(),
                2..=30 => format!("{days} days ago"),
                31..=364 => format!("{} months ago", days / 30),
                _ => format!("{} years ago", days / 365),
            }
        }
    }
}

/// Transient message shown at the top of the screen.
pub struct Toast {
    text: String,
    until: std::time::Instant,
}

impl Toast {
    pub fn new(text: impl Into<String>) -> Self {
        Self::lasting(text, 2200)
    }

    pub fn lasting(text: impl Into<String>, ms: u64) -> Self {
        Toast { text: text.into(), until: std::time::Instant::now() + std::time::Duration::from_millis(ms) }
    }

    pub fn alive(&self) -> bool {
        std::time::Instant::now() < self.until
    }

    pub fn draw(&self, g: &mut Gfx) {
        let left = self.until.saturating_duration_since(std::time::Instant::now()).as_secs_f32();
        let a = (left / 0.3).min(1.0);
        let w = g.measure(FontId::Bold, 15.0, &self.text) + 40.0;
        let r = Rect::new((SCREEN_W as f32 - w) * 0.5, 18.0, w, 36.0);
        g.shadow(r, 16.0, theme::SHADOW.alpha(a));
        g.rounded(r, 18.0, theme::PANEL_HI.alpha(0.97 * a));
        g.text_mid_center(FontId::Bold, 15.0, r.center_x(), r.center_y(), theme::TEXT.alpha(a), &self.text);
    }
}

/// Animated loading spinner.
pub fn spinner(g: &mut Gfx, cx: f32, cy: f32, size: f32, color: Color, t: f32) {
    g.icon_rotated(Icon::Spinner, cx, cy, size, t * 5.5, color);
}
