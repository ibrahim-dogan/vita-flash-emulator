# FlashVita

**Created by İbrahim Doğan.**

A Flash player for PS Vita, built on [Ruffle](https://ruffle.rs). One VPK:
launch it, pick a game from your library, play — with touch, button
mapping and a pause menu.

## Features

- **Library** with automatic cover art (pulled from bitmaps inside the SWF,
  replaced by an in-game screenshot after you've played a bit, or set
  manually from the pause menu), SWF details (AS2/AS3, version, stage size,
  fps, file size), play history, sort by recent / A–Z, subfolders.
- **Touch**: the front screen is the mouse. The rear touchpad can act as a
  trackpad (off by default so holding the Vita doesn't click things).
- **Button mapping per game**: every button (D-pad, ✕ ○ □ △, L, R, Start,
  Select) can send any keyboard key, a mouse click, or open the menu. Pick
  keys on an on-screen keyboard. Sticks: arrows / WASD / mouse cursor / off.
- **Pause menu** (Select, or L + R + Start always): resume, settings with live
  preview, set cover, restart, quit to library. Saves are flushed on pause.
- **Display**: fit / stretch / zoom / movie default, render quality, FPS
  counter, V-Sync.
- Games can load sibling files (external SWFs, XML) from their folder.

Default controls: D-pad & left stick = arrows, ✕ Space, ○ X, □ Z, △ C,
L Shift, R mouse click, Start Enter, Select menu, right stick = mouse cursor.

## Install

1. Install `dist/flashvita.vpk` with VitaShell.
2. Put `.swf` files in `ux0:data/FlashGames/` (subfolders are fine).
3. Launch FlashVita. Settings, covers, saves and `log.txt` live in
   `ux0:data/flashvita/`.

## Build

```bash
./build.sh   # Docker: VitaSDK + SDL2 (vitaGL backend) + vitaGL + cargo-vita
```

Artifacts land in `./dist/`. The first build compiles all of Ruffle with fat
LTO and takes a long time (longer under emulation on Apple Silicon).

### Desktop development

The app also runs on a desktop (macOS uses a GL 3.2 core context; shaders
are translated on the fly), which is much faster to iterate on:

```bash
cd emulator
FLASHVITA_GAMES=/path/to/swfs cargo run
```

Keyboard stands in for the Vita: arrows = D-pad, X/Enter = ✕, C/Esc = ○,
Z = □, V = △, Q/E = L/R, Space = Start, Tab = Select; the mouse is the touch
screen. `FLASHVITA_SCRIPT` automates input and screenshots, e.g.
`"wait 30; press Cross; sleep 4000; shot game.png; quit"`.

### LiveArea artwork

The icon, boot image and LiveArea background are rendered by the app itself
so they match the UI, then converted to the 8-bit palette PNGs the Vita needs:

```bash
cd emulator && FLASHVITA_RENDER_ASSETS=/tmp/art cargo run
python3 ../tools/make_livearea.py /tmp/art
```

## Layout

- `emulator/src/main.rs` — app state machine and frame loop
- `emulator/src/session.rs` — a running game: Ruffle player + input mapping
- `emulator/src/screens/` — library, loading, pause, settings, key picker
- `emulator/src/ui/` — batched 2D renderer (font atlas, SDF icons), theme
- `emulator/src/worker.rs` — background thread: movie loading, SWF analysis, covers
- `emulator/src/swfinfo.rs` — fast header parsing, cover extraction
- `emulator/ruffle_render_glow/` — GLES2 Ruffle renderer tuned for vitaGL

## Credits

- **İbrahim Doğan** — FlashVita: app, UI, input mapping, library, renderer work.
- [Ruffle](https://github.com/ruffle-rs/ruffle) — the Flash Player emulator core.
- [Fancy2209/ruffle4consoles](https://github.com/Fancy2209/ruffle4consoles) —
  the original Vita/Switch glue this started from.
- [Rinnegatamante](https://github.com/Rinnegatamante) — vitaGL, vitaShaRK.
- [Northfear/SDL](https://github.com/Northfear/SDL) — SDL2 with the vitaGL backend.
- [vita-rust](https://github.com/vita-rust) — Rust toolchain for the Vita.
- [Inter](https://rsms.me/inter/) — UI font (SIL OFL, `emulator/assets/fonts/`).
