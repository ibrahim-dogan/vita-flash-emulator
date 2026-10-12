//! Adding a game to the home screen (Options › Home screen): a preview of
//! the bubble, then installing it, or, when RuffleVita may not install apps,
//! saving a VPK and saying how to install it.

use crate::bubble::Picture;
use crate::input::{Btn, InputEvent, Panel, TouchPhase};
use crate::platform::{SCREEN_H, SCREEN_W};
use crate::ui::{self, Color, FontId, Gfx, Rect, Texture, theme};

pub enum BubbleAction {
    None,
    Close,
    /// Make it (draw one "Adding…" frame first: it takes a few seconds).
    Add,
    /// Close, and next time just say where the VPK went.
    HideHelp,
    /// Try the installer again next time (Unsafe Homebrew is on now).
    Retry,
}

pub enum Stage {
    Confirm,
    /// `drawn` once the "Adding…" frame is on screen.
    Working { drawn: bool },
    /// Saved as a VPK at this path; `denied` when the installer had no
    /// permission.
    Saved { path: String, denied: bool },
}

const PANEL: Rect = Rect::new(150.0, 70.0, 660.0, 392.0);
/// LiveArea's own proportions (840×500), scaled down.
const PREVIEW: Rect = Rect::new(PANEL.x + 28.0, PANEL.y + 92.0, 336.0, 200.0);

pub struct BubblePrompt {
    pub key: String,
    pub title: String,
    pub pictures: Vec<Picture>,
    pub stage: Stage,
    icon: Option<Texture>,
    background: Option<Texture>,
    /// Room the bubble takes on the memory card, if known.
    size: Option<u64>,
    footer_targets: Vec<(Rect, Btn)>,
}

impl BubblePrompt {
    pub fn new(g: &Gfx, key: String, title: String, pictures: Vec<Picture>, size: Option<u64>) -> Self {
        // The UI blends premultiplied colour; the pictures are straight.
        let texture = |name: &str| {
            let p = pictures.iter().find(|p| p.name == name)?;
            let mut rgba = p.rgba.clone();
            for px in rgba.chunks_exact_mut(4) {
                let a = u16::from(px[3]);
                for c in &mut px[..3] {
                    *c = (u16::from(*c) * a / 255) as u8;
                }
            }
            g.create_texture(p.w, p.h, &rgba)
        };
        let (icon, background) = (texture("icon0.png"), texture("bg0.png"));
        Self { key, title, pictures, stage: Stage::Confirm, icon, background, size, footer_targets: Vec::new() }
    }

    pub fn update(&mut self, events: &[InputEvent]) -> BubbleAction {
        for ev in events {
            let b = match ev {
                InputEvent::Button(b, true) => *b,
                InputEvent::Touch(t) if t.panel == Panel::Front && t.phase == TouchPhase::Up => {
                    match self.footer_targets.iter().find(|(r, _)| r.contains(t.x, t.y)) {
                        Some((_, b)) => *b,
                        None if !PANEL.contains(t.x, t.y) => Btn::Circle,
                        None => continue,
                    }
                }
                _ => continue,
            };
            match (&self.stage, b) {
                (Stage::Confirm, Btn::Cross) => {
                    self.stage = Stage::Working { drawn: false };
                    return BubbleAction::Add;
                }
                (Stage::Confirm, Btn::Circle) => return BubbleAction::Close,
                (Stage::Saved { .. }, Btn::Cross | Btn::Circle) => return BubbleAction::Close,
                (Stage::Saved { .. }, Btn::Square) => return BubbleAction::HideHelp,
                (Stage::Saved { denied: true, .. }, Btn::Triangle) => return BubbleAction::Retry,
                _ => {}
            }
        }
        BubbleAction::None
    }

