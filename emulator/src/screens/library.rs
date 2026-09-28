//! The game browser: a scrolling list on the left, a large preview with
//! details on the right.

use std::collections::HashMap;

use crate::config::{Settings, Sort};
use crate::input::{Btn, InputEvent, TouchPhase};
use crate::library::{Game, Library};
use crate::platform::{self, SCREEN_H, SCREEN_W};
use crate::screens::ThumbCache;
use crate::ui::{self, Color, FontId, Gfx, Icon, Rect, theme};
use crate::worker::Worker;

pub enum LibAction {
    None,
    Play(usize),
    Settings(usize),
    ToggleSort,
    Rescan,
}

const LIST: Rect = Rect::new(20.0, 70.0, 448.0, 420.0);
const ROW_H: f32 = 70.0;
const DETAIL: Rect = Rect::new(488.0, 70.0, 452.0, 420.0);
const PREVIEW: Rect = Rect::new(488.0, 70.0, 452.0, 254.0);

struct Drag {
    start_y: f32,
    start_scroll: f32,
    moved: bool,
    row: Option<usize>,
}

pub struct LibraryScreen {
    pub selected: usize,
    scroll: f32,
    highlight_y: f32,
    drag: Option<Drag>,
    footer_targets: Vec<(Rect, Btn)>,
    /// Keeps animating (and redrawing) until things settle.
    settled: bool,
    /// Controls summary per game; profiles live on disk, so don't re-read per frame.
    summaries: HashMap<String, String>,
}

impl LibraryScreen {
    pub fn new() -> Self {
        Self {
            selected: 0,
            scroll: 0.0,
            highlight_y: 0.0,
            drag: None,
            footer_targets: Vec::new(),
            settled: false,
            summaries: HashMap::new(),
        }
    }

    pub fn select(&mut self, index: usize, lib: &Library) {
        self.selected = index.min(lib.games.len().saturating_sub(1));
        self.scroll = self.target_scroll(lib);
        self.highlight_y = self.selected as f32 * ROW_H;
        self.settled = false;
    }

    fn max_scroll(lib: &Library) -> f32 {
        (lib.games.len() as f32 * ROW_H - LIST.h).max(0.0)
    }

    fn target_scroll(&self, lib: &Library) -> f32 {
        let row_top = self.selected as f32 * ROW_H;
        let margin = ROW_H * 0.6;
        let mut s = self.scroll;
        if row_top - margin < s {
            s = row_top - margin;
        }
        if row_top + ROW_H + margin > s + LIST.h {
            s = row_top + ROW_H + margin - LIST.h;
        }
        s.clamp(0.0, Self::max_scroll(lib))
    }

    /// Call after a game's profile changed on disk.
    pub fn invalidate_profile(&mut self, key: &str) {
        self.summaries.remove(key);
    }

    pub fn invalidate_all_profiles(&mut self) {
        self.summaries.clear();
    }

    pub fn animating(&self) -> bool {
        !self.settled || self.drag.is_some()
    }

    pub fn update(&mut self, events: &[InputEvent], lib: &Library, dt: f32) -> LibAction {
        let n = lib.games.len();
        let mut action = LibAction::None;
        let press = |b: Btn, this: &mut Self| -> LibAction {
            this.settled = false;
            match b {
                Btn::Up if n > 0 => this.selected = this.selected.saturating_sub(1),
                Btn::Down if n > 0 => this.selected = (this.selected + 1).min(n - 1),
                Btn::L if n > 0 => this.selected = this.selected.saturating_sub(5),
                Btn::R if n > 0 => this.selected = (this.selected + 5).min(n - 1),
                Btn::Cross if n > 0 => return LibAction::Play(this.selected),
                Btn::Cross => return LibAction::Rescan,
                Btn::Triangle if n > 0 => return LibAction::Settings(this.selected),
                Btn::Square if n > 0 => return LibAction::ToggleSort,
                Btn::Select => return LibAction::Rescan,
                _ => {}
            }
            LibAction::None
        };

        for ev in events {
            match ev {
                InputEvent::Button(b, true) => {
                    let a = press(*b, self);
                    if !matches!(a, LibAction::None) {
                        action = a;
                    }
                }
                InputEvent::Touch(t) if t.panel == crate::input::Panel::Front => match t.phase {
                    TouchPhase::Down => {
                        let row = LIST.contains(t.x, t.y).then(|| ((t.y - LIST.y + self.scroll) / ROW_H) as usize);
                        self.drag = Some(Drag {
                            start_y: t.y,
                            start_scroll: self.scroll,
                            moved: false,
                            row: row.filter(|r| *r < n),
                        });
                    }
                    TouchPhase::Move => {
                        if let Some(d) = &mut self.drag {
                            if (t.y - d.start_y).abs() > 10.0 {
                                d.moved = true;
                            }
                            if d.moved && d.row.is_some() {
                                self.scroll = (d.start_scroll - (t.y - d.start_y)).clamp(0.0, Self::max_scroll(lib));
                            }
                        }
                    }
                    TouchPhase::Up => {
                        let Some(d) = self.drag.take() else { continue };
                        if d.moved {
                            continue;
                        }
                        if let Some(row) = d.row {
                            if row == self.selected {
                                action = LibAction::Play(row);
                            } else {
                                self.selected = row;
                                self.settled = false;
                            }
                        } else if PREVIEW.contains(t.x, t.y) && n > 0 {
                            action = LibAction::Play(self.selected);
                        } else if let Some((_, b)) = self.footer_targets.iter().find(|(r, _)| r.contains(t.x, t.y)) {
                            let a = press(*b, self);
                            if !matches!(a, LibAction::None) {
                                action = a;
                            }
                        }
                    }
                },
                _ => {}
            }
        }

        // Animate scroll (unless the finger owns it) and the highlight.
        if self.drag.as_ref().is_none_or(|d| !d.moved) {
            let target = self.target_scroll(lib);
            self.scroll = ui::approach(self.scroll, target, dt, 18.0);
        }
        let target_h = self.selected as f32 * ROW_H;
        self.highlight_y = ui::approach(self.highlight_y, target_h, dt, 22.0);
        self.settled = self.highlight_y == target_h
            && (self.drag.is_some() || self.scroll == self.target_scroll(lib));
        action
    }

