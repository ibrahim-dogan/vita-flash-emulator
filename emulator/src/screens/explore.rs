//! Explore: browse, search and download games from the Silvergames Flash
//! archive. Same layout as the library: a list on the left, the selected
//! game on the right.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::catalog::{self, Catalog, Entry, Probe, Sort};
use crate::fetcher::{Done, Fetcher, Job};
use crate::input::{Btn, InputEvent, Panel, TouchPhase};
use crate::library::Library;
use crate::screens::{self, Tab};
use crate::swfinfo::Image;
use crate::ui::{self, Color, FontId, Gfx, Icon, Rect, Texture, theme};

pub enum ExploreAction {
    None,
    SwitchTab(Tab),
    /// Open the system keyboard for the search box.
    StartSearch,
    Download { slug: String, file_name: String },
    CancelDownload,
    /// Play a game that's already in the library, by file name.
    Play(String),
}

const LIST_CARD: Rect = Rect::new(20.0, 70.0, 462.0, 420.0);
const SEARCH: Rect = Rect::new(LIST_CARD.x + 10.0, LIST_CARD.y + 10.0, LIST_CARD.w - 20.0, 40.0);
const LIST: Rect = Rect::new(LIST_CARD.x + 8.0, SEARCH.y + SEARCH.h + 8.0, LIST_CARD.w - 16.0, LIST_CARD.y + LIST_CARD.h - 8.0 - (SEARCH.y + SEARCH.h + 8.0));
const ROW_H: f32 = 54.0;
const DETAIL: Rect = Rect::new(504.0, 70.0, 436.0, 420.0);
const COVER: Rect = Rect::new(504.0, 70.0, 436.0, 206.0);
/// How long the selection must rest on a game before we look it up.
const PROBE_DELAY: f32 = 0.35;
const MAX_COVERS: usize = 48;

enum CatalogState {
    Idle,
    Fetching { bytes: u64 },
    Failed(String),
}

pub struct Download {
    pub slug: String,
    pub done: u64,
    pub total: Option<u64>,
}

pub struct ExploreScreen {
    catalog: Option<Catalog>,
    state: CatalogState,
    pub query: String,
    pub searching: bool,
    sort: Sort,
    view: Vec<usize>,
    selected: usize,
    scroll: f32,
    highlight_y: f32,
    settled: bool,
    rest: f32,
    probes: HashMap<String, Probe>,
    probes_dirty: bool,
    pending: HashSet<String>,
    covers: HashMap<String, Texture>,
    cover_lru: VecDeque<String>,
    /// Lower-case file names in the games folder.
    owned: HashSet<String>,
    pub download: Option<Download>,
    /// Last download failure, shown on that game.
    failed: Option<(String, String)>,
    footer_targets: Vec<(Rect, Btn)>,
    tab_rects: Vec<Rect>,
    drag: Option<(f32, f32, bool, Option<usize>)>,
}

impl ExploreScreen {
    pub fn new() -> Self {
        Self {
            catalog: None,
            state: CatalogState::Idle,
            query: String::new(),
            searching: false,
            sort: Sort::Name,
            view: Vec::new(),
            selected: 0,
            scroll: 0.0,
            highlight_y: 0.0,
            settled: false,
            rest: 0.0,
            probes: HashMap::new(),
            probes_dirty: false,
            pending: HashSet::new(),
            covers: HashMap::new(),
            cover_lru: VecDeque::new(),
            owned: HashSet::new(),
            download: None,
            failed: None,
            footer_targets: Vec::new(),
            tab_rects: Vec::new(),
            drag: None,
        }
    }

    /// Called when the tab opens: loads the cached catalog and refreshes it
    /// when there's none or it's a week old.
    pub fn open(&mut self, fetcher: &Fetcher, lib: &Library) {
        self.refresh_owned(lib);
        if self.catalog.is_none() {
            self.probes = catalog::load_probes();
            if let Some(c) = Catalog::load_cached() {
                self.catalog = Some(c);
                self.rebuild_view(None);
            }
        }
        let stale = self.catalog.as_ref().is_none_or(|c| c.stale());
        if stale && !matches!(self.state, CatalogState::Fetching { .. }) {
            self.refresh(fetcher);
        }
        self.settled = false;
    }