    pub fn draw(&mut self, g: &mut Gfx) {
        g.rect(Rect::new(0.0, 0.0, SCREEN_W as f32, SCREEN_H as f32), theme::INK.alpha(0.55));
        ui::card(g, PANEL, theme::RADIUS + 2.0, theme::PAPER);
        let x = PANEL.x + 28.0;
        let heading = match self.stage {
            Stage::Saved { .. } => "Saved as a VPK",
            _ => "Add to the home screen",
        };
        g.text_mid(FontId::Display, 26.0, x, PANEL.y + 38.0, theme::INK, heading);
        let name = g.ellipsize(FontId::Bold, 15.0, &self.title, PANEL.w - 56.0);
        g.text_mid(FontId::Bold, 15.0, x, PANEL.y + 68.0, theme::MUTED, &name);

        match &self.stage {
            Stage::Saved { path, denied } => {
                let (path, denied) = (path.clone(), *denied);
                self.draw_saved(g, &path, denied);
                self.footer_targets = if denied {
                    ui::footer(g, &[(Btn::Triangle, "It's on now"), (Btn::Square, "Don't show again"), (Btn::Cross, "OK")])
                } else {
                    ui::footer(g, &[(Btn::Square, "Don't show again"), (Btn::Cross, "OK")])
                };
                return;
            }
            Stage::Working { .. } => {}
            Stage::Confirm => {}
        }

        // The LiveArea background with the round icon on it, as the home
        // screen will show them.
        g.rounded(PREVIEW.inset(-theme::BORDER), 10.0 + theme::BORDER, theme::INK);
        if let Some(bg) = &self.background {
            g.image_cover(bg, PREVIEW, Color::hex(0xFFFFFF));
        }
        g.round_corners(PREVIEW, 10.0, theme::INK);
        if let Some(icon) = &self.icon {
            let r = Rect::new(PREVIEW.x + 18.0, PREVIEW.bottom() - 18.0 - 96.0, 96.0, 96.0);
            // An ink ring: the icon's own round shape, inked and a bit bigger.
            g.image_cover(icon, r.inset(-3.0), Color::hex(0x000000));
            g.image_cover(icon, r, Color::hex(0xFFFFFF));
        }

        let tx = PREVIEW.right() + 26.0;
        let tw = PANEL.right() - 28.0 - tx;
        let mut y = PREVIEW.y + 14.0;
        match self.stage {
            Stage::Working { ref mut drawn } => {
                *drawn = true;
                ui::spinner(g, tx + 22.0, y + 22.0, 40.0, theme::INK, crate::screens::time_secs());
                y += 70.0;
                for line in g.wrap(FontId::Bold, 16.0, "Adding the bubble\u{2026}", tw, 2) {
                    g.text_mid(FontId::Bold, 16.0, tx, y, theme::INK, &line);
                    y += 22.0;
                }
            }
            _ => {
                let text = "A bubble that opens this game straight from the home screen, with its cover as the icon. Your saves and settings are shared with RuffleVita.";
                for line in g.wrap(FontId::Regular, 15.0, text, tw, 6) {
                    g.text_mid(FontId::Regular, 15.0, tx, y, theme::INK, &line);
                    y += 21.0;
                }
                y += 10.0;
                let size = self.size.map(|b| format!("It takes {} on the memory card. ", ui::human_size(b))).unwrap_or_default();
                let note = format!("{size}Delete it like any other bubble: hold it on the home screen.");
                for line in g.wrap(FontId::Regular, 13.0, &note, tw, 4) {
                    g.text_mid(FontId::Regular, 13.0, tx, y, theme::MUTED, &line);
                    y += 18.0;
                }
            }
        }
        self.footer_targets = match self.stage {
            Stage::Confirm => ui::footer(g, &[(Btn::Circle, "Cancel"), (Btn::Cross, "Add")]),
            _ => ui::footer(g, &[]),
        };
    }

    fn draw_saved(&self, g: &mut Gfx, path: &str, denied: bool) {
        let x = PANEL.x + 28.0;
        let w = PANEL.w - 56.0;
        let mut y = PANEL.y + 108.0;
        let text = if denied {
            "RuffleVita adds bubbles itself only when Enable Unsafe Homebrew is on (Settings \u{203a} HENkaku Settings). Turn it on and press Triangle, or install this file with VitaShell:"
        } else {
            "RuffleVita couldn't add the bubble itself, so it saved it as a file to install with VitaShell:"
        };
        for line in g.wrap(FontId::Regular, 15.0, text, w, 4) {
            g.text_mid(FontId::Regular, 15.0, x, y, theme::INK, &line);
            y += 21.0;
        }
        y += 14.0;
        let file = g.ellipsize(FontId::Bold, 15.0, path, w - 28.0);
        let pill = Rect::new(x, y - 18.0, g.measure(FontId::Bold, 15.0, &file) + 28.0, 36.0);
        g.rounded_outline(pill, 10.0, 2.0, theme::INK, theme::SUN);
        g.text_mid(FontId::Bold, 15.0, x + 14.0, pill.center_y(), theme::ON_ACCENT, &file);
        y += 42.0;
        let steps = "In VitaShell: open that folder, press X on the file and confirm. The bubble appears on the home screen.";
        for line in g.wrap(FontId::Regular, 15.0, steps, w, 3) {
            g.text_mid(FontId::Regular, 15.0, x, y, theme::MUTED, &line);
            y += 21.0;
        }
    }
}
