//! Desktop-only: renders the LiveArea artwork (icon, boot image, background,
//! gate button) with the app's own renderer so it matches the UI exactly.
//!
//! `RUFFLEVITA_RENDER_ASSETS=<dir> cargo run` writes true-colour PNGs; the
//! Vita wants 8-bit palette PNGs, so `tools/make_livearea.py` converts them.

use glow::HasContext;

use crate::platform::SCREEN_H;
use crate::ui::{self, Color, FontId, Gfx, Pattern, Rect, theme};

fn backdrop(g: &mut Gfx, w: f32, h: f32) {
    let r = Rect::new(0.0, 0.0, w, h);
    g.rect(r, theme::BLUE);
    g.pattern(Pattern::Dots, r, theme::PAPER.alpha(0.10));
}

fn icon0(g: &mut Gfx) {
    // LiveArea shows this inside a circle: keep the mark centred.
    backdrop(g, 128.0, 128.0);
    let s = 80.0;
    // Nudge up-left so the mark and its shadow are centred together.
    ui::logo(g, Rect::new(64.0 - s * 0.5 - 4.0, 64.0 - s * 0.5 - 4.0, s, s));
}

fn wordmark_centered(g: &mut Gfx, cx: f32, cy: f32, size: f32) {
    let w = ui::wordmark_width(g, size);
    ui::wordmark(g, (cx - w * 0.5).round(), cy, size);
}

fn pic0(g: &mut Gfx) {
    let (w, h) = (960.0, 544.0);
    backdrop(g, w, h);
    wordmark_centered(g, w * 0.5, 232.0, 92.0);
    ui::shadow_text_center(g, FontId::Display, 30.0, w * 0.5, 330.0, theme::SUN, "Flash games on your PS Vita");
    g.text_mid_center(FontId::Bold, 18.0, w * 0.5, h - 40.0, theme::ON_BLUE_DIM, ui::CREDIT);
}

fn bg0(g: &mut Gfx) {
    let (w, h) = (840.0, 500.0);
    backdrop(g, w, h);
    g.text_mid_right(FontId::Bold, 16.0, w - 28.0, h - 26.0, theme::ON_BLUE_DIM, ui::CREDIT);
}

fn startup(g: &mut Gfx) {
    let (w, h) = (280.0, 158.0);
    backdrop(g, w, h);
    wordmark_centered(g, w * 0.5, 66.0, 42.0);
    g.text_mid_center(FontId::Bold, 13.0, w * 0.5, 126.0, theme::PAPER, ui::CREDIT);
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