    pub fn refresh(&mut self, fetcher: &Fetcher) {
        self.state = CatalogState::Fetching { bytes: 0 };
        fetcher.submit(Job::Catalog);
    }

    pub fn refresh_owned(&mut self, lib: &Library) {
        self.owned = lib
            .games
            .iter()
            .filter_map(|g| g.path.file_name().map(|n| n.to_string_lossy().to_lowercase()))
            .collect();
    }

    pub fn animating(&self) -> bool {
        !self.settled || self.drag.is_some() || matches!(self.state, CatalogState::Fetching { .. }) || self.download.is_some()
    }

    /// Saves what we learned about games, if anything changed.
    pub fn save(&mut self) {
        if self.probes_dirty {
            catalog::save_probes(&self.probes);
            self.probes_dirty = false;
        }
    }

    fn selected_entry(&self) -> Option<&Entry> {
        let c = self.catalog.as_ref()?;
        self.view.get(self.selected).map(|&i| &c.entries[i])
    }

    fn owns(&self, e: &Entry) -> bool {
        self.owned.contains(&e.file_name().to_lowercase())
    }

    /// Rebuilds the filtered list, keeping `keep` (a slug) selected.
    fn rebuild_view(&mut self, keep: Option<String>) {
        let Some(c) = &self.catalog else { return };
        self.view = c.view(&self.query, self.sort);
        self.selected = keep
            .and_then(|slug| self.view.iter().position(|&i| c.entries[i].slug == slug))
            .unwrap_or(0);
        self.scroll = self.target_scroll();
        self.highlight_y = self.selected as f32 * ROW_H;
        self.rest = 0.0;
        self.settled = false;
    }

    pub fn set_query(&mut self, q: String) {
        if q != self.query {
            self.query = q;
            self.rebuild_view(None);
        }
    }

    fn max_scroll(&self) -> f32 {
        (self.view.len() as f32 * ROW_H - LIST.h).max(0.0)
    }

    fn target_scroll(&self) -> f32 {
        let row_top = self.selected as f32 * ROW_H;
        let margin = ROW_H * 0.6;
        let mut s = self.scroll;
        if row_top - margin < s {
            s = row_top - margin;
        }
        if row_top + ROW_H + margin > s + LIST.h {
            s = row_top + ROW_H + margin - LIST.h;
        }
        s.clamp(0.0, self.max_scroll())
    }

    // ------------------------------------------------------------ results

    pub fn on_result(&mut self, done: Done, gfx: &Gfx, lib: &Library) -> Option<String> {
        let mut message = None;
        match done {
            Done::CatalogProgress(bytes) => {
                if let CatalogState::Fetching { bytes: b } = &mut self.state {
                    *b = bytes;
                }
            }
            Done::Catalog(Ok(entries)) => {
                let keep = self.selected_entry().map(|e| e.slug.clone());
                let c = Catalog { entries, fetched: crate::platform::now_unix() };
                c.save();
                self.catalog = Some(c);
                self.state = CatalogState::Idle;
                self.rebuild_view(keep);
            }
            Done::Catalog(Err(e)) => {
                tracing::warn!("Couldn't fetch the Explore catalog: {e}");
                if self.catalog.is_none() {
                    self.state = CatalogState::Failed(e);
                } else {
                    self.state = CatalogState::Idle;
                    message = Some("Couldn't refresh the catalog; showing the saved one".into());
                }
            }
            Done::Probe { slug, probe, cover } => {
                self.pending.remove(&slug);
                self.probes.insert(slug.clone(), probe);
                self.probes_dirty = true;
                self.insert_cover(gfx, &slug, cover);
            }
            Done::Cover { slug, cover } => {
                self.pending.remove(&slug);
                self.insert_cover(gfx, &slug, cover);
            }
            Done::Progress { slug, done, total } => {
                if let Some(d) = self.download.as_mut().filter(|d| d.slug == slug) {
                    d.done = done;
                    d.total = total;
                }
            }
            Done::Downloaded { slug, result } => {
                self.download = None;
                let title = self.title_of(&slug);
                match result {
                    Ok(_) => {
                        self.failed = None;
                        self.refresh_owned(lib);
                        message = Some(format!("{title} is in your library"));
                    }
                    Err(e) if e == crate::net::NetError::Cancelled.to_string() => {
                        message = Some("Download cancelled".into());
                    }
                    Err(e) => {
                        tracing::warn!("Couldn't download {slug}: {e}");
                        message = Some(format!("Couldn't download {title}"));
                        self.failed = Some((slug, e));
                    }
                }
            }
        }
        message
    }

