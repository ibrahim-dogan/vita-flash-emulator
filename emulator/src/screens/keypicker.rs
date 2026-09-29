//! On-screen keyboard for choosing what a Vita button sends.

use crate::config::Action;
use crate::input::{Btn, InputEvent, Panel, TouchPhase};
use crate::keys;
use crate::platform::{SCREEN_H, SCREEN_W};
use crate::ui::{self, Color, FontId, Gfx, Rect, theme};

pub enum PickResult {
    None,
    Cancel,
    Chosen(Action),
}

struct Key {
    action: Action,
    label: String,
    units: f32,
}

const ROWS: &[&[(&str, f32)]] = &[
    &[("Escape", 1.3), ("1", 1.0), ("2", 1.0), ("3", 1.0), ("4", 1.0), ("5", 1.0), ("6", 1.0), ("7", 1.0), ("8", 1.0), ("9", 1.0), ("0", 1.0), ("-", 1.0), ("=", 1.0), ("Backspace", 1.6)],
    &[("Tab", 1.4), ("Q", 1.0), ("W", 1.0), ("E", 1.0), ("R", 1.0), ("T", 1.0), ("Y", 1.0), ("U", 1.0), ("I", 1.0), ("O", 1.0), ("P", 1.0), ("[", 1.0), ("]", 1.0)],
    &[("A", 1.0), ("S", 1.0), ("D", 1.0), ("F", 1.0), ("G", 1.0), ("H", 1.0), ("J", 1.0), ("K", 1.0), ("L", 1.0), (";", 1.0), ("'", 1.0), ("Enter", 1.8)],
    &[("Shift", 1.8), ("Z", 1.0), ("X", 1.0), ("C", 1.0), ("V", 1.0), ("B", 1.0), ("N", 1.0), ("M", 1.0), (",", 1.0), (".", 1.0), ("/", 1.0), ("Ctrl", 1.4)],
    &[("Alt", 1.3), ("Space", 5.0), ("Left", 1.0), ("Up", 1.0), ("Down", 1.0), ("Right", 1.0), ("Delete", 1.2), ("PageUp", 1.3), ("PageDown", 1.3)],
];

const UNIT: f32 = 44.0;
const GAP: f32 = 5.0;
const KEY_H: f32 = 40.0;
const PANEL: Rect = Rect::new(48.0, 40.0, 864.0, 448.0);

pub struct KeyPicker {
    pub btn: Btn,
    rows: Vec<Vec<Key>>,
    sel: (usize, usize),
    rects: Vec<Vec<Rect>>,
    pressed_at: Option<(usize, usize)>,
}

impl KeyPicker {
    pub fn new(btn: Btn, current: Action) -> Self {
        let mut rows: Vec<Vec<Key>> = ROWS
            .iter()
            .map(|row| {
                row.iter()
                    .map(|(id, units)| {
                        let k = keys::key(id);
                        Key { action: Action::Key(k), label: keys::get(k).label.to_owned(), units: *units }
                    })
                    .collect()
            })
            .collect();
        rows.push(vec![
            Key { action: Action::None, label: "Nothing".into(), units: 4.0 },
            Key { action: Action::MouseLeft, label: "Mouse click".into(), units: 4.0 },
            Key { action: Action::Menu, label: "RuffleVita menu".into(), units: 4.0 },
        ]);

        // Lay out rows centred in the panel.
        let mut rects = Vec::new();
        let mut y = PANEL.y + 100.0;
        for (ri, row) in rows.iter().enumerate() {
            if ri == rows.len() - 1 {
                y += 14.0;
            }
            let width: f32 = row.iter().map(|k| k.units * UNIT + (k.units - 1.0).max(0.0) * GAP).sum::<f32>()
                + GAP * (row.len() as f32 - 1.0);
            let mut x = PANEL.center_x() - width * 0.5;
            let mut rr = Vec::new();
            for k in row {
                let w = k.units * UNIT + (k.units - 1.0).max(0.0) * GAP;
                rr.push(Rect::new(x.round(), y, w.round(), KEY_H));
                x += w + GAP;
            }
            rects.push(rr);
            y += KEY_H + GAP;
        }

        let sel = rows
            .iter()
            .enumerate()
            .find_map(|(ri, row)| row.iter().position(|k| k.action == current).map(|ci| (ri, ci)))
            .unwrap_or((2, 0));
        Self { btn, rows, sel, rects, pressed_at: None }
    }

