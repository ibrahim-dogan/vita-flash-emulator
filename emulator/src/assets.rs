//! Desktop-only: renders the LiveArea artwork (icon, boot image, background,
//! gate button) with the app's own renderer so it matches the UI exactly.
//!
//! `RUFFLEVITA_RENDER_ASSETS=<dir> cargo run` writes true-colour PNGs; the
//! Vita wants 8-bit palette PNGs, so `tools/make_livearea.py` converts them.

use glow::HasContext;

use crate::platform::SCREEN_H;
use crate::ui::{self, Color, FontId, Gfx, Rect, theme};

fn backdrop(g: &mut Gfx, w: f32, h: f32) {
    g.rect_v(Rect::new(0.0, 0.0, w, h), theme::BG_TOP, theme::BG_BOTTOM);
    g.glow(w * 0.18, 0.0, w * 0.5, h * 0.45, theme::ACCENT.alpha(0.28));
    g.glow(w * 0.92, h, w * 0.45, h * 0.45, theme::ACCENT2.alpha(0.24));
}

fn icon0(g: &mut Gfx) {
    // LiveArea shows this inside a circle: keep the mark centred, full bleed.
    let r = Rect::new(0.0, 0.0, 128.0, 128.0);
    g.rect_h(r, theme::ACCENT, theme::ACCENT2);
    g.rect_v(Rect::new(0.0, 0.0, 128.0, 64.0), Color::hex(0xFFFFFF).alpha(0.12), Color::hex(0xFFFFFF).alpha(0.0));
    let s = 70.0;
    let (x, y) = (29.0 + 3.0, 29.0);
    g.icon(ui::Icon::Play, x, y + 4.0, s, Color::hex(0x1A1040).alpha(0.35));
    g.icon(ui::Icon::Play, x, y, s, Color::hex(0xFFFFFF));
}

fn wordmark(g: &mut Gfx, cx: f32, cy: f32, logo: f32, px: f32) {
    let tw = g.measure(FontId::Bold, px, "RuffleVita");
    let gap = logo * 0.32;
    let total = logo + gap + tw;
    let x = cx - total * 0.5;
    ui::logo(g, Rect::new(x, cy - logo * 0.5, logo, logo));
    g.text_mid(FontId::Bold, px, x + logo + gap, cy, theme::TEXT, "RuffleVita");
}

fn pic0(g: &mut Gfx) {
    let (w, h) = (960.0, 544.0);
    backdrop(g, w, h);
    wordmark(g, w * 0.5, 230.0, 96.0, 64.0);
    g.text_mid_center(FontId::Regular, 22.0, w * 0.5, 320.0, theme::DIM, "Flash games on your PS Vita");
    g.text_mid_center(FontId::Regular, 17.0, w * 0.5, h - 40.0, theme::FAINT, ui::CREDIT);
}

fn bg0(g: &mut Gfx) {
    let (w, h) = (840.0, 500.0);
    backdrop(g, w, h);
    g.text_mid_right(FontId::Regular, 16.0, w - 28.0, h - 26.0, theme::FAINT, ui::CREDIT);
}

fn startup(g: &mut Gfx) {
    let (w, h) = (280.0, 158.0);
    backdrop(g, w, h);
    wordmark(g, w * 0.5, 70.0, 46.0, 30.0);
    g.text_mid_center(FontId::Regular, 13.0, w * 0.5, 124.0, theme::DIM, ui::CREDIT);
}

fn save(gl: &glow::Context, w: u32, h: u32, path: &std::path::Path) {
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
    let file = std::fs::File::create(path).expect("create png");
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().and_then(|mut wr| wr.write_image_data(&rows)).expect("write png");
    eprintln!("wrote {}", path.display());
}

pub fn render_all(g: &mut Gfx, gl: &glow::Context, dir: &std::path::Path) {
    std::fs::create_dir_all(dir).expect("create output dir");
    let jobs: [(&str, u32, u32, fn(&mut Gfx)); 4] = [
        ("icon0.png", 128, 128, icon0),
        ("pic0.png", 960, 544, pic0),
        ("bg0.png", 840, 500, bg0),
        ("startup.png", 280, 158, startup),
    ];
    for (name, w, h, draw) in jobs {
        g.clear(Color::hex(0x000000));
        draw(g);
        g.flush();
        save(gl, w, h, &dir.join(name));
    }
}