    fn title_of(&self, slug: &str) -> String {
        self.catalog
            .as_ref()
            .and_then(|c| c.entries.iter().find(|e| e.slug == slug))
            .map(|e| e.title.clone())
            .unwrap_or_else(|| slug.to_owned())
    }

    fn insert_cover(&mut self, gfx: &Gfx, slug: &str, cover: Option<Image>) {
        let Some(img) = cover else { return };
        if let Some(tex) = gfx.create_texture(img.w, img.h, &img.rgba) {
            self.covers.insert(slug.to_owned(), tex);
            self.cover_lru.retain(|k| k != slug);
            self.cover_lru.push_back(slug.to_owned());
            while self.cover_lru.len() > MAX_COVERS {
                if let Some(old) = self.cover_lru.pop_front() {
                    self.covers.remove(&old);
                }
            }
        }
    }

    /// Asks for a cover we know is on the memory card.
    fn want_cover(&mut self, slug: &str, fetcher: &Fetcher) {
        if self.covers.contains_key(slug) {
            if let Some(pos) = self.cover_lru.iter().position(|k| k == slug) {
                let k = self.cover_lru.remove(pos).unwrap();
                self.cover_lru.push_back(k);
            }
            return;
        }
        if !self.pending.contains(slug) && self.probes.get(slug).is_some_and(|p| p.has_cover) {
            self.pending.insert(slug.to_owned());
            fetcher.submit(Job::LoadCover { slug: slug.to_owned() });
        }
    }

    // ------------------------------------------------------------- update

    pub fn update(&mut self, events: &[InputEvent], fetcher: &Fetcher, dt: f32) -> ExploreAction {
        let n = self.view.len();
        let mut action = ExploreAction::None;
        let before = self.selected;

        if self.searching {
            let mut q = self.query.clone();
            for ev in events {
                match ev {
                    InputEvent::Text(t) => q.push_str(t),
                    InputEvent::Backspace => {
                        q.pop();
                    }
                    InputEvent::Enter => self.searching = false,
                    _ => {}
                }
            }
            self.set_query(q);
        } else {
            for ev in events {
                let a = match ev {
                    InputEvent::Button(b, true) => self.press(*b, n, fetcher),
                    InputEvent::Touch(t) if t.panel == Panel::Front => self.touch(t.phase, t.x, t.y, n, fetcher),
                    _ => ExploreAction::None,
                };
                if !matches!(a, ExploreAction::None) {
                    action = a;
                }
            }
        }

        if self.selected != before {
            self.rest = 0.0;
            self.settled = false;
        }
        self.rest += dt;
        if self.rest >= PROBE_DELAY {
            if let Some(slug) = self.selected_entry().map(|e| e.slug.clone()) {
                if !self.probes.contains_key(&slug) && !self.pending.contains(&slug) {
                    self.pending.insert(slug.clone());
                    fetcher.submit(Job::Probe { slug });
                }
            }
        }

        if self.drag.as_ref().is_none_or(|d| !d.2) {
            let target = self.target_scroll();
            self.scroll = ui::approach(self.scroll, target, dt, 18.0);
        }
        let target_h = self.selected as f32 * ROW_H;
        self.highlight_y = ui::approach(self.highlight_y, target_h, dt, 22.0);
        self.settled = self.highlight_y == target_h && self.scroll == self.target_scroll() && self.rest > PROBE_DELAY;
        action
    }

