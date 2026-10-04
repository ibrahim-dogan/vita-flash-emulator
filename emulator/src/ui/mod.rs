//! RuffleVita's UI toolkit: theme, shared widgets and animation helpers.
//!
//! The look borrows from 2000s Flash game portals: flat saturated colours,
//! thick ink outlines and hard offset shadows instead of gradients and blur.

pub mod gfx;
pub mod icons;

pub use gfx::{Color, FontId, Gfx, Pattern, Rect, Texture};
pub use icons::Icon;

use crate::input::Btn;
use crate::platform::{SCREEN_H, SCREEN_W};

pub mod theme {
    use super::Color;

    /// Screen background.
    pub const BLUE: Color = Color::hex(0x2340C8);
    /// Outlines, hard shadows, the footer band and text on light fills.
    pub const INK: Color = Color::hex(0x0F1226);
    /// Panels, and text on the blue background.
    pub const PAPER: Color = Color::hex(0xFFF8E7);
    /// A second fill on paper panels (unselected keys, tracks).
    pub const PAPER_DIM: Color = Color::hex(0xF1E5C9);
    /// Secondary text on paper.
    pub const MUTED: Color = Color::hex(0x6B6456);
    /// Tertiary text on paper.
    pub const FAINT: Color = Color::hex(0xA0957F);
    /// Secondary text on the blue background.
    pub const ON_BLUE_DIM: Color = Color::hex(0xBCC7F7);
    /// Secondary text on ink.
    pub const ON_INK_DIM: Color = Color::hex(0x8E97C9);
    /// Selection and primary actions.
    pub const SUN: Color = Color::hex(0xFFC933);
    /// Errors, warnings and destructive actions.
    pub const TOMATO: Color = Color::hex(0xFF5A3C);
    /// Done, in the library, progress.
    pub const MINT: Color = Color::hex(0x3DDC97);

    // PlayStation face button colours, drawn on ink.
    pub const CROSS: Color = Color::hex(0x7FA8FF);
    pub const CIRCLE: Color = Color::hex(0xFF6F86);
    pub const SQUARE: Color = Color::hex(0xF28FD9);
    pub const TRIANGLE: Color = Color::hex(0x42D9B4);

    /// Outline thickness of cards and buttons.
    pub const BORDER: f32 = 3.0;
    /// Offset of the hard shadow under cards.
    pub const SHADOW: f32 = 5.0;
    pub const RADIUS: f32 = 14.0;

    /// Text/icons drawn on a light accent fill (the yellow selection, the
    /// logo), kept dark in both themes. Distinct from `INK` so the dark-mode
    /// text remap leaves it alone (see `gfx::fg_remap`).
    pub const ON_ACCENT: Color = Color::hex(0x121426);

    /// Text/icon colour for a fill: `INK` on paper, `ON_ACCENT` on colour.
    pub fn ink_on(fill: Color) -> Color {
        if fill == PAPER || fill == PAPER_DIM { INK } else { ON_ACCENT }
    }

    /// Cover colours for games without art, picked by name.
    pub const COVERS: [u32; 8] = [0xFF5A3C, 0x8E5BD8, 0x20A37A, 0xE8883A, 0x3B7BE0, 0xD14B8F, 0x5C9E2E, 0xC9A227];
}

/// Exponential smoothing towards a target, frame-rate independent.
pub fn approach(current: f32, target: f32, dt: f32, speed: f32) -> f32 {
    let t = 1.0 - (-speed * dt).exp();
    let v = current + (target - current) * t;
    if (v - target).abs() < 0.05 { target } else { v }
}

/// The app's backdrop: flat blue with a faint dot grid.
pub fn background(g: &mut Gfx) {
    let full = Rect::new(0.0, 0.0, SCREEN_W as f32, SCREEN_H as f32);
    g.rect(full, theme::BLUE);
    g.pattern(Pattern::Dots, full, theme::PAPER.alpha(0.10));
}

/// Author credit, shown in the library and on the LiveArea.
pub const CREDIT: &str = "by \u{0130}brahim Do\u{011f}an";

/// A panel: ink outline, `fill` inside and a hard ink shadow.
pub fn card(g: &mut Gfx, r: Rect, radius: f32, fill: Color) {
    card_with(g, r, radius, fill, theme::BORDER, theme::SHADOW);
}

pub fn card_with(g: &mut Gfx, r: Rect, radius: f32, fill: Color, border: f32, shadow: f32) {
    if shadow > 0.0 {
        g.rounded(r.offset(shadow, shadow), radius, theme::INK);
    }
    g.rounded_outline(r, radius, border, theme::INK, fill);
}