    pub fn draw(&mut self, g: &mut Gfx, lib: &Library, settings: &Settings, thumbs: &mut ThumbCache, worker: &Worker, analyzing: bool) {
        ui::background(g);
        header(g, lib.games.len(), analyzing);

        if lib.games.is_empty() {
            empty_state(g);
            self.footer_targets = ui::footer(g, &[(Btn::Cross, "Refresh")]);
            ui::footer_credit(g);
            return;
        }

        // ---- list
        g.push_clip(LIST);
        let first = (self.scroll / ROW_H).floor().max(0.0) as usize;
        let last = (((self.scroll + LIST.h) / ROW_H).ceil() as usize).min(lib.games.len());
        let hl = Rect::new(LIST.x, LIST.y + self.highlight_y - self.scroll, LIST.w, ROW_H - 6.0);
        g.shadow(hl, 8.0, theme::ACCENT.alpha(0.18));
        g.rounded(hl, 12.0, theme::PANEL_HI);
        g.rounded(Rect::new(hl.x, hl.y + 14.0, 4.0, hl.h - 28.0), 2.0, theme::ACCENT);
        for i in first..last {
            let game = &lib.games[i];
            let r = Rect::new(LIST.x, LIST.y + i as f32 * ROW_H - self.scroll, LIST.w, ROW_H - 6.0);
            thumbs.want(&game.key, lib, worker);
            draw_row(g, game, r, i == self.selected, thumbs);
        }
        g.pop_clip();
        // Fade the list edges.
        let fade = Color::hex(0x0E1322);
        if self.scroll > 1.0 {
            g.rect_v(Rect::new(LIST.x, LIST.y, LIST.w, 14.0), fade, fade.alpha(0.0));
        }
        if self.scroll < Self::max_scroll(lib) - 1.0 {
            g.rect_v(Rect::new(LIST.x, LIST.bottom() - 14.0, LIST.w, 14.0), fade.alpha(0.0), theme::BG_BOTTOM.mix(fade, 0.5));
        }
        // Scrollbar.
        let total = lib.games.len() as f32 * ROW_H;
        if total > LIST.h {
            let h = (LIST.h * LIST.h / total).max(30.0);
            let y = LIST.y + (self.scroll / (total - LIST.h)) * (LIST.h - h);
            g.rounded(Rect::new(LIST.right() + 6.0, y, 3.0, h), 1.5, theme::FAINT.alpha(0.6));
        }

        // ---- details
        let game = &lib.games[self.selected];
        thumbs.want(&game.key, lib, worker);
        let summary = self
            .summaries
            .entry(game.key.clone())
            .or_insert_with(|| settings.load_profile(&game.key).summary());
        draw_details(g, game, summary, thumbs);

        let sort_label = match settings.sort {
            Sort::Recent => "Sort: Recent",
            Sort::Name => "Sort: A\u{2013}Z",
        };
        self.footer_targets = ui::footer(
            g,
            &[(Btn::Square, sort_label), (Btn::Triangle, "Settings"), (Btn::Cross, "Play")],
        );
        ui::footer_credit(g);
    }
}