    fn press(&mut self, b: Btn, n: usize, fetcher: &Fetcher) -> ExploreAction {
        match b {
            Btn::Up if n > 0 => self.selected = self.selected.saturating_sub(1),
            Btn::Down if n > 0 => self.selected = (self.selected + 1).min(n - 1),
            Btn::Left if n > 0 => self.selected = self.selected.saturating_sub(6),
            Btn::Right if n > 0 => self.selected = (self.selected + 6).min(n - 1),
            Btn::L | Btn::R => return ExploreAction::SwitchTab(Tab::Library),
            Btn::Triangle if self.catalog.is_some() => {
                self.searching = true;
                return ExploreAction::StartSearch;
            }
            Btn::Circle if !self.query.is_empty() => self.set_query(String::new()),
            Btn::Square if self.catalog.is_some() => {
                self.sort = self.sort.next();
                let keep = self.selected_entry().map(|e| e.slug.clone());
                self.rebuild_view(keep);
            }
            Btn::Select => self.refresh(fetcher),
            Btn::Cross => return self.primary(fetcher),
            _ => {}
        }
        ExploreAction::None
    }

    /// ✕: retry a failed catalog, or download / cancel / play the game.
    fn primary(&mut self, fetcher: &Fetcher) -> ExploreAction {
        if matches!(self.state, CatalogState::Failed(_)) {
            self.refresh(fetcher);
            return ExploreAction::None;
        }
        let Some(e) = self.selected_entry() else { return ExploreAction::None };
        if self.owns(e) {
            return ExploreAction::Play(e.file_name());
        }
        match &self.download {
            Some(d) if d.slug == e.slug => ExploreAction::CancelDownload,
            Some(_) => ExploreAction::None,
            None => {
                let (slug, file_name) = (e.slug.clone(), e.file_name());
                self.download = Some(Download { slug: slug.clone(), done: 0, total: self.probes.get(&slug).and_then(|p| p.total) });
                self.failed = None;
                ExploreAction::Download { slug, file_name }
            }
        }
    }

    fn touch(&mut self, phase: TouchPhase, x: f32, y: f32, n: usize, fetcher: &Fetcher) -> ExploreAction {
        match phase {
            TouchPhase::Down => {
                if self.tab_rects.first().is_some_and(|r| r.contains(x, y)) {
                    return ExploreAction::SwitchTab(Tab::Library);
                }
                if SEARCH.contains(x, y) && self.catalog.is_some() {
                    self.searching = true;
                    return ExploreAction::StartSearch;
                }
                let row = LIST.contains(x, y).then(|| ((y - LIST.y + self.scroll) / ROW_H) as usize).filter(|r| *r < n);
                self.drag = Some((y, self.scroll, false, row));
            }
            TouchPhase::Move => {
                let max = self.max_scroll();
                if let Some(d) = &mut self.drag {
                    if (y - d.0).abs() > 10.0 {
                        d.2 = true;
                    }
                    if d.2 && d.3.is_some() {
                        self.scroll = (d.1 - (y - d.0)).clamp(0.0, max);
                    }
                }
            }
            TouchPhase::Up => {
                let Some((_, _, moved, row)) = self.drag.take() else { return ExploreAction::None };
                if moved {
                    return ExploreAction::None;
                }
                if let Some(row) = row {
                    if row == self.selected {
                        return self.primary(fetcher);
                    }
                    self.selected = row;
                } else if COVER.contains(x, y) {
                    return self.primary(fetcher);
                } else if let Some((_, b)) = self.footer_targets.iter().find(|(r, _)| r.contains(x, y)).copied() {
                    return self.press(b, n, fetcher);
                }
            }
        }
        ExploreAction::None
    }

