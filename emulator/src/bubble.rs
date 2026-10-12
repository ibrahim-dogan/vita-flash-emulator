//! Home screen bubbles: a small VPK per game that opens it in RuffleVita.
//!
//! The bubble is the forwarder in `bubble/forwarder.c` (one eboot for every
//! bubble), a `game.txt` naming the game, a param.sfo with its own title ID
//! and the game's cover as icon, LiveArea background and gate. The VPK lands
//! in `<data>/bubbles/` and is installed with VitaShell.

use std::path::{Path, PathBuf};

use crate::platform::{self, SCREEN_H};
use crate::swfinfo::Image;
use crate::ui::{self, Color, FontId, Gfx, Rect, theme};

/// Built from `bubble/forwarder.c` by `bubble/build.sh`.
static FORWARDER: &[u8] = include_bytes!("../assets/bubble/forwarder.bin");

const TEMPLATE_XML: &str = r#"<?xml version="1.0" encoding="utf-8"?>

<livearea style="a1" format-ver="01.00" content-rev="1">
	<livearea-background>
		<image>bg0.png</image>
	</livearea-background>

	<gate>
		<startup-image>startup.png</startup-image>
	</gate>
</livearea>
"#;

/// The bubble's title ID: `RUFV` and five digits from the game's key, so
/// making a bubble again for the same game updates it.
pub fn title_id(game_key: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in game_key.as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("RUFV{:05}", hash % 100_000)
}

