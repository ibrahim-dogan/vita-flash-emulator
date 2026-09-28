//! The keyboard keys a Vita button can be bound to, and how each one is
//! presented to Ruffle.

use ruffle_core::events::{KeyDescriptor, KeyLocation, LogicalKey, NamedKey, PhysicalKey};

pub struct KeyInfo {
    /// Stable identifier used in profile files.
    pub id: &'static str,
    /// Label shown on the on-screen keyboard and in the bindings list.
    pub label: &'static str,
    pub physical: PhysicalKey,
    pub logical: LogicalKey,
    pub location: KeyLocation,
}

impl KeyInfo {
    pub fn descriptor(&self) -> KeyDescriptor {
        KeyDescriptor {
            physical_key: self.physical,
            logical_key: self.logical,
            key_location: self.location,
        }
    }
}

macro_rules! ch {
    ($id:literal, $phys:ident, $c:literal) => {
        KeyInfo {
            id: $id,
            label: $id,
            physical: PhysicalKey::$phys,
            logical: LogicalKey::Character($c),
            location: KeyLocation::Standard,
        }
    };
}

macro_rules! named {
    ($id:literal, $label:literal, $phys:ident, $named:ident, $loc:ident) => {
        KeyInfo {
            id: $id,
            label: $label,
            physical: PhysicalKey::$phys,
            logical: LogicalKey::Named(NamedKey::$named),
            location: KeyLocation::$loc,
        }
    };
}

pub static KEYS: &[KeyInfo] = &[
    named!("Escape", "Esc", Escape, Escape, Standard),
    ch!("1", Digit1, '1'),
    ch!("2", Digit2, '2'),
    ch!("3", Digit3, '3'),
    ch!("4", Digit4, '4'),
    ch!("5", Digit5, '5'),
    ch!("6", Digit6, '6'),
    ch!("7", Digit7, '7'),
    ch!("8", Digit8, '8'),
    ch!("9", Digit9, '9'),
    ch!("0", Digit0, '0'),
    ch!("-", Minus, '-'),
    ch!("=", Equal, '='),
    named!("Backspace", "Bksp", Backspace, Backspace, Standard),
    named!("Tab", "Tab", Tab, Tab, Standard),
    ch!("Q", KeyQ, 'q'),
    ch!("W", KeyW, 'w'),
    ch!("E", KeyE, 'e'),
    ch!("R", KeyR, 'r'),
    ch!("T", KeyT, 't'),
    ch!("Y", KeyY, 'y'),
    ch!("U", KeyU, 'u'),
    ch!("I", KeyI, 'i'),
    ch!("O", KeyO, 'o'),
    ch!("P", KeyP, 'p'),
    ch!("[", BracketLeft, '['),
    ch!("]", BracketRight, ']'),
    ch!("A", KeyA, 'a'),
    ch!("S", KeyS, 's'),
    ch!("D", KeyD, 'd'),
    ch!("F", KeyF, 'f'),
    ch!("G", KeyG, 'g'),
    ch!("H", KeyH, 'h'),
    ch!("J", KeyJ, 'j'),
    ch!("K", KeyK, 'k'),
    ch!("L", KeyL, 'l'),
    ch!(";", Semicolon, ';'),
    ch!("'", Quote, '\''),
    named!("Enter", "Enter", Enter, Enter, Standard),
    named!("Shift", "Shift", ShiftLeft, Shift, Left),
    ch!("Z", KeyZ, 'z'),
    ch!("X", KeyX, 'x'),
    ch!("C", KeyC, 'c'),
    ch!("V", KeyV, 'v'),
    ch!("B", KeyB, 'b'),
    ch!("N", KeyN, 'n'),
    ch!("M", KeyM, 'm'),
    ch!(",", Comma, ','),
    ch!(".", Period, '.'),
    ch!("/", Slash, '/'),
    named!("Ctrl", "Ctrl", ControlLeft, Control, Left),
    KeyInfo {
        id: "Space",
        label: "Space",
        physical: PhysicalKey::Space,
        logical: LogicalKey::Character(' '),
        location: KeyLocation::Standard,
    },
    named!("Alt", "Alt", AltLeft, Alt, Left),
    named!("Left", "\u{2190}", ArrowLeft, ArrowLeft, Standard),
    named!("Up", "\u{2191}", ArrowUp, ArrowUp, Standard),
    named!("Down", "\u{2193}", ArrowDown, ArrowDown, Standard),
    named!("Right", "\u{2192}", ArrowRight, ArrowRight, Standard),
    named!("F1", "F1", F1, F1, Standard),
    named!("F2", "F2", F2, F2, Standard),
    named!("Delete", "Del", Delete, Delete, Standard),
    named!("Home", "Home", Home, Home, Standard),
    named!("End", "End", End, End, Standard),
    named!("PageUp", "PgUp", PageUp, PageUp, Standard),
    named!("PageDown", "PgDn", PageDown, PageDown, Standard),
];

/// Index into [`KEYS`] for a profile id.
pub fn find(id: &str) -> Option<u16> {
    KEYS.iter()
        .position(|k| k.id.eq_ignore_ascii_case(id))
        .map(|i| i as u16)
}

pub fn get(index: u16) -> &'static KeyInfo {
    &KEYS[index as usize]
}

/// Shorthand for code that names keys it knows exist.
pub fn key(id: &str) -> u16 {
    find(id).unwrap_or_else(|| panic!("unknown key id {id}"))
}

/// Human-friendly name for bindings lists ("Space", "Arrow Up", "Z").
pub fn display_name(index: u16) -> String {
    let k = get(index);
    match k.id {
        "Left" => "Arrow Left".into(),
        "Up" => "Arrow Up".into(),
        "Down" => "Arrow Down".into(),
        "Right" => "Arrow Right".into(),
        "Escape" => "Esc".into(),
        id => id.into(),
    }
}