/// Text with a hard ink shadow, for titles on the blue background.
pub fn shadow_text(g: &mut Gfx, font: FontId, px: f32, x: f32, cy: f32, color: Color, s: &str) -> f32 {
    // Flat in dark mode: the shadow only reads on the blue background.
    if !gfx::is_dark() {
        let d = (px / 11.0).clamp(2.0, 4.0).round();
        g.text_mid(font, px, x + d, cy + d, theme::INK, s);
    }
    g.text_mid(font, px, x, cy, color, s)
}

pub fn shadow_text_center(g: &mut Gfx, font: FontId, px: f32, cx: f32, cy: f32, color: Color, s: &str) {
    let w = g.measure(font, px, s);
    shadow_text(g, font, px, cx - w * 0.5, cy, color, s);
}

/// The RuffleVita mark: a sun-yellow tile with a play symbol.
pub fn logo(g: &mut Gfx, r: Rect) {
    let border = (r.w / 12.0).clamp(2.0, 6.0).round();
    let shadow = (r.w / 10.0).clamp(2.0, 10.0).round();
    card_with(g, r, (r.w * 0.26).round(), theme::SUN, border, shadow);
    let s = (r.w * 0.56).round();
    let (x, y) = (r.x + (r.w - s) * 0.5 + r.w * 0.04, r.y + (r.h - s) * 0.5);
    g.icon(Icon::Play, x, y, s, theme::ON_ACCENT);
}

/// Width of [`wordmark`] at `size`.
pub fn wordmark_width(g: &mut Gfx, size: f32) -> f32 {
    size * 1.42 + g.measure(FontId::Display, size * 0.95, "RuffleVita")
}

/// The mark plus the "RuffleVita" wordmark, vertically centred on `cy`.
/// Returns the width drawn.
pub fn wordmark(g: &mut Gfx, x: f32, cy: f32, size: f32) -> f32 {
    let logo_r = Rect::new(x, cy - size * 0.5, size, size);
    logo(g, logo_r);
    let px = size * 0.95;
    let tx = logo_r.right() + size * 0.42;
    let w = shadow_text(g, FontId::Display, px, tx, cy, theme::PAPER, "RuffleVita");
    tx + w - x
}

/// A small outlined label, e.g. "AS3" or "3.8 MB". Returns its width.
pub fn chip(g: &mut Gfx, x: f32, y: f32, text: &str, fill: Color) -> f32 {
    let w = g.measure(FontId::Bold, 13.0, text) + 18.0;
    let r = Rect::new(x, y, w.round(), 26.0);
    g.rounded_outline(r, 7.0, 2.0, theme::INK, fill);
    g.text_mid_center(FontId::Bold, 13.0, r.center_x(), r.center_y(), theme::ink_on(fill), text);
    r.w
}

/// A progress bar in a paper track; `p` is 0..=1.
pub fn progress_bar(g: &mut Gfx, r: Rect, p: f32, left: &str, right: &str) {
    card_with(g, r, 10.0, theme::PAPER, theme::BORDER, 4.0);
    let inner = r.inset(theme::BORDER);
    let w = (inner.w * p.clamp(0.0, 1.0)).round();
    if w > 0.0 {
        g.push_clip(inner);
        g.rect(Rect::new(inner.x, inner.y, w, inner.h), theme::MINT);
        if w < inner.w {
            g.rect(Rect::new(inner.x + w, inner.y, theme::BORDER, inner.h), theme::INK);
        }
        g.pop_clip();
        g.round_corners(inner, 7.0, theme::INK);
    }
    // Dark text over the mint fill, normal ink over the track.
    let split = inner.x + w;
    for (clip, color) in [
        (Rect::new(inner.x, inner.y, w, inner.h), theme::ON_ACCENT),
        (Rect::new(split, inner.y, inner.right() - split, inner.h), theme::INK),
    ] {
        if clip.w <= 0.0 {
            continue;
        }
        g.push_clip(clip);
        g.text_mid(FontId::Bold, 14.0, inner.x + 12.0, inner.center_y(), color, left);
        g.text_mid_right(FontId::Bold, 14.0, inner.right() - 12.0, inner.center_y(), color, right);
        g.pop_clip();
    }
}

