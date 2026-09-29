//! In-game menu (Select, or L+R+Start).

use crate::input::{Btn, InputEvent, Panel, TouchPhase};
use crate::platform::{SCREEN_H, SCREEN_W};
use crate::ui::{self, FontId, Gfx, Icon, Rect, theme};

#[derive(Clone, Copy, PartialEq)]
pub enum PauseAction {
    None,
    Resume,
    Settings,
    SetCover,
    Restart,
    Quit,
}

const ITEMS: [(PauseAction, Icon, &str); 5] = [
    (PauseAction::Resume, Icon::Play, "Resume"),
    (PauseAction::Settings, Icon::Sliders, "Controls & display"),
    (PauseAction::SetCover, Icon::Image, "Use this screen as cover"),
    (PauseAction::Restart, Icon::Refresh, "Restart game"),
    (PauseAction::Quit, Icon::Exit, "Quit to library"),
];

const PANEL: Rect = Rect::new(290.0, 70.0, 380.0, 382.0);
const ITEM_H: f32 = 52.0;
const ITEMS_TOP: f32 = PANEL.y + 96.0;

pub struct PauseMenu {
    sel: usize,
    touch: Option<usize>,
    /// Confirmation for destructive actions.
    confirm: Option<PauseAction>,
}

impl PauseMenu {
    pub fn new() -> Self {
        Self { sel: 0, touch: None, confirm: None }
    }

    fn item_rect(i: usize) -> Rect {
        Rect::new(PANEL.x + 16.0, ITEMS_TOP + i as f32 * ITEM_H, PANEL.w - 32.0, ITEM_H - 6.0)
    }

    fn choose(&mut self, i: usize) -> PauseAction {
        let action = ITEMS[i].0;
        if matches!(action, PauseAction::Restart | PauseAction::Quit) && self.confirm != Some(action) {
            self.confirm = Some(action);
            return PauseAction::None;
        }
        self.confirm = None;
        action
    }

    pub fn update(&mut self, events: &[InputEvent]) -> PauseAction {
        for ev in events {
            match ev {
                InputEvent::Button(b, true) => match b {
                    Btn::Up => {
                        self.sel = (self.sel + ITEMS.len() - 1) % ITEMS.len();
                        self.confirm = None;
                    }
                    Btn::Down => {
                        self.sel = (self.sel + 1) % ITEMS.len();
                        self.confirm = None;
                    }
                    Btn::Cross => {
                        let a = self.choose(self.sel);
                        if a != PauseAction::None {
                            return a;
                        }
                    }
                    Btn::Circle | Btn::Select | Btn::Start => {
                        if self.confirm.take().is_none() {
                            return PauseAction::Resume;
                        }
                    }
                    _ => {}
                },
                InputEvent::Touch(t) if t.panel == Panel::Front => {
                    let hit = (0..ITEMS.len()).find(|i| Self::item_rect(*i).contains(t.x, t.y));
                    match t.phase {
                        TouchPhase::Down => {
                            if !PANEL.contains(t.x, t.y) {
                                return PauseAction::Resume;
                            }
                            if let Some(h) = hit {
                                if h != self.sel {
                                    self.confirm = None;
                                }
                                self.sel = h;
                            }
                            self.touch = hit;
                        }
                        TouchPhase::Up => {
                            if hit.is_some() && hit == self.touch {
                                let a = self.choose(hit.unwrap());
                                if a != PauseAction::None {
                                    return a;
                                }
                            }
                            self.touch = None;
                        }
                        TouchPhase::Move => {}
                    }
                }
                _ => {}
            }
        }
        PauseAction::None
    }

    pub fn draw(&self, g: &mut Gfx, game_name: &str, fps: f32) {
        g.rect(Rect::new(0.0, 0.0, SCREEN_W as f32, SCREEN_H as f32), theme::INK.alpha(0.55));
        ui::card(g, PANEL, theme::RADIUS + 2.0, theme::PAPER);
        let name = g.ellipsize(FontId::Display, 24.0, game_name, PANEL.w - 56.0);
        g.text_mid(FontId::Display, 24.0, PANEL.x + 28.0, PANEL.y + 38.0, theme::INK, &name);
        let (h, m) = crate::platform::local_time();
        let info = format!("Paused \u{00b7} {fps:.0} FPS \u{00b7} {h:02}:{m:02}");
        g.text_mid(FontId::Bold, 13.5, PANEL.x + 28.0, PANEL.y + 66.0, theme::MUTED, &info);
        g.rect(Rect::new(PANEL.x + 28.0, PANEL.y + 84.0, PANEL.w - 56.0, 2.0), theme::INK.alpha(0.15));

        for (i, (action, icon, label)) in ITEMS.iter().enumerate() {
            let r = Self::item_rect(i);
            let selected = i == self.sel;
            let confirming = self.confirm == Some(*action);
            if selected {
                let fill = if confirming { theme::TOMATO } else { theme::SUN };
                g.rounded_outline(r, 10.0, theme::BORDER, theme::INK, fill);
            }
            g.icon(*icon, r.x + 16.0, r.center_y() - 11.0, 22.0, theme::INK);
            let text = if confirming { "Press Cross again to confirm" } else { label };
            g.text_mid(FontId::Bold, 16.5, r.x + 52.0, r.center_y(), theme::INK, text);
        }
        ui::footer(g, &[(Btn::Circle, "Resume"), (Btn::Cross, "Select")]);
    }
}