    // --------------------------------------------------------------- draw

    pub fn draw(&mut self, g: &mut Gfx, fetcher: &Fetcher) {
        ui::background(g);
        let note = match (&self.catalog, &self.state) {
            (Some(c), _) if !self.query.is_empty() => format!("{} of {} games", group(self.view.len()), group(c.entries.len())),
            (Some(c), CatalogState::Fetching { .. }) => format!("{} games \u{00b7} refreshing\u{2026}", group(c.entries.len())),
            (Some(c), _) => format!("{} games", group(c.entries.len())),
            (None, _) => String::new(),
        };
        self.tab_rects = screens::top_bar(g, Tab::Explore, &note);

        if self.catalog.is_none() {
            self.draw_catalog_state(g);
            return;
        }

        ui::card(g, LIST_CARD, theme::RADIUS, theme::PAPER);
        self.draw_search(g);

        g.push_clip(LIST);
        if self.view.is_empty() {
            let msg = format!("No games match \u{201c}{}\u{201d}", self.query);
            let msg = g.ellipsize(FontId::Bold, 16.0, &msg, LIST.w - 40.0);
            g.text_mid_center(FontId::Bold, 16.0, LIST.center_x(), LIST.y + 60.0, theme::MUTED, &msg);
        } else {
            let hl = Rect::new(LIST.x, LIST.y + self.highlight_y - self.scroll, LIST.w, ROW_H - 4.0);
            g.rounded_outline(hl, 10.0, theme::BORDER, theme::INK, theme::SUN);
            let first = (self.scroll / ROW_H).floor().max(0.0) as usize;
            let last = (((self.scroll + LIST.h) / ROW_H).ceil() as usize).min(self.view.len());
            for i in first..last {
                let r = Rect::new(LIST.x, LIST.y + i as f32 * ROW_H - self.scroll, LIST.w, ROW_H - 4.0);
                let e = self.catalog.as_ref().unwrap().entries[self.view[i]].clone();
                self.want_cover(&e.slug, fetcher);
                self.draw_row(g, &e, r, i == self.selected);
            }
        }
        g.pop_clip();
        screens::scrollbar(g, LIST, self.scroll, self.view.len() as f32 * ROW_H);

        let primary = match self.selected_entry().cloned() {
            Some(e) => {
                self.want_cover(&e.slug, fetcher);
                self.draw_details(g, &e)
            }
            None => "",
        };
        let mut hints: Vec<(Btn, &str)> = vec![(Btn::Square, self.sort.label()), (Btn::Triangle, "Search")];
        if !self.query.is_empty() {
            hints.push((Btn::Circle, "Clear"));
        }
        if !primary.is_empty() {
            hints.push((Btn::Cross, primary));
        }
        self.footer_targets = ui::footer(g, &hints);
        ui::footer_note(g, &format!("Games from {}", catalog::SOURCE_HOST));
    }