/// "Library / Explore" style tabs with L and R glyphs on either side.
/// Returns the tab rectangles, for touch.
pub fn tabs(g: &mut Gfx, x: f32, cy: f32, names: &[&str], active: usize) -> Vec<Rect> {
    let mut x = x;
    let lw = button_glyph_width(g, Btn::L, 22.0);
    button_glyph(g, Btn::L, x + lw * 0.5, cy, 22.0);
    x += lw + 10.0;
    let mut rects = Vec::new();
    for (i, name) in names.iter().enumerate() {
        let w = g.measure(FontId::Display, 19.0, name) + 30.0;
        let r = Rect::new(x, cy - 18.0, w.round(), 36.0);
        let on = i == active;
        card_with(g, r, 11.0, if on { theme::SUN } else { theme::PAPER }, theme::BORDER, 3.0);
        g.text_mid_center(FontId::Display, 19.0, r.center_x(), r.center_y() + 1.0, if on { theme::ON_ACCENT } else { theme::INK }, name);
        rects.push(r);
        x += r.w + 10.0;
    }
    let rw = button_glyph_width(g, Btn::R, 22.0);
    button_glyph(g, Btn::R, x + rw * 0.5, cy, 22.0);
    rects
}

/// Draws the glyph for a Vita button centred at (`cx`, `cy`); returns its width.
pub fn button_glyph(g: &mut Gfx, b: Btn, cx: f32, cy: f32, size: f32) -> f32 {
    let face = |g: &mut Gfx, icon: Icon, color: Color| {
        g.circle(cx, cy, size * 0.5, theme::INK);
        g.icon(icon, cx - size * 0.5, cy - size * 0.5, size, color);
        size
    };
    match b {
        Btn::Cross => face(g, Icon::Cross, theme::CROSS),
        Btn::Circle => face(g, Icon::CircleBtn, theme::CIRCLE),
        Btn::Square => face(g, Icon::Square, theme::SQUARE),
        Btn::Triangle => face(g, Icon::Triangle, theme::TRIANGLE),
        Btn::Up | Btn::Down | Btn::Left | Btn::Right => {
            g.circle(cx, cy, size * 0.5, theme::INK);
            g.icon(Icon::DPad, cx - size * 0.4, cy - size * 0.4, size * 0.8, theme::ON_INK_DIM);
            // Highlight the relevant arm.
            let (dx, dy) = match b {
                Btn::Up => (0.0, -0.22),
                Btn::Down => (0.0, 0.22),
                Btn::Left => (-0.22, 0.0),
                _ => (0.22, 0.0),
            };
            let s = size * 0.18;
            g.rounded(Rect::new(cx + dx * size - s * 0.5, cy + dy * size - s * 0.5, s, s), 3.0, theme::PAPER);
            size
        }
        Btn::L | Btn::R | Btn::Start | Btn::Select => {
            let label = shoulder_label(b);
            let px = if label.len() > 1 { size * 0.44 } else { size * 0.6 };
            let w = button_glyph_width(g, b, size);
            let r = Rect::new(cx - w * 0.5, cy - size * 0.42, w, size * 0.84);
            g.rounded_outline(r, size * 0.3, 2.0, theme::INK, theme::PAPER);
            g.text_mid_center(FontId::Bold, px, cx, cy, theme::INK, label);
            w
        }
    }
}

fn shoulder_label(b: Btn) -> &'static str {
    match b {
        Btn::L => "L",
        Btn::R => "R",
        Btn::Start => "START",
        _ => "SELECT",
    }
}

/// Width a glyph will take, for right-aligned layouts.
pub fn button_glyph_width(g: &mut Gfx, b: Btn, size: f32) -> f32 {
    match b {
        Btn::L | Btn::R | Btn::Start | Btn::Select => {
            let label = shoulder_label(b);
            let px = if label.len() > 1 { size * 0.44 } else { size * 0.6 };
            (g.measure(FontId::Bold, px, label) + size * 0.6).max(size * 1.1).round()
        }
        _ => size,
    }
}

/// A footer hint: glyph + label. Returns the total width drawn.
pub fn hint(g: &mut Gfx, x: f32, cy: f32, b: Btn, label: &str) -> f32 {
    let size = 22.0;
    let gw = button_glyph_width(g, b, size);
    button_glyph(g, b, x + gw * 0.5, cy, size);
    let tw = g.text_mid(FontId::Bold, 15.0, x + gw + 8.0, cy, theme::PAPER, label);
    gw + 8.0 + tw
}

pub fn hint_width(g: &mut Gfx, b: Btn, label: &str) -> f32 {
    button_glyph_width(g, b, 22.0) + 8.0 + g.measure(FontId::Bold, 15.0, label)
}

pub const FOOTER_H: f32 = 40.0;

/// Ink bar along the bottom with hints laid out from the right. Returns the
/// tap targets.
pub fn footer(g: &mut Gfx, hints: &[(Btn, &str)]) -> Vec<(Rect, Btn)> {
    let bar = Rect::new(0.0, SCREEN_H as f32 - FOOTER_H, SCREEN_W as f32, FOOTER_H);
    g.rect(bar, theme::INK);
    let mut x = SCREEN_W as f32 - 22.0;
    let mut targets = Vec::new();

    for (b, label) in hints.iter().rev() {
        let w = hint_width(g, *b, label);
        x -= w;
        hint(g, x, bar.center_y(), *b, label);
        targets.push((Rect::new(x - 6.0, bar.y, w + 12.0, bar.h), *b));
        x -= 24.0;
    }
    targets
}

