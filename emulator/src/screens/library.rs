//! The game browser: a scrolling list on the left, a large preview with
//! details on the right.

use std::collections::HashMap;

use crate::config::{Settings, Sort};
use crate::input::{Btn, InputEvent, TouchPhase};
use crate::library::{Game, Library};
use crate::platform;
use crate::screens::{self, Tab, ThumbCache};
use crate::ui::{self, Color, FontId, Gfx, Icon, Rect, theme};
use crate::worker::Worker;

pub enum LibAction {
    None,
    Play(usize),
    Settings(usize),
    ToggleSort,
    Rescan,
    SwitchTab(Tab),
}

/// The paper card holding the list, and the scrolling area inside it.
const LIST_CARD: Rect = Rect::new(20.0, 70.0, 462.0, 420.0);
const LIST: Rect = Rect::new(LIST_CARD.x + 8.0, LIST_CARD.y + 8.0, LIST_CARD.w - 16.0, LIST_CARD.h - 16.0);
const ROW_H: f32 = 54.0;
const DETAIL: Rect = Rect::new(504.0, 70.0, 436.0, 420.0);
const PREVIEW: Rect = Rect::new(504.0, 70.0, 436.0, 206.0);

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
    tab_rects: Vec<Rect>,
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
            tab_rects: Vec::new(),
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
                Btn::Left if n > 0 => this.selected = this.selected.saturating_sub(6),
                Btn::Right if n > 0 => this.selected = (this.selected + 6).min(n - 1),
                Btn::L | Btn::R => return LibAction::SwitchTab(Tab::Explore),
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
                        if self.tab_rects.get(1).is_some_and(|r| r.contains(t.x, t.y)) {
                            action = LibAction::SwitchTab(Tab::Explore);
                            continue;
                        }
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
        let count = match lib.games.len() {
            0 => String::new(),
            1 => "1 game".into(),
            n => format!("{n} games"),
        };
        let note = if analyzing { format!("{count} \u{00b7} scanning\u{2026}") } else { count };
        self.tab_rects = screens::top_bar(g, Tab::Library, &note);

        if lib.games.is_empty() {
            empty_state(g);
            self.footer_targets = ui::footer(g, &[(Btn::R, "Explore"), (Btn::Cross, "Refresh")]);
            ui::footer_credit(g);
            return;
        }

        // ---- list
        ui::card(g, LIST_CARD, theme::RADIUS, theme::PAPER);
        g.push_clip(LIST);
        let first = (self.scroll / ROW_H).floor().max(0.0) as usize;
        let last = (((self.scroll + LIST.h) / ROW_H).ceil() as usize).min(lib.games.len());
        let hl = Rect::new(LIST.x, LIST.y + self.highlight_y - self.scroll, LIST.w, ROW_H - 4.0);
        g.rounded_outline(hl, 10.0, theme::BORDER, theme::INK, theme::SUN);
        for i in first..last {
            let game = &lib.games[i];
            let r = Rect::new(LIST.x, LIST.y + i as f32 * ROW_H - self.scroll, LIST.w, ROW_H - 4.0);
            thumbs.want(&game.key, lib, worker);
            draw_row(g, game, r, i == self.selected, thumbs);
        }
        g.pop_clip();
        screens::scrollbar(g, LIST, self.scroll, lib.games.len() as f32 * ROW_H);

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

/// Cover colour for a game without art: its SWF background when that's
/// colourful, else one picked by name.
fn placeholder_color(game: &Game) -> Color {
    if let Some([r, gg, b]) = game.db.info.as_ref().and_then(|i| i.background) {
        let (max, min) = (r.max(gg).max(b) as f32, r.min(gg).min(b) as f32);
        // Only colourful backgrounds; white/black/grey ones make dull covers.
        if max > 40.0 && (max - min) / max > 0.35 {
            return Color { r: r as f32 / 255.0, g: gg as f32 / 255.0, b: b as f32 / 255.0, a: 1.0 };
        }
    }
    ui::cover_color(&game.key)
}

/// Draws a cover inside an ink outline of `border`.
fn cover(g: &mut Gfx, game: &Game, r: Rect, radius: f32, border: f32, thumbs: &ThumbCache, big: bool) {
    g.rounded(r, radius, theme::INK);
    let inner = r.inset(border);
    match thumbs.get(&game.key) {
        Some(tex) if big => {
            g.image_contain(tex, inner, Color::hex(0xFFFFFF));
        }
        Some(tex) => g.image_cover(tex, inner, Color::hex(0xFFFFFF)),
        None => {
            g.push_clip(inner);
            ui::generated_cover(g, inner, game.title(), placeholder_color(game), big);
            g.pop_clip();
        }
    }
    g.round_corners(inner, (radius - border).max(3.0), theme::INK);
}

fn draw_row(g: &mut Gfx, game: &Game, r: Rect, selected: bool, thumbs: &ThumbCache) {
    let thumb = Rect::new(r.x + 8.0, r.y + 6.0, 70.0, r.h - 12.0);
    cover(g, game, thumb, 8.0, 2.0, thumbs, false);
    let tx = thumb.right() + 12.0;
    let max_w = r.right() - tx - 12.0;
    let title = g.ellipsize(FontId::Bold, 16.5, game.title(), max_w);
    g.text_mid(FontId::Bold, 16.5, tx, r.y + r.h * 0.36, theme::INK, &title);

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
    let sub_color = if selected { theme::INK.alpha(0.7) } else { theme::MUTED };
    g.text_mid(FontId::Regular, 13.0, tx, r.y + r.h * 0.7, sub_color, &sub);
}

