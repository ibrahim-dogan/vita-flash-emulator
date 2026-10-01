//! Turns raw SDL events into Vita-shaped input: buttons, sticks and front /
//! rear touch. Both the UI and the in-game mapper consume these.

use std::time::{Duration, Instant};

use sdl2::controller::{Axis, Button};
use sdl2::event::Event;
use serde::{Deserialize, Serialize};

use crate::platform::{SCREEN_H, SCREEN_W};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Btn {
    Up,
    Down,
    Left,
    Right,
    Cross,
    Circle,
    Square,
    Triangle,
    L,
    R,
    Start,
    Select,
}

impl Btn {
    pub const ALL: [Btn; 12] = [
        Btn::Up,
        Btn::Down,
        Btn::Left,
        Btn::Right,
        Btn::Cross,
        Btn::Circle,
        Btn::Square,
        Btn::Triangle,
        Btn::L,
        Btn::R,
        Btn::Start,
        Btn::Select,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Btn::Up => "D-Pad Up",
            Btn::Down => "D-Pad Down",
            Btn::Left => "D-Pad Left",
            Btn::Right => "D-Pad Right",
            Btn::Cross => "Cross",
            Btn::Circle => "Circle",
            Btn::Square => "Square",
            Btn::Triangle => "Triangle",
            Btn::L => "L Button",
            Btn::R => "R Button",
            Btn::Start => "Start",
            Btn::Select => "Select",
        }
    }

    fn index(self) -> usize {
        self as usize
    }

    fn from_sdl(button: Button) -> Option<Btn> {
        Some(match button {
            Button::DPadUp => Btn::Up,
            Button::DPadDown => Btn::Down,
            Button::DPadLeft => Btn::Left,
            Button::DPadRight => Btn::Right,
            Button::A => Btn::Cross,
            Button::B => Btn::Circle,
            Button::X => Btn::Square,
            Button::Y => Btn::Triangle,
            Button::LeftShoulder => Btn::L,
            Button::RightShoulder => Btn::R,
            Button::Start => Btn::Start,
            Button::Back => Btn::Select,
            _ => return None,
        })
    }

    /// Desktop development: drive the "Vita" from a keyboard.
    #[cfg(not(target_os = "vita"))]
    fn from_keyboard(key: sdl2::keyboard::Scancode) -> Option<Btn> {
        use sdl2::keyboard::Scancode as S;
        Some(match key {
            S::Up => Btn::Up,
            S::Down => Btn::Down,
            S::Left => Btn::Left,
            S::Right => Btn::Right,
            S::X | S::Return => Btn::Cross,
            S::C | S::Escape => Btn::Circle,
            S::Z => Btn::Square,
            S::V => Btn::Triangle,
            S::Q => Btn::L,
            S::E => Btn::R,
            S::Space => Btn::Start,
            S::Tab => Btn::Select,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TouchPhase {
    Down,
    Move,
    Up,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Panel {
    Front,
    Rear,
}

#[derive(Clone, Copy, Debug)]
pub struct Touch {
    pub phase: TouchPhase,
    pub panel: Panel,
    pub finger: i64,
    /// Screen pixels (0..960, 0..544).
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Debug)]
pub enum InputEvent {
    Button(Btn, bool),
    Touch(Touch),
    Text(String),
    Backspace,
    /// Return on a keyboard: ends typing.
    Enter,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Stick {
    pub x: f32,
    pub y: f32,
}

impl Stick {
    /// Radial deadzone with the remaining range rescaled to 0..1.
    pub fn filtered(self) -> Stick {
        const DEADZONE: f32 = 0.22;
        let mag = (self.x * self.x + self.y * self.y).sqrt();
        if mag < DEADZONE {
            return Stick::default();
        }
        let scale = ((mag - DEADZONE) / (1.0 - DEADZONE)).min(1.0) / mag;
        Stick { x: self.x * scale, y: self.y * scale }
    }
}

const REPEAT_DELAY: Duration = Duration::from_millis(380);
const REPEAT_RATE: Duration = Duration::from_millis(70);

/// Aggregated input state; fed every SDL event, drained once per frame.
pub struct Input {
    held: [bool; 12],
    pub left: Stick,
    pub right: Stick,
    events: Vec<InputEvent>,
    /// Directional auto-repeat for menus (D-pad or left stick).
    nav_dir: Option<(Btn, Instant)>,
    stick_dir: Option<Btn>,
    pub quit: bool,
    /// While typing into a text box, the desktop keyboard types instead of
    /// standing in for the Vita's buttons.
    pub typing: bool,
    #[cfg(not(target_os = "vita"))]
    mouse_down: bool,
}

impl Input {
    pub fn new() -> Self {
        Self {
            held: [false; 12],
            left: Stick::default(),
            right: Stick::default(),
            events: Vec::new(),
            nav_dir: None,
            stick_dir: None,
            quit: false,
            typing: false,
            #[cfg(not(target_os = "vita"))]
            mouse_down: false,
        }
    }

    pub fn held(&self, b: Btn) -> bool {
        self.held[b.index()]
    }

    fn set_button(&mut self, b: Btn, down: bool) {
        if self.held[b.index()] == down {
            return;
        }
        self.held[b.index()] = down;
        self.events.push(InputEvent::Button(b, down));
    }

    pub fn handle_sdl(&mut self, event: &Event) {
        match *event {
            Event::Quit { .. } => self.quit = true,
            Event::ControllerButtonDown { button, .. } => {
                if let Some(b) = Btn::from_sdl(button) {
                    self.set_button(b, true);
                }
            }
            Event::ControllerButtonUp { button, .. } => {
                if let Some(b) = Btn::from_sdl(button) {
                    self.set_button(b, false);
                }
            }
            Event::ControllerAxisMotion { axis, value, .. } => {
                let v = (value as f32 / 32767.0).clamp(-1.0, 1.0);
                match axis {
                    Axis::LeftX => self.left.x = v,
                    Axis::LeftY => self.left.y = v,
                    Axis::RightX => self.right.x = v,
                    Axis::RightY => self.right.y = v,
                    _ => {}
                }
            }
            Event::FingerDown { touch_id, finger_id, x, y, .. } => {
                self.push_touch(TouchPhase::Down, touch_id, finger_id, x, y)
            }
            Event::FingerMotion { touch_id, finger_id, x, y, .. } => {
                self.push_touch(TouchPhase::Move, touch_id, finger_id, x, y)
            }
            Event::FingerUp { touch_id, finger_id, x, y, .. } => {
                self.push_touch(TouchPhase::Up, touch_id, finger_id, x, y)
            }
            Event::TextInput { ref text, .. } => self.events.push(InputEvent::Text(text.clone())),
            #[cfg(not(target_os = "vita"))]
            Event::KeyDown { scancode: Some(sc), .. } if self.typing => {
                use sdl2::keyboard::Scancode as S;
                match sc {
                    S::Return | S::KpEnter | S::Escape => self.events.push(InputEvent::Enter),
                    S::Backspace => self.events.push(InputEvent::Backspace),
                    _ => {}
                }
            }
            #[cfg(not(target_os = "vita"))]
            Event::KeyDown { scancode: Some(sc), repeat: false, .. } => {
                if let Some(b) = Btn::from_keyboard(sc) {
                    self.set_button(b, true);
                } else if sc == sdl2::keyboard::Scancode::Backspace {
                    self.events.push(InputEvent::Backspace);
                }
            }
            #[cfg(not(target_os = "vita"))]
            Event::KeyUp { scancode: Some(sc), .. } => {
                if let Some(b) = Btn::from_keyboard(sc) {
                    self.set_button(b, false);
                }
            }
            // The Vita's on-screen keyboard sends these as keys, not text.
            #[cfg(target_os = "vita")]
            Event::KeyDown { scancode: Some(sc), .. } => match sc {
                sdl2::keyboard::Scancode::Backspace => self.events.push(InputEvent::Backspace),
                sdl2::keyboard::Scancode::Space => self.events.push(InputEvent::Text(" ".into())),
                sdl2::keyboard::Scancode::Return => self.events.push(InputEvent::Enter),
                _ => {}
            },
            // Desktop: the mouse stands in for the front touchscreen.
            #[cfg(not(target_os = "vita"))]
            Event::MouseButtonDown { mouse_btn: sdl2::mouse::MouseButton::Left, x, y, .. } => {
                self.mouse_down = true;
                self.push_mouse_touch(TouchPhase::Down, x, y);
            }
            #[cfg(not(target_os = "vita"))]
            Event::MouseMotion { x, y, .. } => {
                if self.mouse_down {
                    self.push_mouse_touch(TouchPhase::Move, x, y);
                }
            }
            #[cfg(not(target_os = "vita"))]
            Event::MouseButtonUp { mouse_btn: sdl2::mouse::MouseButton::Left, x, y, .. } => {
                self.mouse_down = false;
                self.push_mouse_touch(TouchPhase::Up, x, y);
            }
            _ => {}
        }
    }

    #[cfg(not(target_os = "vita"))]
    fn push_mouse_touch(&mut self, phase: TouchPhase, x: i32, y: i32) {
        self.events.push(InputEvent::Touch(Touch {
            phase,
            panel: Panel::Front,
            finger: 0,
            x: x as f32,
            y: y as f32,
        }));
    }

    fn push_touch(&mut self, phase: TouchPhase, touch_id: i64, finger: i64, x: f32, y: f32) {
        // SDL's Vita driver reports the front screen as a direct device and
        // the rear pad as an indirect one. Asking beats guessing ids.
        let direct = unsafe {
            sdl2::sys::SDL_GetTouchDeviceType(touch_id)
                == sdl2::sys::SDL_TouchDeviceType::SDL_TOUCH_DEVICE_DIRECT
        };
        self.events.push(InputEvent::Touch(Touch {
            phase,
            panel: if direct { Panel::Front } else { Panel::Rear },
            finger,
            x: x.clamp(0.0, 1.0) * SCREEN_W as f32,
            y: y.clamp(0.0, 1.0) * SCREEN_H as f32,
        }));
    }

    /// Synthesises a press for scripted runs (`RUFFLEVITA_SCRIPT`).
    pub fn inject(&mut self, b: Btn, down: bool) {
        self.set_button(b, down);
    }

    pub fn inject_event(&mut self, ev: InputEvent) {
        self.events.push(ev);
    }

    pub fn inject_touch(&mut self, phase: TouchPhase, x: f32, y: f32) {
        self.events.push(InputEvent::Touch(Touch { phase, panel: Panel::Front, finger: 0, x, y }));
    }

    /// Returns this frame's events, plus synthetic directional repeats for
    /// menu navigation when `nav` is set.
    pub fn drain(&mut self, nav: bool) -> Vec<InputEvent> {
        let mut events = std::mem::take(&mut self.events);
        if !nav {
            self.nav_dir = None;
            self.stick_dir = None;
            return events;
        }

        let now = Instant::now();
        for ev in &events {
            if let InputEvent::Button(b @ (Btn::Up | Btn::Down | Btn::Left | Btn::Right), down) = *ev {
                if down {
                    self.nav_dir = Some((b, now + REPEAT_DELAY));
                } else if matches!(self.nav_dir, Some((cur, _)) if cur == b) {
                    self.nav_dir = None;
                }
            }
        }

        // Left stick acts as a D-pad in menus.
        let s = self.left.filtered();
        let stick_dir = if s.x.abs().max(s.y.abs()) < 0.5 {
            None
        } else if s.x.abs() > s.y.abs() {
            Some(if s.x < 0.0 { Btn::Left } else { Btn::Right })
        } else {
            Some(if s.y < 0.0 { Btn::Up } else { Btn::Down })
        };
        if stick_dir != self.stick_dir {
            self.stick_dir = stick_dir;
            if let Some(d) = stick_dir {
                events.push(InputEvent::Button(d, true));
                events.push(InputEvent::Button(d, false));
                self.nav_dir = Some((d, now + REPEAT_DELAY));
            }
        }

        // Stop repeating once neither the D-pad nor the stick holds the direction.
        if let Some((b, next)) = self.nav_dir {
            if !self.held(b) && self.stick_dir != Some(b) {
                self.nav_dir = None;
            } else if now >= next {
                events.push(InputEvent::Button(b, true));
                events.push(InputEvent::Button(b, false));
                self.nav_dir = Some((b, now + REPEAT_RATE));
            }
        }
        events
    }

    /// Forget held state, e.g. when switching between menus and gameplay so
    /// no press leaks across.
    pub fn clear_events(&mut self) {
        self.events.clear();
        self.nav_dir = None;
    }
}