/// "RuffleVita 1.1.0 · by İbrahim Doğan" at the left of the footer bar.
pub fn footer_credit(g: &mut Gfx) {
    footer_note(g, &format!("RuffleVita {} \u{00b7} {CREDIT}", env!("CARGO_PKG_VERSION")));
}

/// Small text at the left of the footer bar.
pub fn footer_note(g: &mut Gfx, text: &str) {
    let cy = SCREEN_H as f32 - FOOTER_H * 0.5;
    g.text_mid(FontId::Regular, 13.0, 22.0, cy, theme::ON_INK_DIM, text);
}

/// Status cluster for the top-right corner: clock and battery.
pub fn status(g: &mut Gfx, right: f32, cy: f32) {
    let (h, m) = crate::platform::local_time();
    let clock = format!("{h:02}:{m:02}");
    let mut x = right;
    if let Some((pct, charging)) = crate::platform::battery() {
        let bw = 28.0;
        let body = Rect::new(x - bw - 3.0, cy - 7.5, bw, 15.0);
        g.rounded_outline(body, 4.0, 2.0, theme::INK, theme::PAPER);
        g.rect(Rect::new(body.right(), cy - 3.5, 3.0, 7.0), theme::INK);
        let fill = if pct <= 15 && !charging { theme::TOMATO } else { theme::MINT };
        let inner = body.inset(4.0);
        g.rect(Rect::new(inner.x, inner.y, (inner.w * pct as f32 / 100.0).round(), inner.h), fill);
        x = body.x - 10.0;
        let label = format!("{pct}%");
        let w = g.text_mid_right(FontId::Bold, 14.0, x, cy, theme::ON_BLUE_DIM, &label);
        x -= w + 14.0;
    }
    let w = g.measure(FontId::Display, 19.0, &clock);
    shadow_text(g, FontId::Display, 19.0, x - w, cy, theme::PAPER, &clock);
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

/// Deterministic cover colour for a name.
pub fn cover_color(name: &str) -> Color {
    let mut h: u32 = 2166136261;
    for b in name.bytes() {
        h = (h ^ b as u32).wrapping_mul(16777619);
    }
    Color::hex(theme::COVERS[(h % theme::COVERS.len() as u32) as usize])
}

/// Up to two initials, for small generated covers.
pub fn initials(name: &str) -> String {
    let mut out: String = name
        .split_whitespace()
        .filter_map(|w| w.chars().find(|c| c.is_alphanumeric()))
        .take(2)
        .collect();
    if out.is_empty() {
        out.push('?');
    }
    out.to_uppercase()
}

/// A cover for a game without art: its colour, stripes and its name (or
/// initials when small). Draws inside `r` without an outline.
pub fn generated_cover(g: &mut Gfx, r: Rect, name: &str, color: Color, big: bool) {
    g.rect(r, color);
    g.pattern(Pattern::Stripes, r, theme::PAPER.alpha(0.16));
    if big {
        let px = (r.h * 0.26).clamp(22.0, 48.0);
        let lines = g.wrap(FontId::Display, px, name, r.w - 44.0, 2);
        let line_h = px * 1.02;
        let mut cy = r.bottom() - 22.0 - px * 0.36 - line_h * (lines.len() as f32 - 1.0);
        for line in &lines {
            shadow_text(g, FontId::Display, px, r.x + 22.0, cy, theme::PAPER, line);
            cy += line_h;
        }
    } else {
        let px = (r.h * 0.42).clamp(12.0, 26.0);
        shadow_text_center(g, FontId::Display, px, r.center_x(), r.center_y(), theme::PAPER, &initials(name));
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
        // Slide up and out over the last 0.25 s instead of fading.
        let out = (1.0 - left / 0.25).clamp(0.0, 1.0);
        let w = g.measure(FontId::Bold, 15.0, &self.text) + 40.0;
        let r = Rect::new(((SCREEN_W as f32 - w) * 0.5).round(), (16.0 - out * 70.0).round(), w.round(), 38.0);
        card_with(g, r, 12.0, theme::SUN, theme::BORDER, 4.0);
        g.text_mid_center(FontId::Bold, 15.0, r.center_x(), r.center_y(), theme::ON_ACCENT, &self.text);
    }
}

/// Animated loading spinner.
pub fn spinner(g: &mut Gfx, cx: f32, cy: f32, size: f32, color: Color, t: f32) {
    g.icon_rotated(Icon::Spinner, cx, cy, size, t * 5.5, color);
}