fn header(g: &mut Gfx, count: usize, analyzing: bool) {
    let logo = Rect::new(22.0, 17.0, 32.0, 32.0);
    ui::logo(g, logo);
    let x = logo.right() + 12.0;
    let w = g.text_mid(FontId::Bold, 22.0, x, logo.center_y(), theme::TEXT, "FlashVita");
    let label = match count {
        0 => String::new(),
        1 => "1 game".into(),
        n => format!("{n} games"),
    };
    let lw = g.text_mid(FontId::Regular, 14.0, x + w + 14.0, logo.center_y() + 1.0, theme::FAINT, &label);
    if analyzing {
        g.text_mid(
            FontId::Regular,
            13.0,
            x + w + 14.0 + lw + 14.0,
            logo.center_y() + 1.0,
            theme::FAINT,
            "\u{00b7} scanning\u{2026}",
        );
    }
    ui::status(g, SCREEN_W as f32 - 24.0, logo.center_y());
}

/// Deterministic pleasant colour per game for covers without art.
fn placeholder_color(game: &Game) -> Color {
    if let Some([r, gg, b]) = game.db.info.as_ref().and_then(|i| i.background) {
        let (max, min) = (r.max(gg).max(b) as f32, r.min(gg).min(b) as f32);
        // Only colourful backgrounds; white/black/grey ones make dull covers.
        if max > 40.0 && (max - min) / max > 0.35 {
            let c = Color { r: r as f32 / 255.0, g: gg as f32 / 255.0, b: b as f32 / 255.0, a: 1.0 };
            return c.mix(theme::PANEL_HI, 0.35);
        }
    }
    let mut h: u32 = 2166136261;
    for b in game.key.bytes() {
        h = (h ^ b as u32).wrapping_mul(16777619);
    }
    const PALETTE: [u32; 8] = [0x3A5BD9, 0x7B4FE0, 0xD9480F, 0x0CA678, 0xE64980, 0x1098AD, 0xF08C00, 0x5C7CFA];
    Color::hex(PALETTE[(h % 8) as usize]).mix(theme::PANEL, 0.35)
}

