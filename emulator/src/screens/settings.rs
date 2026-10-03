//! Per-game settings: button bindings, sticks and display options. Used from
//! both the library and the in-game pause menu (where changes apply live).

use crate::config::{PhysicsMode, Profile, Quality, ScaleMode, StickMode};
use crate::input::{Btn, InputEvent, Panel, TouchPhase};
use crate::platform::{SCREEN_H, SCREEN_W};
use crate::screens::keypicker::{KeyPicker, PickResult};
use crate::ui::{self, FontId, Gfx, Icon, Rect, theme};

pub enum SettingsResult {
    None,
    /// Something changed that should apply immediately (in-game preview).
    Changed,
    Close,
    MakeDefault,
}

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Controls,
    Display,
}

#[derive(Clone, Copy, PartialEq)]
enum Row {
    Button(Btn),
    LeftStick,
    RightStick,
    ResetControls,
    Scale,
    Quality,
    Physics,
    ShowFps,
    CursorSpeed,
    RearTouch,
    VSync,
    MakeDefault,
}

const PANEL: Rect = Rect::new(96.0, 28.0, 768.0, 462.0);
const LIST_TOP: f32 = PANEL.y + 112.0;
const LIST_H: f32 = PANEL.h - 124.0;
const ROW_H: f32 = 42.0;

pub struct SettingsScreen {
    pub game_name: String,
    pub profile: Profile,
    pub vsync: bool,
    tab: Tab,
    cursor: usize,
    scroll: f32,
    picker: Option<KeyPicker>,
    tab_rects: [Rect; 2],
    touch_row: Option<usize>,
}

impl SettingsScreen {
    pub fn new(game_name: String, profile: Profile, vsync: bool) -> Self {
        Self {
            game_name,
            profile,
            vsync,
            tab: Tab::Controls,
            cursor: 0,
            scroll: 0.0,
            picker: None,
            tab_rects: [Rect::default(); 2],
            touch_row: None,
        }
    }

    fn rows(&self) -> Vec<Row> {
        match self.tab {
            Tab::Controls => {
                let mut v: Vec<Row> = Btn::ALL.iter().map(|b| Row::Button(*b)).collect();
                v.insert(4, Row::LeftStick);
                v.insert(5, Row::RightStick);
                v.push(Row::ResetControls);
                v
            }
            Tab::Display => vec![
                Row::Scale,
                Row::Quality,
                Row::Physics,
                Row::ShowFps,
                Row::CursorSpeed,
                Row::RearTouch,
                Row::VSync,
                Row::MakeDefault,
            ],
        }
    }

    fn switch_tab(&mut self) {
        self.tab = if self.tab == Tab::Controls { Tab::Display } else { Tab::Controls };
        self.cursor = 0;
        self.scroll = 0.0;
    }

    /// Cycles an option row; `dir` is +1/-1. Returns true if something changed.
    fn cycle(&mut self, row: Row, dir: i32) -> bool {
        fn step<T: Copy + PartialEq>(all: &[T], cur: T, dir: i32) -> T {
            let i = all.iter().position(|x| *x == cur).unwrap_or(0) as i32;
            all[(i + dir).rem_euclid(all.len() as i32) as usize]
        }
        let p = &mut self.profile;
        match row {
            Row::LeftStick => p.left_stick = step(&StickMode::ALL, p.left_stick, dir),
            Row::RightStick => p.right_stick = step(&StickMode::ALL, p.right_stick, dir),
            Row::Scale => p.scale = step(&ScaleMode::ALL, p.scale, dir),
            Row::Quality => p.quality = step(&Quality::ALL, p.quality, dir),
            Row::Physics => p.physics = step(&PhysicsMode::ALL, p.physics, dir),
            Row::ShowFps => p.show_fps = !p.show_fps,
            Row::RearTouch => p.rear_touch = !p.rear_touch,
            Row::VSync => self.vsync = !self.vsync,
            Row::CursorSpeed => p.cursor_speed = (p.cursor_speed as i32 + dir).clamp(1, 10) as u8,
            _ => return false,
        }
        true
    }

    fn activate(&mut self, row: Row) -> SettingsResult {
        match row {
            Row::Button(b) => {
                self.picker = Some(KeyPicker::new(b, self.profile.action(b)));
                SettingsResult::None
            }
            Row::ResetControls => {
                let d = Profile::default();
                self.profile.buttons = d.buttons;
                self.profile.left_stick = d.left_stick;
                self.profile.right_stick = d.right_stick;
                SettingsResult::Changed
            }
            Row::MakeDefault => SettingsResult::MakeDefault,
            other => {
                if self.cycle(other, 1) {
                    SettingsResult::Changed
                } else {
                    SettingsResult::None
                }
            }
        }
    }