/// One LiveArea image, straight RGBA.
pub struct Picture {
    pub name: &'static str,
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

/// Draws the icon, background and gate from the game's cover with the
/// launcher's own renderer and reads them back. Leaves the back buffer
/// dirty; the next frame redraws it.
pub fn render(g: &mut Gfx, gl: &glow::Context, cover: &Image, title: &str) -> Option<Vec<Picture>> {
    let tex = g.create_texture(cover.w, cover.h, &cover.rgba)?;
    let jobs: [(&'static str, u32, u32); 3] = [("icon0.png", 128, 128), ("bg0.png", 840, 500), ("startup.png", 280, 158)];
    let mut out = Vec::new();
    for (name, w, h) in jobs {
        let full = Rect::new(0.0, 0.0, w as f32, h as f32);
        g.clear(Color::hex(0x000000));
        g.image_cover(&tex, full, Color::hex(0xFFFFFF));
        if name == "startup.png" {
            gate(g, full, title);
        }
        g.flush();
        let mut rgba = read_back(gl, w, h);
        if name == "icon0.png" {
            round(&mut rgba, w, h);
        }
        out.push(Picture { name, w, h, rgba });
    }
    Some(out)
}

/// The gate: the cover, darkened at the bottom under the title and credit.
fn gate(g: &mut Gfx, r: Rect, title: &str) {
    let band = Rect::new(0.0, r.h - 62.0, r.w, 62.0);
    g.rect(band, theme::INK.alpha(0.78));
    let title = g.ellipsize(FontId::Display, 19.0, title, r.w - 20.0);
    ui::shadow_text_center(g, FontId::Display, 19.0, r.w * 0.5, band.y + 21.0, theme::PAPER, &title);
    let credit = format!("RuffleVita {}", ui::CREDIT);
    g.text_mid_center(FontId::Bold, 12.0, r.w * 0.5, band.y + 46.0, theme::SUN, &credit);
}

fn read_back(gl: &glow::Context, w: u32, h: u32) -> Vec<u8> {
    use glow::HasContext;
    let mut data = vec![0u8; (w * h * 4) as usize];
    unsafe {
        gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
        gl.read_pixels(
            0,
            (SCREEN_H - h) as i32,
            w as i32,
            h as i32,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(Some(&mut data)),
        );
    }
    let stride = (w * 4) as usize;
    let mut rows = Vec::with_capacity(data.len());
    for row in (0..h as usize).rev() {
        rows.extend_from_slice(&data[row * stride..(row + 1) * stride]);
    }
    for px in rows.chunks_exact_mut(4) {
        px[3] = 255;
    }
    rows
}

/// Makes everything outside the inscribed circle transparent, with a
/// one-pixel soft edge.
fn round(rgba: &mut [u8], w: u32, h: u32) {
    let (cx, cy, r) = (w as f32 * 0.5, h as f32 * 0.5, w.min(h) as f32 * 0.5);
    for y in 0..h {
        for x in 0..w {
            let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            let cover = (r - (dx * dx + dy * dy).sqrt() + 0.5).clamp(0.0, 1.0);
            let a = &mut rgba[((y * w + x) * 4 + 3) as usize];
            *a = (*a as f32 * cover).round() as u8;
        }
    }
}

/// An 8-bit palette PNG, the only kind LiveArea accepts.
fn indexed_png(p: &Picture) -> Vec<u8> {
    let quant = color_quant::NeuQuant::new(10, 256, &p.rgba);
    let map = quant.color_map_rgba();
    let indices: Vec<u8> = p.rgba.chunks_exact(4).map(|px| quant.index_of(px) as u8).collect();
    let palette: Vec<u8> = map.chunks_exact(4).flat_map(|c| [c[0], c[1], c[2]]).collect();
    let alpha: Vec<u8> = map.chunks_exact(4).map(|c| c[3]).collect();
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, p.w, p.h);
        enc.set_color(png::ColorType::Indexed);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_palette(palette);
        if alpha.iter().any(|&a| a != 255) {
            enc.set_trns(alpha);
        }
        let mut wr = enc.write_header().expect("png header");
        wr.write_image_data(&indices).expect("png data");
    }
    out
}

/// The game's path for `game.txt`, percent-encoded so the URI the forwarder
/// builds survives spaces and the like (`platform::launch_param` decodes it).
fn encode_path(path: &Path) -> String {
    let mut out = String::new();
    for b in path.to_string_lossy().bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'/' | b':' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Writes the bubble VPK and returns where it went.
pub fn write_vpk(game_key: &str, title: &str, game_path: &Path, pictures: &[Picture]) -> std::io::Result<PathBuf> {
    let id = title_id(game_key);
    let mut zip = Zip::default();
    zip.add("eboot.bin", FORWARDER);
    zip.add("game.txt", encode_path(game_path).as_bytes());
    zip.add("sce_sys/param.sfo", &sfo::build(&id, title));
    zip.add("sce_sys/livearea/contents/template.xml", TEMPLATE_XML.as_bytes());
    for p in pictures {
        let name = match p.name {
            "icon0.png" => "sce_sys/icon0.png".to_owned(),
            other => format!("sce_sys/livearea/contents/{other}"),
        };
        zip.add(&name, &indexed_png(p));
    }
    let dir = platform::data_dir().join("bubbles");
    std::fs::create_dir_all(&dir)?;
    let stem: String = title
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == ' ' || c == '-' { c } else { '_' })
        .collect::<String>()
        .trim()
        .chars()
        .take(40)
        .collect();
    let path = dir.join(format!("{} ({id}).vpk", if stem.is_empty() { "Game" } else { &stem }));
    std::fs::write(&path, zip.finish())?;
    Ok(path)
}

/// A zip with stored (uncompressed) entries: all a VPK needs.
#[derive(Default)]
struct Zip {
    data: Vec<u8>,
    central: Vec<u8>,
    count: u16,
}

impl Zip {
    fn add(&mut self, name: &str, body: &[u8]) {
        let crc = crc32fast::hash(body);
        let offset = self.data.len() as u32;
        let (size, name_len) = (body.len() as u32, name.len() as u16);
        // Version 2.0, no flags, stored, 1980-01-01 00:00.
        let common = |out: &mut Vec<u8>| {
            out.extend_from_slice(&20u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0x21u16.to_le_bytes());
            out.extend_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&size.to_le_bytes());
            out.extend_from_slice(&size.to_le_bytes());
            out.extend_from_slice(&name_len.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
        };
        self.data.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        common(&mut self.data);
        self.data.extend_from_slice(name.as_bytes());
        self.data.extend_from_slice(body);

        self.central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        self.central.extend_from_slice(&20u16.to_le_bytes());
        common(&mut self.central);
        // Comment length, disk, internal and external attributes.
        self.central.extend_from_slice(&[0u8; 10]);
        self.central.extend_from_slice(&offset.to_le_bytes());
        self.central.extend_from_slice(name.as_bytes());
        self.count += 1;
    }

    fn finish(mut self) -> Vec<u8> {
        let (start, len) = (self.data.len() as u32, self.central.len() as u32);
        self.data.append(&mut self.central);
        self.data.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        self.data.extend_from_slice(&[0u8; 4]);
        self.data.extend_from_slice(&self.count.to_le_bytes());
        self.data.extend_from_slice(&self.count.to_le_bytes());
        self.data.extend_from_slice(&len.to_le_bytes());
        self.data.extend_from_slice(&start.to_le_bytes());
        self.data.extend_from_slice(&0u16.to_le_bytes());
        self.data
    }
}

/// param.sfo with the same fields, values and layout as RuffleVita's own
/// (written by the VitaSDK `vita-mksfoex` that cargo-vita runs), which is
/// known to install. VitaShell fills in the empty CONTENT_ID itself.
mod sfo {
    enum Value<'a> {
        /// The string and its field size (0: the string rounded up to 4).
        Str(&'a str, u32),
        Int(u32),
    }