fn draw_details(g: &mut Gfx, game: &Game, summary: &str, thumbs: &ThumbCache) {
    g.rounded(PREVIEW.offset(theme::SHADOW, theme::SHADOW), theme::RADIUS + 2.0, theme::INK);
    cover(g, game, PREVIEW, theme::RADIUS + 2.0, theme::BORDER, thumbs, true);

    let mut y = PREVIEW.bottom() + 32.0;
    let lines = g.wrap(FontId::Display, 30.0, game.title(), DETAIL.w, 2);
    for line in &lines {
        ui::shadow_text(g, FontId::Display, 30.0, DETAIL.x, y, theme::PAPER, line);
        y += 34.0;
    }
    y -= 6.0;

    let mut x = DETAIL.x;
    if let Some(info) = &game.db.info {
        let (label, c) = if info.as3 { ("AS3", theme::TOMATO) } else { ("AS1/2", theme::SUN) };
        x += ui::chip(g, x, y, label, c) + 7.0;
        x += ui::chip(g, x, y, &format!("SWF {}", info.version), theme::PAPER) + 7.0;
        if info.width > 0 {
            x += ui::chip(g, x, y, &format!("{}\u{00d7}{}", info.width, info.height), theme::PAPER) + 7.0;
        }
        x += ui::chip(g, x, y, &format!("{:.0} fps", info.fps), theme::PAPER) + 7.0;
    } else if let Some(err) = &game.db.error {
        x += ui::chip(g, x, y, err, theme::TOMATO) + 7.0;
    } else {
        x += ui::chip(g, x, y, "Reading\u{2026}", theme::PAPER) + 7.0;
    }
    ui::chip(g, x, y, &ui::human_size(game.db.size), theme::PAPER);
    y += 46.0;

    let played = if game.db.last_played > 0 {
        let when = ui::relative_time(game.db.last_played, platform::now_unix());
        match game.db.play_count {
            1 => format!("Played {when}"),
            n => format!("Played {when} \u{00b7} {n} sessions"),
        }
    } else {
        "Not played yet".into()
    };
    g.icon(Icon::Play, DETAIL.x - 2.0, y - 8.0, 16.0, theme::ON_BLUE_DIM);
    g.text_mid(FontId::Bold, 14.5, DETAIL.x + 20.0, y, theme::PAPER, &played);
    y += 26.0;
    g.icon(Icon::Keyboard, DETAIL.x - 2.0, y - 8.0, 16.0, theme::ON_BLUE_DIM);
    let summary = g.ellipsize(FontId::Bold, 14.5, summary, DETAIL.w - 24.0);
    g.text_mid(FontId::Bold, 14.5, DETAIL.x + 20.0, y, theme::PAPER, &summary);
}

fn empty_state(g: &mut Gfx) {
    let card = Rect::new(180.0, 110.0, 600.0, 320.0);
    ui::card(g, card, theme::RADIUS + 2.0, theme::PAPER);
    ui::logo(g, Rect::new(card.center_x() - 30.0, card.y + 34.0, 60.0, 60.0));
    g.text_mid_center(FontId::Display, 30.0, card.center_x(), card.y + 136.0, theme::INK, "No games yet");
    g.text_mid_center(FontId::Regular, 16.0, card.center_x(), card.y + 174.0, theme::MUTED, "Copy your .swf files (subfolders work too) to");
    let path = platform::games_dir_display();
    let w = g.measure(FontId::Bold, 17.0, &path) + 28.0;
    let pill = Rect::new((card.center_x() - w * 0.5).round(), card.y + 194.0, w.round(), 36.0);
    g.rounded_outline(pill, 10.0, 2.0, theme::INK, theme::SUN);
    g.text_mid_center(FontId::Bold, 17.0, pill.center_x(), pill.center_y(), theme::INK, &path);
    // "or find some in Explore (R), then press (X) to refresh." with real glyphs.
    let (a, b, c) = ("Find some in", "Explore, or press", "to refresh.");
    let size = 22.0;
    let rw = ui::button_glyph_width(g, Btn::R, size);
    let widths = [g.measure(FontId::Regular, 15.0, a), g.measure(FontId::Regular, 15.0, b), g.measure(FontId::Regular, 15.0, c)];
    let total = widths.iter().sum::<f32>() + rw + size + 6.0 * 8.0;
    let mut x = card.center_x() - total * 0.5;
    let cy = card.y + 270.0;
    x += g.text_mid(FontId::Regular, 15.0, x, cy, theme::MUTED, a) + 8.0;
    ui::button_glyph(g, Btn::R, x + rw * 0.5, cy, size);
    x += rw + 8.0;
    x += g.text_mid(FontId::Regular, 15.0, x, cy, theme::MUTED, b) + 8.0;
    ui::button_glyph(g, Btn::Cross, x + size * 0.5, cy, size);
    x += size + 8.0;
    g.text_mid(FontId::Regular, 15.0, x, cy, theme::MUTED, c);
}