    fn move_vert(&mut self, down: bool) {
        let (r, c) = self.sel;
        let nr = if down { (r + 1) % self.rows.len() } else { (r + self.rows.len() - 1) % self.rows.len() };
        let cx = self.rects[r][c].center_x();
        let nc = self.rects[nr]
            .iter()
            .enumerate()
            .min_by(|a, b| (a.1.center_x() - cx).abs().partial_cmp(&(b.1.center_x() - cx).abs()).unwrap())
            .map(|(i, _)| i)
            .unwrap_or(0);
        self.sel = (nr, nc);
    }

    pub fn update(&mut self, events: &[InputEvent]) -> PickResult {
        for ev in events {
            match ev {
                InputEvent::Button(b, true) => match b {
                    Btn::Left => {
                        let len = self.rows[self.sel.0].len();
                        self.sel.1 = (self.sel.1 + len - 1) % len;
                    }
                    Btn::Right => {
                        let len = self.rows[self.sel.0].len();
                        self.sel.1 = (self.sel.1 + 1) % len;
                    }
                    Btn::Up => self.move_vert(false),
                    Btn::Down => self.move_vert(true),
                    Btn::Cross => return PickResult::Chosen(self.rows[self.sel.0][self.sel.1].action),
                    Btn::Circle => return PickResult::Cancel,
                    _ => {}
                },
                InputEvent::Touch(t) if t.panel == Panel::Front => {
                    let hit = self.rects.iter().enumerate().find_map(|(ri, row)| {
                        row.iter().position(|r| r.contains(t.x, t.y)).map(|ci| (ri, ci))
                    });
                    match t.phase {
                        TouchPhase::Down => {
                            if let Some(h) = hit {
                                self.sel = h;
                            }
                            self.pressed_at = hit;
                            if hit.is_none() && !PANEL.contains(t.x, t.y) {
                                return PickResult::Cancel;
                            }
                        }
                        TouchPhase::Move => {
                            if let Some(h) = hit {
                                self.sel = h;
                            }
                        }
                        TouchPhase::Up => {
                            if hit.is_some() && hit == Some(self.sel) && self.pressed_at.is_some() {
                                let (r, c) = self.sel;
                                return PickResult::Chosen(self.rows[r][c].action);
                            }
                            self.pressed_at = None;
                        }
                    }
                }
                _ => {}
            }
        }
        PickResult::None
    }

    pub fn draw(&self, g: &mut Gfx) {
        g.rect(Rect::new(0.0, 0.0, SCREEN_W as f32, SCREEN_H as f32), Color::hex(0x000000).alpha(0.45));
        ui::panel(g, PANEL);
        let cy = PANEL.y + 44.0;
        let mut x = PANEL.x + 32.0;
        x += g.text_mid(FontId::Bold, 22.0, x, cy, theme::TEXT, "Bind") + 12.0;
        let gw = ui::button_glyph_width(g, self.btn, 28.0);
        ui::button_glyph(g, self.btn, x + gw * 0.5, cy, 28.0);
        x += gw + 12.0;
        g.text_mid(FontId::Regular, 16.0, x, cy + 1.0, theme::DIM, self.btn.label());
        g.text_mid_right(FontId::Regular, 14.0, PANEL.right() - 32.0, cy + 1.0, theme::FAINT, "Pick the key this button presses");

        for (ri, row) in self.rows.iter().enumerate() {
            for (ci, key) in row.iter().enumerate() {
                let r = self.rects[ri][ci];
                let selected = self.sel == (ri, ci);
                let special = ri == self.rows.len() - 1;
                let bg = if selected {
                    theme::ACCENT
                } else if special {
                    theme::PANEL_HI.mix(theme::ACCENT, 0.12)
                } else {
                    theme::PANEL_HI
                };
                if selected {
                    g.shadow(r, 8.0, theme::ACCENT.alpha(0.35));
                }
                g.rounded(r, 8.0, bg);
                let px = if key.label.chars().count() > 3 { 13.0 } else { 16.0 };
                let color = if selected { Color::hex(0xFFFFFF) } else { theme::TEXT };
                g.text_mid_center(FontId::Bold, px, r.center_x(), r.center_y(), color, &key.label);
            }
        }
        ui::footer(g, &[(Btn::Circle, "Cancel"), (Btn::Cross, "Choose")]);
    }
}