    fn draw_catalog_state(&mut self, g: &mut Gfx) {
        let card = Rect::new(210.0, 130.0, 540.0, 260.0);
        let cx = card.center_x();
        match &self.state {
            CatalogState::Failed(e) => {
                ui::card(g, card, theme::RADIUS + 2.0, theme::PAPER);
                g.icon(Icon::Globe, cx - 28.0, card.y + 30.0, 56.0, theme::TOMATO);
                g.text_mid_center(FontId::Display, 26.0, cx, card.y + 116.0, theme::INK, "Couldn't load the catalog");
                let lines = g.wrap(FontId::Regular, 15.0, e, card.w - 60.0, 3);
                for (i, l) in lines.iter().enumerate() {
                    g.text_mid_center(FontId::Regular, 15.0, cx, card.y + 152.0 + i as f32 * 21.0, theme::MUTED, l);
                }
                self.footer_targets = ui::footer(g, &[(Btn::Cross, "Try again")]);
            }
            state => {
                ui::card(g, card, theme::RADIUS + 2.0, theme::PAPER);
                let t = crate::screens::time_secs();
                ui::spinner(g, cx, card.y + 66.0, 48.0, theme::INK, t);
                g.text_mid_center(FontId::Display, 26.0, cx, card.y + 128.0, theme::INK, "Opening the arcade\u{2026}");
                let detail = match state {
                    CatalogState::Fetching { bytes } if *bytes > 0 => {
                        format!("Downloading the {} catalog \u{00b7} {}", catalog::SOURCE_NAME, ui::human_size(*bytes))
                    }
                    _ => format!("Downloading the {} catalog", catalog::SOURCE_NAME),
                };
                g.text_mid_center(FontId::Bold, 15.0, cx, card.y + 166.0, theme::MUTED, &detail);
                g.text_mid_center(FontId::Regular, 14.0, cx, card.y + 200.0, theme::FAINT, "This needs Wi-Fi the first time; after that it's saved.");
                self.footer_targets = ui::footer(g, &[(Btn::L, "Library")]);
            }
        }
        ui::footer_note(g, &format!("Games from {}", catalog::SOURCE_HOST));
    }

    fn draw_search(&self, g: &mut Gfx) {
        let fill = if self.searching { theme::SUN } else { theme::PAPER_DIM };
        g.rounded_outline(SEARCH, 10.0, 2.0, theme::INK, fill);
        g.icon(Icon::Search, SEARCH.x + 12.0, SEARCH.center_y() - 10.0, 20.0, theme::ink_on(fill));
        let x = SEARCH.x + 42.0;
        let max_w = SEARCH.w - 42.0 - 44.0;
        if self.query.is_empty() && !self.searching {
            let n = self.catalog.as_ref().map_or(0, |c| c.entries.len());
            g.text_mid(FontId::Bold, 15.0, x, SEARCH.center_y(), theme::MUTED, &format!("Search {} games", group(n)));
        } else {
            let shown = tail_fit(g, &self.query, max_w);
            let w = g.text_mid(FontId::Bold, 15.0, x, SEARCH.center_y(), theme::INK, &shown);
            if self.searching {
                g.rect(Rect::new(x + w + 2.0, SEARCH.center_y() - 10.0, 2.0, 20.0), theme::INK);
            }
        }
        let glyph = if self.query.is_empty() || self.searching { Btn::Triangle } else { Btn::Circle };
        ui::button_glyph(g, glyph, SEARCH.right() - 22.0, SEARCH.center_y(), 22.0);
    }

    fn draw_row(&self, g: &mut Gfx, e: &Entry, r: Rect, selected: bool) {
        let thumb = Rect::new(r.x + 8.0, r.y + 6.0, 70.0, r.h - 12.0);
        g.rounded(thumb, 8.0, theme::INK);
        let inner = thumb.inset(2.0);
        match self.covers.get(&e.slug) {
            Some(tex) => g.image_cover(tex, inner, Color::hex(0xFFFFFF)),
            None => {
                g.push_clip(inner);
                ui::generated_cover(g, inner, &e.title, ui::cover_color(&e.slug), false);
                g.pop_clip();
            }
        }
        g.round_corners(inner, 6.0, theme::INK);

        let tx = thumb.right() + 12.0;
        let badge = if self.download.as_ref().is_some_and(|d| d.slug == e.slug) {
            Some(("SAVING", theme::SUN))
        } else if self.owns(e) {
            Some(("IN LIBRARY", theme::MINT))
        } else {
            None
        };
        let badge_w = badge.map_or(0.0, |(t, _)| g.measure(FontId::Bold, 11.0, t) + 16.0);
        let max_w = r.right() - tx - 12.0 - if badge_w > 0.0 { badge_w + 8.0 } else { 0.0 };
        let title = g.ellipsize(FontId::Bold, 16.5, &e.title, max_w);
        let ink = if selected { theme::ON_ACCENT } else { theme::INK };
        g.text_mid(FontId::Bold, 16.5, tx, r.y + r.h * 0.36, ink, &title);
        let mut sub = vec![ui::human_size(self.probes.get(&e.slug).and_then(|p| p.total).unwrap_or(e.size)), e.year().to_owned()];
        if let Some(Some(Ok(info))) = self.probes.get(&e.slug).map(|p| &p.info) {
            sub.insert(0, if info.as3 { "AS3".into() } else { "AS1/2".into() });
        }
        let sub_color = if selected { theme::ON_ACCENT.alpha(0.7) } else { theme::MUTED };
        g.text_mid(FontId::Regular, 13.0, tx, r.y + r.h * 0.7, sub_color, &sub.join("  \u{00b7}  "));
        if let Some((text, fill)) = badge {
            let b = Rect::new(r.right() - 10.0 - badge_w, r.center_y() - 11.0, badge_w, 22.0);
            g.rounded_outline(b, 6.0, 2.0, theme::INK, fill);
            g.text_mid_center(FontId::Bold, 11.0, b.center_x(), b.center_y(), theme::ON_ACCENT, text);
        }
    }