    pub fn update(&mut self, events: &[InputEvent], dt: f32) -> SettingsResult {
        if let Some(picker) = &mut self.picker {
            match picker.update(events) {
                PickResult::None => return SettingsResult::None,
                PickResult::Cancel => {
                    self.picker = None;
                    return SettingsResult::None;
                }
                PickResult::Chosen(action) => {
                    let btn = picker.btn;
                    self.profile.set_action(btn, action);
                    self.picker = None;
                    return SettingsResult::Changed;
                }
            }
        }

        let rows = self.rows();
        let mut result = SettingsResult::None;
        for ev in events {
            let r = match ev {
                InputEvent::Button(b, true) => match b {
                    Btn::Up => {
                        self.cursor = (self.cursor + rows.len() - 1) % rows.len();
                        SettingsResult::None
                    }
                    Btn::Down => {
                        self.cursor = (self.cursor + 1) % rows.len();
                        SettingsResult::None
                    }
                    Btn::Left | Btn::Right => {
                        let dir = if *b == Btn::Left { -1 } else { 1 };
                        if self.cycle(rows[self.cursor], dir) { SettingsResult::Changed } else { SettingsResult::None }
                    }
                    Btn::L | Btn::R => {
                        self.switch_tab();
                        return SettingsResult::None;
                    }
                    Btn::Cross => self.activate(rows[self.cursor]),
                    Btn::Circle | Btn::Start => SettingsResult::Close,
                    _ => SettingsResult::None,
                },
                InputEvent::Touch(t) if t.panel == Panel::Front => {
                    let row_at = |y: f32, scroll: f32| -> Option<usize> {
                        if y < LIST_TOP || y >= LIST_TOP + LIST_H {
                            return None;
                        }
                        let i = ((y - LIST_TOP + scroll) / ROW_H) as usize;
                        (i < rows.len()).then_some(i)
                    };
                    match t.phase {
                        TouchPhase::Down => {
                            if !PANEL.contains(t.x, t.y) {
                                SettingsResult::Close
                            } else if let Some(i) = self.tab_rects.iter().position(|r| r.contains(t.x, t.y)) {
                                if (i == 0) != (self.tab == Tab::Controls) {
                                    self.switch_tab();
                                    return SettingsResult::None;
                                }
                                SettingsResult::None
                            } else {
                                self.touch_row = row_at(t.y, self.scroll).filter(|_| PANEL.contains(t.x, t.y));
                                if let Some(i) = self.touch_row {
                                    self.cursor = i;
                                }
                                SettingsResult::None
                            }
                        }
                        TouchPhase::Up => {
                            let hit = row_at(t.y, self.scroll);
                            if hit.is_some() && hit == self.touch_row {
                                self.touch_row = None;
                                self.activate(rows[self.cursor])
                            } else {
                                self.touch_row = None;
                                SettingsResult::None
                            }
                        }
                        TouchPhase::Move => SettingsResult::None,
                    }
                }
                _ => SettingsResult::None,
            };
            if !matches!(r, SettingsResult::None) {
                result = r;
                if matches!(result, SettingsResult::Close) {
                    return result;
                }
            }
        }

        // Keep the cursor row in view.
        let top = self.cursor as f32 * ROW_H;
        let mut target = self.scroll;
        if top < target {
            target = top;
        }
        if top + ROW_H > target + LIST_H {
            target = top + ROW_H - LIST_H;
        }
        let max = (rows.len() as f32 * ROW_H - LIST_H).max(0.0);
        self.scroll = ui::approach(self.scroll, target.clamp(0.0, max), dt, 20.0);
        result
    }

    fn value(&self, row: Row) -> (String, bool) {
        let p = &self.profile;
        let on_off = |b: bool| if b { "On".to_owned() } else { "Off".to_owned() };
        match row {
            Row::Button(b) => (p.action(b).label(), false),
            Row::LeftStick => (p.left_stick.label().into(), true),
            Row::RightStick => (p.right_stick.label().into(), true),
            Row::Scale => (p.scale.label().into(), true),
            Row::Quality => (p.quality.label().into(), true),
            Row::Physics => (p.physics.label().into(), true),
            Row::ShowFps => (on_off(p.show_fps), true),
            Row::RearTouch => (on_off(p.rear_touch), true),
            Row::VSync => (on_off(self.vsync), true),
            Row::CursorSpeed => (format!("{}", p.cursor_speed), true),
            Row::ResetControls | Row::MakeDefault => (String::new(), false),
        }
    }

    fn label(row: Row) -> (&'static str, Option<&'static str>) {
        match row {
            Row::Button(b) => (b.label(), None),
            Row::LeftStick => ("Left stick", None),
            Row::RightStick => ("Right stick", None),
            Row::ResetControls => ("Reset controls to defaults", None),
            Row::Scale => ("Screen fit", Some("How the game fills the 960\u{00d7}544 screen")),
            Row::Quality => ("Render quality", Some("Low turns off bitmap smoothing")),
            Row::Physics => ("Physics speed", Some("Fast speeds up physics-heavy games; a little looser")),
            Row::ShowFps => ("Show FPS counter", None),
            Row::CursorSpeed => ("Stick cursor speed", None),
            Row::RearTouch => ("Rear touchpad as trackpad", Some("Drag to move the cursor, tap to click")),
            Row::VSync => ("V-Sync (all games)", Some("Off can raise FPS but may tear")),
            Row::MakeDefault => ("Use these settings for new games", None),
        }
    }

