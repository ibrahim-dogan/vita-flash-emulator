//! Home screen bubbles: an app per game that opens it straight away.
//!
//! The bubble is a copy of RuffleVita's own eboot plus a `game.txt` naming
//! the game (see `platform::bubble_game`), so Start goes right into the game
//! with one launch animation; a small app that started RuffleVita showed
//! two. It has its own title ID and param.sfo, the game's cover as icon and
//! LiveArea background, and a RuffleVita gate. With unsafe permissions
//! RuffleVita installs it itself through the system's promoter (as VitaShell
//! does); otherwise it saves a VPK in `<data>/bubbles/` to install with
//! VitaShell.

use std::path::{Path, PathBuf};

use crate::platform::{self, SCREEN_H, SCREEN_W};
use crate::swfinfo::Image;
use crate::ui::{self, Color, FontId, Gfx, Pattern, Rect, theme};

/// The package header the promoter wants next to an unpacked app
/// (`sce_sys/package/head.bin`): VitaShell's template (resources/head.bin),
/// with the content ID and digests filled in by `head_bin`.
static HEAD_BIN: &[u8] = include_bytes!("../assets/bubble/head.bin");

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
/// Without a cover, uses the generated one the library shows (`placeholder`).
pub fn render(g: &mut Gfx, gl: &glow::Context, cover: Option<&Image>, title: &str, placeholder: Color) -> Option<Vec<Picture>> {
    let tex = match cover {
        Some(c) => Some(g.create_texture(c.w, c.h, &c.rgba)?),
        None => None,
    };
    let jobs: [(&'static str, u32, u32); 4] =
        [("icon0.png", 128, 128), ("bg0.png", 840, 500), ("startup.png", 280, 158), ("pic0.png", 960, 544)];
    let mut out = Vec::new();
    for (name, w, h) in jobs {
        let full = Rect::new(0.0, 0.0, w as f32, h as f32);
        g.clear(Color::hex(0x000000));
        if name == "startup.png" || name == "pic0.png" {
            if name == "startup.png" {
                gate(g, full);
            } else {
                boot_image(g, full, title);
            }
            g.flush();
            out.push(Picture { name, w, h, rgba: read_back(gl, w, h) });
            continue;
        }
        match &tex {
            Some(tex) => g.image_cover(tex, full, Color::hex(0xFFFFFF)),
            // Initials on the small pictures, the title on the background.
            None => ui::generated_cover(g, full, title, placeholder, name == "bg0.png"),
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
/// RuffleVita's blue with its dots.
fn backdrop(g: &mut Gfx, r: Rect) {
    g.rect(r, theme::BLUE);
    g.pattern(Pattern::Dots, r, theme::PAPER.alpha(0.10));
}

/// The gate: RuffleVita's wordmark and credit on its blue, centred above
/// the system's Start label at the bottom.
fn gate(g: &mut Gfx, r: Rect) {
    backdrop(g, r);
    let size = 30.0;
    let w = ui::wordmark_width(g, size);
    ui::wordmark(g, ((r.w - w) * 0.5).round(), 58.0, size);
    g.text_mid_center(FontId::Bold, 14.0, r.w * 0.5, 94.0, theme::PAPER, ui::CREDIT);
}

/// The boot image the system shows while the bubble starts: RuffleVita's
/// own, with the game's title in place of the tagline.
fn boot_image(g: &mut Gfx, r: Rect, title: &str) {
    backdrop(g, r);
    let size = 92.0;
    let w = ui::wordmark_width(g, size);
    ui::wordmark(g, ((r.w - w) * 0.5).round(), 232.0, size);
    let title = g.ellipsize(FontId::Display, 34.0, title, r.w - 120.0);
    ui::shadow_text_center(g, FontId::Display, 34.0, r.w * 0.5, 332.0, theme::SUN, &title);
    g.text_mid_center(FontId::Bold, 18.0, r.w * 0.5, r.h - 40.0, theme::ON_BLUE_DIM, ui::CREDIT);
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

/// The game's path for `game.txt`, percent-encoded like a launcher's
/// `param=` (`platform::bubble_game` decodes it).
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

/// Where a bubble is unpacked for the installer, and the files that pass
/// what's going on between RuffleVita and the installer across the restart.
fn stage_dir() -> PathBuf {
    platform::data_dir().join("bubble_pkg")
}
fn install_file() -> PathBuf {
    platform::data_dir().join("bubble_install.txt")
}
fn result_file() -> PathBuf {
    platform::data_dir().join("bubble_result.txt")
}

/// Unpacks the bubble for the installer: its files, `head.bin`, and
/// `bubble_install.txt` with the title ID, game key and title.
pub fn stage(id: &str, game_key: &str, title: &str, files: &[(String, Vec<u8>)]) -> std::io::Result<()> {
    let dir = stage_dir();
    let _ = std::fs::remove_dir_all(&dir);
    let head = head_bin(id);
    let all = files.iter().map(|(n, b)| (n.as_str(), b.as_slice())).chain([("sce_sys/package/head.bin", &head[..])]);
    for (name, body) in all {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, body)?;
    }
    let _ = std::fs::remove_file(result_file());
    std::fs::write(install_file(), format!("{id}\n{game_key}\n{title}\n"))
}

/// Draws what the installer shows while it works (it has no renderer of its
/// own) into `bubble_screen.raw`: 960×544 RGBA, top row first.
pub fn write_install_screen(g: &mut Gfx, gl: &glow::Context, title: &str) -> std::io::Result<()> {
    let r = Rect::new(0.0, 0.0, SCREEN_W as f32, SCREEN_H as f32);
    g.clear(Color::hex(0x000000));
    backdrop(g, r);
    let size = 64.0;
    let w = ui::wordmark_width(g, size);
    ui::wordmark(g, ((r.w - w) * 0.5).round(), 196.0, size);
    let line = format!("Adding {title} to the home screen\u{2026}");
    let line = g.ellipsize(FontId::Display, 28.0, &line, r.w - 120.0);
    ui::shadow_text_center(g, FontId::Display, 28.0, r.w * 0.5, 292.0, theme::SUN, &line);
    g.text_mid_center(FontId::Bold, 17.0, r.w * 0.5, 338.0, theme::PAPER, "RuffleVita will be right back.");
    g.text_mid_center(FontId::Bold, 16.0, r.w * 0.5, r.h - 40.0, theme::ON_BLUE_DIM, ui::CREDIT);
    g.flush();
    std::fs::write(platform::data_dir().join("bubble_screen.raw"), read_back(gl, SCREEN_W, SCREEN_H))
}

/// What the installer did, read once when RuffleVita is back.
pub struct Installed {
    pub id: String,
    pub game_key: String,
    pub title: String,
    pub outcome: Outcome,
}

pub enum Outcome {
    Done,
    /// No permission: HENkaku's "Enable Unsafe Homebrew" is off.
    Denied,
    Failed(String),
}

/// The installer's report, if RuffleVita was just restarted by it.
pub fn take_installed() -> Option<Installed> {
    let result = std::fs::read_to_string(result_file()).ok()?;
    let _ = std::fs::remove_file(result_file());
    let _ = std::fs::remove_file(platform::data_dir().join("bubble_screen.raw"));
    let info = std::fs::read_to_string(install_file()).unwrap_or_default();
    let _ = std::fs::remove_file(install_file());
    let mut lines = info.lines();
    let (id, game_key, title) = (lines.next()?.to_owned(), lines.next()?.to_owned(), lines.next().unwrap_or("").to_owned());
    let result = result.trim();
    tracing::info!("bubble installer for {game_key}: {result}");
    let outcome = if result == "ok" {
        Outcome::Done
    } else if result.starts_with("denied") {
        Outcome::Denied
    } else {
        Outcome::Failed(result.trim_start_matches("error ").to_owned())
    };
    Some(Installed { id, game_key, title, outcome })
}

/// Turns what was unpacked for the installer into a VPK, and cleans up.
pub fn vpk_from_stage(id: &str, title: &str) -> std::io::Result<PathBuf> {
    fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
            if path.is_dir() {
                if rel != "sce_sys/package" {
                    collect(root, &path, out)?;
                }
            } else {
                out.push((rel, std::fs::read(&path)?));
            }
        }
        Ok(())
    }
    let dir = stage_dir();
    let mut files = Vec::new();
    collect(&dir, &dir, &mut files)?;
    let vpk = write_vpk(id, title, &files);
    let _ = std::fs::remove_dir_all(&dir);
    vpk
}

/// Removes what's left of an installed bubble's files.
pub fn clean_stage() {
    let _ = std::fs::remove_dir_all(stage_dir());
}

/// RuffleVita's own eboot, which the bubble runs.
fn own_eboot() -> std::io::Result<Vec<u8>> {
    #[cfg(target_os = "vita")]
    {
        std::fs::read("app0:eboot.bin")
    }
    #[cfg(not(target_os = "vita"))]
    {
        Ok(b"RuffleVita desktop build: no eboot".to_vec())
    }
}

/// About how much room a bubble takes on the memory card.
pub fn size_estimate() -> Option<u64> {
    #[cfg(target_os = "vita")]
    {
        std::fs::metadata("app0:eboot.bin").ok().map(|m| m.len() + 300 * 1024)
    }
    #[cfg(not(target_os = "vita"))]
    {
        None
    }
}

/// The bubble's files, by path inside the app.
pub fn files(id: &str, title: &str, game_path: &Path, pictures: &[Picture]) -> std::io::Result<Vec<(String, Vec<u8>)>> {
    let mut files = vec![
        ("eboot.bin".to_owned(), own_eboot()?),
        ("game.txt".to_owned(), encode_path(game_path).into_bytes()),
        ("sce_sys/param.sfo".to_owned(), sfo::build(id, title)),
        ("sce_sys/livearea/contents/template.xml".to_owned(), TEMPLATE_XML.as_bytes().to_vec()),
    ];
    for p in pictures {
        let name = match p.name {
            "icon0.png" | "pic0.png" => format!("sce_sys/{}", p.name),
            other => format!("sce_sys/livearea/contents/{other}"),
        };
        files.push((name, indexed_png(p)));
    }
    Ok(files)
}

/// Writes the bubble VPK and returns where it went.
pub fn write_vpk(id: &str, title: &str, files: &[(String, Vec<u8>)]) -> std::io::Result<PathBuf> {
    let mut zip = Zip::default();
    zip.data.reserve(files.iter().map(|(n, b)| b.len() + n.len() + 30).sum());
    for (name, body) in files {
        zip.add(name, body);
    }
    let dir = bubbles_dir();
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

/// Where bubble VPKs go when RuffleVita can't install them.
pub fn bubbles_dir() -> PathBuf {
    platform::data_dir().join("bubbles")
}

/// `HEAD_BIN` for one title: a made-up content ID and the three digests
/// the promoter checks, computed the way VitaShell's `makeHeadBin` does.
fn head_bin(title_id: &str) -> Vec<u8> {
    let mut h = HEAD_BIN.to_vec();
    let content_id = format!("EP9000-{title_id}_00-0000000000000000");
    h[0x30..0x30 + 48].fill(0);
    h[0x30..0x30 + content_id.len()].copy_from_slice(content_id.as_bytes());
    let be = |h: &[u8], at: usize| u32::from_be_bytes(h[at..at + 4].try_into().unwrap()) as usize;
    // The header.
    let len = be(&h, 0xD0);
    let mac = pkg_hmac(&h[..len]);
    h[len..len + 16].copy_from_slice(&mac);
    // The metadata.
    let (off, len, out) = (be(&h, 0x8), be(&h, 0x10), be(&h, 0xD4));
    let mac = pkg_hmac(&h[off..off + len - 64]);
    h[out..out + 16].copy_from_slice(&mac);
    // Everything before the last digest.
    let len = be(&h, 0xE8);
    let mac = pkg_hmac(&h[..len]);
    h[len..len + 16].copy_from_slice(&mac);
    h
}

/// The fake-PKG digest: SHA-1 of the data, shuffled into a 64-byte block and
/// hashed again, first 16 bytes.
fn pkg_hmac(data: &[u8]) -> [u8; 16] {
    let sha = sha1_smol::Sha1::from(data).digest().bytes();
    let mut buf = [0u8; 64];
    buf[0..8].copy_from_slice(&sha[4..12]);
    buf[8..16].copy_from_slice(&sha[4..12]);
    buf[16..20].copy_from_slice(&sha[12..16]);
    buf[20] = sha[16];
    buf[21..24].copy_from_slice(&sha[1..4]);
    buf.copy_within(16..24, 24);
    let mut mac = [0u8; 16];
    mac.copy_from_slice(&sha1_smol::Sha1::from(buf).digest().bytes()[..16]);
    mac
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
    fn head_bin_digests_fill_their_slots() {
        let h = head_bin("RUFV12345");
        assert_eq!(h.len(), HEAD_BIN.len());
        assert_eq!(&h[0x30..0x30 + 36], b"EP9000-RUFV12345_00-0000000000000000");
        // Same title, same bytes; another title changes the digests too.
        assert_eq!(h, head_bin("RUFV12345"));
        let other = head_bin("RUFV54321");
        let (a, b) = (0x100usize, 0x110usize);
        assert_ne!(h[a..b], other[a..b]);
    }

    #[test]
    fn paths_are_percent_encoded() {
        let p = Path::new("ux0:data/FlashGames/a koopa's revenge 2.swf");
        assert_eq!(encode_path(p), "ux0:data/FlashGames/a%20koopa%27s%20revenge%202.swf");
    }
}