    pub fn build(title_id: &str, title: &str) -> Vec<u8> {
        use Value::{Int, Str};
        // Sorted by key (strcmp order), as the format requires.
        let entries: [(&str, Value); 19] = [
            ("APP_VER", Str("00.00", 0)),
            ("ATTRIBUTE", Int(0x8000)),
            ("ATTRIBUTE2", Int(0x0C)),
            ("ATTRIBUTE_MINOR", Int(0x10)),
            ("BOOT_FILE", Str("", 32)),
            ("CATEGORY", Str("gd", 0)),
            ("CONTENT_ID", Str("", 48)),
            ("EBOOT_APP_MEMSIZE", Int(0)),
            ("EBOOT_ATTRIBUTE", Int(0)),
            ("EBOOT_PHY_MEMSIZE", Int(0)),
            ("LAREA_TYPE", Int(0)),
            ("NP_COMMUNICATION_ID", Str("", 16)),
            ("PARENTAL_LEVEL", Int(0)),
            ("PSP2_DISP_VER", Str("00.000", 0)),
            ("PSP2_SYSTEM_VER", Int(0)),
            ("STITLE", Str(title, 52)),
            ("TITLE", Str(title, 0x80)),
            ("TITLE_ID", Str(title_id, 12)),
            ("VERSION", Str("00.00", 0)),
        ];
        let mut keys = Vec::new();
        let mut data = Vec::new();
        let mut index = Vec::new();
        for (key, value) in &entries {
            let key_off = keys.len() as u16;
            keys.extend_from_slice(key.as_bytes());
            keys.push(0);
            let data_off = data.len() as u32;
            let (kind, len, total) = match value {
                Int(v) => {
                    data.extend_from_slice(&v.to_le_bytes());
                    (4u8, 4, 4)
                }
                Str(s, size) => {
                    let s = if *size > 0 { truncate(s, *size as usize - 1) } else { s };
                    let len = s.len() as u32 + 1;
                    let total = if *size > 0 { *size } else { (len + 3) & !3 };
                    data.extend_from_slice(s.as_bytes());
                    data.resize(data_off as usize + total as usize, 0);
                    (2u8, len, total)
                }
            };
            index.extend_from_slice(&key_off.to_le_bytes());
            index.push(4); // alignment
            index.push(kind);
            index.extend_from_slice(&len.to_le_bytes());
            index.extend_from_slice(&total.to_le_bytes());
            index.extend_from_slice(&data_off.to_le_bytes());
        }
        while keys.len() % 4 != 0 {
            keys.push(0);
        }
        let key_table = 20 + index.len() as u32;
        let data_table = key_table + keys.len() as u32;
        let mut out = Vec::new();
        out.extend_from_slice(b"\0PSF");
        out.extend_from_slice(&0x0101u32.to_le_bytes());
        out.extend_from_slice(&key_table.to_le_bytes());
        out.extend_from_slice(&data_table.to_le_bytes());
        out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        out.extend_from_slice(&index);
        out.extend_from_slice(&keys);
        out.extend_from_slice(&data);
        out
    }

    /// At most `max` bytes, cut at a character boundary.
    fn truncate(s: &str, max: usize) -> &str {
        let mut end = s.len().min(max);
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        &s[..end]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_ids_look_like_title_ids() {
        let id = title_id("a koopa's revenge 2.swf");
        assert_eq!(id.len(), 9);
        assert!(id.starts_with("RUFV") && id[4..].chars().all(|c| c.is_ascii_digit()));
        assert_eq!(id, title_id("a koopa's revenge 2.swf"));
    }

    #[test]
    fn param_sfo_matches_rufflevitas_own_layout() {
        // RuffleVita 1.2.0's param.sfo is 912 bytes: 20-byte header, 19
        // entries, keys padded to 4, then the data.
        let sfo = sfo::build("RUFFLEVIT", "RuffleVita");
        assert_eq!(&sfo[..4], b"\0PSF");
        let word = |at: usize| u32::from_le_bytes(sfo[at..at + 4].try_into().unwrap());
        assert_eq!((word(8), word(12), word(16)), (324, 560, 19));
        assert_eq!(sfo.len(), 560 + 352);
    }

    #[test]
    fn paths_are_percent_encoded() {
        let p = Path::new("ux0:data/FlashGames/a koopa's revenge 2.swf");
        assert_eq!(encode_path(p), "ux0:data/FlashGames/a%20koopa%27s%20revenge%202.swf");
    }
}