    pub fn draw(&mut self, g: &mut Gfx) {
        g.rect(Rect::new(0.0, 0.0, SCREEN_W as f32, SCREEN_H as f32), theme::INK.alpha(0.55));
        ui::card(g, PANEL, theme::RADIUS + 2.0, theme::PAPER);

        let cy = PANEL.y + 38.0;
        g.icon(Icon::Sliders, PANEL.x + 28.0, cy - 12.0, 24.0, theme::INK);
        g.text_mid(FontId::Display, 26.0, PANEL.x + 62.0, cy + 1.0, theme::INK, "Game settings");
        let name = g.ellipsize(FontId::Bold, 15.0, &self.game_name, 330.0);
        g.text_mid_right(FontId::Bold, 15.0, PANEL.right() - 28.0, cy + 1.0, theme::MUTED, &name);

        // Tabs.
        let rects = ui::tabs(g, PANEL.x + 28.0, PANEL.y + 82.0, &["Controls", "Display"], (self.tab == Tab::Display) as usize);
        for (i, r) in rects.into_iter().enumerate().take(2) {
            self.tab_rects[i] = r;
        }

        // Rows.
        let rows = self.rows();
        let list = Rect::new(PANEL.x + 16.0, LIST_TOP, PANEL.w - 32.0, LIST_H);
        g.push_clip(list);
        for (i, row) in rows.iter().enumerate() {
            let r = Rect::new(list.x, list.y + i as f32 * ROW_H - self.scroll, list.w - 8.0, ROW_H - 4.0);
            if r.bottom() < list.y || r.y > list.bottom() {
                continue;
            }
            let selected = i == self.cursor;
            if selected {
                g.rounded_outline(r, 10.0, theme::BORDER, theme::INK, theme::SUN);
            }
            let mut lx = r.x + 16.0;
            match row {
                Row::Button(b) => {
                    let gw = ui::button_glyph_width(g, *b, 24.0);
                    ui::button_glyph(g, *b, lx + gw * 0.5, r.center_y(), 24.0);
                    lx += gw.max(24.0) + 12.0;
                }
                Row::LeftStick | Row::RightStick => {
                    g.icon(Icon::Stick, lx, r.center_y() - 12.0, 24.0, theme::INK);
                    lx += 36.0;
                }
                Row::ResetControls => {
                    g.icon(Icon::Refresh, lx, r.center_y() - 11.0, 22.0, theme::INK);
                    lx += 36.0;
                }
                Row::MakeDefault => {
                    g.icon(Icon::Check, lx, r.center_y() - 11.0, 22.0, theme::INK);
                    lx += 36.0;
                }
                _ => {}
            }
            let (label, help) = Self::label(*row);
            match (help, selected) {
                (Some(h), true) => {
                    g.text_mid(FontId::Bold, 15.5, lx, r.center_y() - 8.0, theme::INK, label);
                    g.text_mid(FontId::Regular, 12.0, lx, r.center_y() + 10.0, theme::INK.alpha(0.7), h);
                }
                _ => {
                    g.text_mid(FontId::Bold, 15.5, lx, r.center_y(), theme::INK, label);
                }
            }

            let (value, cyclable) = self.value(*row);
            if !value.is_empty() {
                let right = r.right() - 16.0;
                let vcolor = if selected { theme::INK } else { theme::MUTED };
                if cyclable && selected {
                    g.icon(Icon::ChevronRight, right - 16.0, r.center_y() - 8.0, 16.0, theme::INK);
                    let w = g.text_mid_right(FontId::Bold, 15.5, right - 22.0, r.center_y(), vcolor, &value);
                    g.icon(Icon::ChevronLeft, right - 22.0 - w - 22.0, r.center_y() - 8.0, 16.0, theme::INK);
                } else {
                    g.text_mid_right(FontId::Bold, 15.5, right, r.center_y(), vcolor, &value);
                }
            }
        }
        g.pop_clip();
        crate::screens::scrollbar(g, list, self.scroll, rows.len() as f32 * ROW_H);

        let action = match rows.get(self.cursor) {
            Some(Row::Button(_)) => "Change",
            Some(Row::ResetControls) => "Reset",
            Some(Row::MakeDefault) => "Apply",
            _ => "Toggle",
        };
        match &self.picker {
            Some(p) => p.draw(g),
            None => {
                ui::footer(g, &[(Btn::R, "Tab"), (Btn::Circle, "Done"), (Btn::Cross, action)]);
            }
        }
    }
}