    /// Draws the selected game; returns the ✕ hint for the footer.
    fn draw_details(&self, g: &mut Gfx, e: &Entry) -> &'static str {
        // Cover: generated art, with the real cover as a sticker when there is one.
        g.rounded(COVER.offset(theme::SHADOW, theme::SHADOW), theme::RADIUS + 2.0, theme::INK);
        g.rounded(COVER, theme::RADIUS + 2.0, theme::INK);
        let inner = COVER.inset(theme::BORDER);
        g.push_clip(inner);
        let color = ui::cover_color(&e.slug);
        match self.covers.get(&e.slug) {
            Some(tex) => {
                g.rect(inner, color);
                g.pattern(ui::Pattern::Stripes, inner, theme::PAPER.alpha(0.16));
                let s = inner.h - 44.0;
                let sticker = Rect::new(inner.x + 22.0, inner.y + 20.0, s, s);
                ui::card_with(g, sticker, 12.0, theme::INK, theme::BORDER, 4.0);
                let img = sticker.inset(theme::BORDER);
                g.image_cover(tex, img, Color::hex(0xFFFFFF));
                g.round_corners(img, 9.0, theme::INK);
                let tx = sticker.right() + 22.0;
                let px = 34.0;
                let lines = g.wrap(FontId::Display, px, &e.title, inner.right() - tx - 18.0, 3);
                let mut cy = inner.center_y() - (lines.len() as f32 - 1.0) * px * 0.52;
                for l in &lines {
                    ui::shadow_text(g, FontId::Display, px, tx, cy, theme::PAPER, l);
                    cy += px * 1.04;
                }
            }
            None => ui::generated_cover(g, inner, &e.title, color, true),
        }
        g.pop_clip();
        g.round_corners(inner, theme::RADIUS - 1.0, theme::INK);

        let mut y = COVER.bottom() + 32.0;
        let title = g.ellipsize(FontId::Display, 30.0, &e.title, DETAIL.w);
        ui::shadow_text(g, FontId::Display, 30.0, DETAIL.x, y, theme::PAPER, &title);
        y += 28.0;

        let probe = self.probes.get(&e.slug);
        let mut x = DETAIL.x;
        match probe.and_then(|p| p.info.as_ref()) {
            Some(Ok(info)) => {
                let (label, c) = if info.as3 { ("AS3", theme::TOMATO) } else { ("AS1/2", theme::SUN) };
                x += ui::chip(g, x, y, label, c) + 7.0;
                x += ui::chip(g, x, y, &format!("SWF {}", info.version), theme::PAPER) + 7.0;
                if info.width > 0 {
                    x += ui::chip(g, x, y, &format!("{}\u{00d7}{}", info.width, info.height), theme::PAPER) + 7.0;
                }
                x += ui::chip(g, x, y, &format!("{:.0} fps", info.fps), theme::PAPER) + 7.0;
            }
            Some(Err(_)) => x += ui::chip(g, x, y, "No details", theme::PAPER) + 7.0,
            None => x += ui::chip(g, x, y, "Checking\u{2026}", theme::PAPER) + 7.0,
        }
        let size = probe.and_then(|p| p.total).unwrap_or(e.size);
        ui::chip(g, x, y, &ui::human_size(size), theme::PAPER);
        y += 46.0;