fn initials(name: &str) -> String {
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

fn cover(g: &mut Gfx, game: &Game, r: Rect, radius: f32, bg: Color, thumbs: &ThumbCache, big: bool) {
    match thumbs.get(&game.key) {
        Some(tex) => {
            g.rect(r, Color::hex(0x05070C));
            if big {
                // Blurred-looking fill behind a letterboxed image.
                g.image_cover(tex, r, Color::hex(0xFFFFFF).alpha(0.25));
                g.image_contain(tex, r, Color::hex(0xFFFFFF));
            } else {
                g.image_cover(tex, r, Color::hex(0xFFFFFF));
            }
        }
        None => {
            let c = placeholder_color(game);
            g.rect_v(r, c.mix(Color::hex(0xFFFFFF), 0.08), c.mix(Color::hex(0x000000), 0.35));
            let px = if big { 64.0 } else { 20.0 };
            g.text_mid_center(FontId::Bold, px, r.center_x(), r.center_y(), Color::hex(0xFFFFFF).alpha(0.9), &initials(game.title()));
        }
    }
    g.round_corners(r, radius, bg);
}

fn draw_row(g: &mut Gfx, game: &Game, r: Rect, selected: bool, thumbs: &ThumbCache) {
    let bg = if selected { theme::PANEL_HI } else { theme::BG_TOP.mix(theme::BG_BOTTOM, (r.y / SCREEN_H as f32).clamp(0.0, 1.0)) };
    let thumb = Rect::new(r.x + 12.0, r.y + 8.0, 86.0, r.h - 16.0);
    cover(g, game, thumb, 8.0, bg, thumbs, false);
    let tx = thumb.right() + 14.0;
    let max_w = r.right() - tx - 12.0;
    let title = g.ellipsize(FontId::Bold, 17.0, game.title(), max_w);
    g.text_mid(FontId::Bold, 17.0, tx, r.y + r.h * 0.36, if selected { theme::TEXT } else { theme::TEXT.alpha(0.88) }, &title);

    let mut sub = Vec::new();
    if let Some(info) = &game.db.info {
        sub.push(if info.as3 { "AS3".to_owned() } else { "AS1/2".to_owned() });
    } else if game.db.error.is_some() {
        sub.push("Unreadable".to_owned());
    }
    sub.push(ui::human_size(game.db.size));
    if let Some(folder) = game.folder.as_ref().filter(|f| !f.eq_ignore_ascii_case(game.title())) {
        sub.push(folder.clone());
    }
    let sub = g.ellipsize(FontId::Regular, 13.0, &sub.join("  \u{00b7}  "), max_w);
    g.text_mid(FontId::Regular, 13.0, tx, r.y + r.h * 0.68, theme::DIM, &sub);
}

fn chip(g: &mut Gfx, x: f32, y: f32, text: &str, color: Color) -> f32 {
    let w = g.measure(FontId::Bold, 12.0, text) + 18.0;
    let r = Rect::new(x, y, w, 24.0);
    g.rounded(r, 12.0, color.alpha(0.16));
    g.text_mid_center(FontId::Bold, 12.0, r.center_x(), r.center_y(), color, text);
    w
}

fn draw_details(g: &mut Gfx, game: &Game, summary: &str, thumbs: &ThumbCache) {
    g.shadow(PREVIEW, 16.0, theme::SHADOW);
    cover(g, game, PREVIEW, 14.0, theme::BG_TOP.mix(theme::BG_BOTTOM, 0.25), thumbs, true);

    let mut y = PREVIEW.bottom() + 18.0;
    let lines = g.wrap(FontId::Bold, 22.0, game.title(), DETAIL.w, 2);
    for line in &lines {
        g.text(FontId::Bold, 22.0, DETAIL.x, y + 18.0, theme::TEXT, line);
        y += 28.0;
    }
    y += 6.0;

    let mut x = DETAIL.x;
    if let Some(info) = &game.db.info {
        let (label, c) = if info.as3 { ("ActionScript 3", theme::ACCENT2) } else { ("ActionScript 1/2", theme::ACCENT) };
        x += chip(g, x, y, label, c) + 6.0;
        x += chip(g, x, y, &format!("SWF {}", info.version), theme::DIM) + 6.0;
        if info.width > 0 {
            x += chip(g, x, y, &format!("{}\u{00d7}{}", info.width, info.height), theme::DIM) + 6.0;
        }
        x += chip(g, x, y, &format!("{:.0} fps", info.fps), theme::DIM) + 6.0;
    } else if let Some(err) = &game.db.error {
        x += chip(g, x, y, err, theme::DANGER) + 6.0;
    } else {
        x += chip(g, x, y, "Reading\u{2026}", theme::DIM) + 6.0;
    }
    chip(g, x, y, &ui::human_size(game.db.size), theme::DIM);
    y += 38.0;

    let played = if game.db.last_played > 0 {
        let when = ui::relative_time(game.db.last_played, platform::now_unix());
        match game.db.play_count {
            1 => format!("Played {when}"),
            n => format!("Played {when} \u{00b7} {n} sessions"),
        }
    } else {
        "Not played yet".into()
    };
    g.icon(Icon::Play, DETAIL.x - 2.0, y - 8.0, 16.0, theme::FAINT);
    g.text_mid(FontId::Regular, 14.0, DETAIL.x + 20.0, y, theme::DIM, &played);
    y += 24.0;
    g.icon(Icon::Keyboard, DETAIL.x - 2.0, y - 8.0, 16.0, theme::FAINT);
    let summary = g.ellipsize(FontId::Regular, 14.0, summary, DETAIL.w - 24.0);
    g.text_mid(FontId::Regular, 14.0, DETAIL.x + 20.0, y, theme::DIM, &summary);
}

fn empty_state(g: &mut Gfx) {
    let card = Rect::new(180.0, 120.0, 600.0, 300.0);
    ui::panel(g, card);
    g.icon(Icon::Folder, card.center_x() - 32.0, card.y + 36.0, 64.0, theme::ACCENT);
    g.text_mid_center(FontId::Bold, 24.0, card.center_x(), card.y + 136.0, theme::TEXT, "No games yet");
    g.text_mid_center(FontId::Regular, 16.0, card.center_x(), card.y + 172.0, theme::DIM, "Copy your .swf files (subfolders work too) to");
    let path = platform::games_dir_display();
    let w = g.measure(FontId::Bold, 17.0, &path) + 28.0;
    let pill = Rect::new(card.center_x() - w * 0.5, card.y + 192.0, w, 34.0);
    g.rounded(pill, 17.0, theme::ACCENT.alpha(0.14));
    g.text_mid_center(FontId::Bold, 17.0, pill.center_x(), pill.center_y(), theme::ACCENT, &path);
    // "then press (X) to refresh this list." with a real button glyph.
    let (a, b) = ("then press", "to refresh this list.");
    let (wa, wb) = (g.measure(FontId::Regular, 15.0, a), g.measure(FontId::Regular, 15.0, b));
    let total = wa + 8.0 + 22.0 + 8.0 + wb;
    let mut x = card.center_x() - total * 0.5;
    let cy = card.y + 256.0;
    x += g.text_mid(FontId::Regular, 15.0, x, cy, theme::FAINT, a) + 8.0;
    ui::button_glyph(g, Btn::Cross, x + 11.0, cy, 22.0);
    g.text_mid(FontId::Regular, 15.0, x + 30.0, cy, theme::FAINT, b);
}
