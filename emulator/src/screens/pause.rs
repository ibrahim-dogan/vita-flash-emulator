//! In-game menu (Select, or L+R+Start).

use crate::input::{Btn, InputEvent, Panel, TouchPhase};
use crate::platform::{SCREEN_H, SCREEN_W};
use crate::ui::{self, Color, FontId, Gfx, Icon, Rect, theme};

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
        g.rect(Rect::new(0.0, 0.0, SCREEN_W as f32, SCREEN_H as f32), Color::hex(0x04060B).alpha(0.62));
        ui::panel(g, PANEL);
        let name = g.ellipsize(FontId::Bold, 20.0, game_name, PANEL.w - 56.0);
        g.text_mid(FontId::Bold, 20.0, PANEL.x + 28.0, PANEL.y + 38.0, theme::TEXT, &name);
        let (h, m) = crate::platform::local_time();
        let info = format!("Paused \u{00b7} {fps:.0} FPS \u{00b7} {h:02}:{m:02}");
        g.text_mid(FontId::Regular, 13.0, PANEL.x + 28.0, PANEL.y + 64.0, theme::DIM, &info);
        g.rect(Rect::new(PANEL.x + 28.0, PANEL.y + 84.0, PANEL.w - 56.0, 1.0), theme::LINE);

        for (i, (action, icon, label)) in ITEMS.iter().enumerate() {
            let r = Self::item_rect(i);
            let selected = i == self.sel;
            let confirming = self.confirm == Some(*action);
            if selected {
                let c = if confirming { theme::DANGER.alpha(0.22) } else { theme::PANEL_HI };
                g.rounded(r, 10.0, c);
                g.rounded(Rect::new(r.x, r.y + 12.0, 3.0, r.h - 24.0), 1.5, if confirming { theme::DANGER } else { theme::ACCENT });
            }
            let icolor = if confirming { theme::DANGER } else if selected { theme::ACCENT } else { theme::DIM };
            g.icon(*icon, r.x + 16.0, r.center_y() - 11.0, 22.0, icolor);
            let text = if confirming { "Press Cross again to confirm" } else { label };
            let color = if selected { theme::TEXT } else { theme::TEXT.alpha(0.85) };
            g.text_mid(FontId::Bold, 16.0, r.x + 52.0, r.center_y(), color, text);
        }
        ui::footer(g, &[(Btn::Circle, "Resume"), (Btn::Cross, "Select")]);
    }
}