        // Action area.
        let area = Rect::new(DETAIL.x, y, DETAIL.w, 40.0);
        let downloading = self.download.as_ref().filter(|d| d.slug == e.slug);
        let hint = if let Some(d) = downloading {
            let total = d.total.unwrap_or(size).max(1);
            let right = format!("{} / {}", ui::human_size(d.done), ui::human_size(total));
            ui::progress_bar(g, area, d.done as f32 / total as f32, "Downloading\u{2026}", &right);
            "Cancel"
        } else if self.owns(e) {
            status_line(g, area, Icon::Check, theme::MINT, "In your library");
            "Play"
        } else if self.download.is_some() {
            status_line(g, area, Icon::Clock, theme::PAPER, "Another download is running");
            ""
        } else if let Some((_, err)) = self.failed.as_ref().filter(|(s, _)| *s == e.slug) {
            status_line(g, area, Icon::Warning, theme::TOMATO, err);
            "Try again"
        } else {
            let big = size > 20 * 1024 * 1024;
            let text = if big { "Large game: it may not fit in the Vita's memory" } else { "Downloads to your library" };
            status_line(g, area, Icon::Download, if big { theme::SUN } else { theme::PAPER }, text);
            "Download"
        };
        y += 56.0;
        let added = format!("Added {}", pretty_date(&e.date));
        g.icon(Icon::Clock, DETAIL.x - 2.0, y - 8.0, 16.0, theme::ON_BLUE_DIM);
        g.text_mid(FontId::Bold, 14.0, DETAIL.x + 20.0, y, theme::ON_BLUE_DIM, &added);
        hint
    }
}

/// An icon in a small outlined tile, then a line of text on the blue.
fn status_line(g: &mut Gfx, area: Rect, icon: Icon, fill: Color, text: &str) {
    let tile = Rect::new(area.x, area.y + 2.0, 36.0, 36.0);
    ui::card_with(g, tile, 9.0, fill, 2.0, 3.0);
    g.icon(icon, tile.x + 7.0, tile.y + 7.0, 22.0, theme::ink_on(fill));
    let text = g.ellipsize(FontId::Bold, 15.5, text, area.w - 52.0);
    g.text_mid(FontId::Bold, 15.5, tile.right() + 14.0, tile.center_y(), theme::PAPER, &text);
}

/// "5953" -> "5,953".
fn group(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// "2013-03-14" -> "14 March 2013".
fn pretty_date(date: &str) -> String {
    const MONTHS: [&str; 12] =
        ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    let mut p = date.split('-');
    match (p.next(), p.next().and_then(|m| m.parse::<usize>().ok()), p.next().and_then(|d| d.parse::<u32>().ok())) {
        (Some(y), Some(m @ 1..=12), Some(d)) => format!("{d} {} {y}", MONTHS[m - 1]),
        _ => date.to_owned(),
    }
}

/// The end of `text` that fits in `max_w` (the caret stays visible).
fn tail_fit(g: &mut Gfx, text: &str, max_w: f32) -> String {
    if g.measure(FontId::Bold, 15.0, text) <= max_w {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    for start in 0..chars.len() {
        let s: String = std::iter::once('\u{2026}').chain(chars[start..].iter().copied()).collect();
        if g.measure(FontId::Bold, 15.0, &s) <= max_w {
            return s;
        }
    }
    String::new()
}
