//! A small batched 2D renderer for FlashVita's UI.
//!
//! Everything (text, rounded rectangles, soft shadows, icons) is drawn from
//! one RGBA atlas plus optional image textures, accumulated into a single
//! vertex buffer and flushed with a handful of draw calls per frame. Colours
//! are premultiplied. Anti-aliasing comes from rasterising shapes at their
//! exact on-screen pixel size, so no shader derivatives are needed (vitaGL
//! doesn't reliably have them).

use std::collections::HashMap;
use std::sync::Arc;

use glow::HasContext;

use super::icons::{self, Icon};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const fn hex(rgb: u32) -> Self {
        Color {
            r: ((rgb >> 16) & 0xff) as f32 / 255.0,
            g: ((rgb >> 8) & 0xff) as f32 / 255.0,
            b: (rgb & 0xff) as f32 / 255.0,
            a: 1.0,
        }
    }

    pub const fn alpha(self, a: f32) -> Self {
        Color { a: self.a * a, ..self }
    }

    pub fn mix(self, other: Color, t: f32) -> Color {
        let t = t.clamp(0.0, 1.0);
        Color {
            r: self.r + (other.r - self.r) * t,
            g: self.g + (other.g - self.g) * t,
            b: self.b + (other.b - self.b) * t,
            a: self.a + (other.a - self.a) * t,
        }
    }

    fn premul(self) -> [u8; 4] {
        let a = self.a.clamp(0.0, 1.0);
        let q = |c: f32| ((c.clamp(0.0, 1.0) * a) * 255.0 + 0.5) as u8;
        [q(self.r), q(self.g), q(self.b), (a * 255.0 + 0.5) as u8]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Rect { x, y, w, h }
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn center_y(&self) -> f32 {
        self.y + self.h * 0.5
    }
    pub fn center_x(&self) -> f32 {
        self.x + self.w * 0.5
    }
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }
    pub fn inset(&self, d: f32) -> Rect {
        Rect::new(self.x + d, self.y + d, (self.w - 2.0 * d).max(0.0), (self.h - 2.0 * d).max(0.0))
    }
    pub fn intersect(&self, o: &Rect) -> Rect {
        let x = self.x.max(o.x);
        let y = self.y.max(o.y);
        let r = self.right().min(o.right());
        let b = self.bottom().min(o.bottom());
        Rect::new(x, y, (r - x).max(0.0), (b - y).max(0.0))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FontId {
    Regular = 0,
    Bold = 1,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Vertex {
    pos: [f32; 2],
    uv: [f32; 2],
    color: [u8; 4],
}

#[derive(Clone, Copy, PartialEq)]
struct UvRect {
    u0: f32,
    v0: f32,
    u1: f32,
    v1: f32,
}

#[derive(Clone, Copy)]
struct Glyph {
    uv: UvRect,
    w: f32,
    h: f32,
    xmin: f32,
    ymin: f32,
    advance: f32,
}

struct DrawCmd {
    texture: glow::Texture,
    clip: Option<Rect>,
    first: i32,
    count: i32,
}

/// An RGBA image texture (e.g. a game thumbnail) owned by the UI.
pub struct Texture {
    gl: Arc<glow::Context>,
    tex: glow::Texture,
    pub w: u32,
    pub h: u32,
}

impl Drop for Texture {
    fn drop(&mut self) {
        unsafe { self.gl.delete_texture(self.tex) }
    }
}

const ATLAS_W: usize = 1024;
const ATLAS_H: usize = 1024;
/// Corner radii with pre-rasterised quarter circles; others snap to the nearest.
const RADII: [u32; 9] = [4, 6, 8, 10, 12, 16, 20, 24, 32];
const BLURS: [u32; 3] = [8, 16, 28];

struct Shelf {
    y: usize,
    h: usize,
    x: usize,
}

struct Atlas {
    tex: glow::Texture,
    pixels: Vec<u8>,
    shelves: Vec<Shelf>,
    next_y: usize,
    /// Rows [y0, y1) that must be re-uploaded.
    dirty: Option<(usize, usize)>,
}

impl Atlas {
    fn alloc(&mut self, w: usize, h: usize) -> Option<(usize, usize)> {
        let (w, h) = (w + 1, h + 1); // 1px gutter against bleeding
        let bucket = h.next_multiple_of(4);
        for shelf in &mut self.shelves {
            if shelf.h == bucket && shelf.x + w <= ATLAS_W {
                let x = shelf.x;
                shelf.x += w;
                return Some((x, shelf.y));
            }
        }
        if self.next_y + bucket > ATLAS_H || w > ATLAS_W {
            return None;
        }
        let y = self.next_y;
        self.next_y += bucket;
        self.shelves.push(Shelf { y, h: bucket, x: w });
        Some((0, y))
    }

    /// Stores coverage `alpha` (w*h) as premultiplied white.
    fn put_alpha(&mut self, x: usize, y: usize, w: usize, h: usize, alpha: &[u8]) {
        for row in 0..h {
            for col in 0..w {
                let a = alpha[row * w + col];
                let i = ((y + row) * ATLAS_W + x + col) * 4;
                self.pixels[i..i + 4].copy_from_slice(&[a, a, a, a]);
            }
        }
        let (y0, y1) = self.dirty.unwrap_or((y, y + h));
        self.dirty = Some((y0.min(y), y1.max(y + h)));
    }

    fn uv(&self, x: usize, y: usize, w: usize, h: usize) -> UvRect {
        UvRect {
            u0: x as f32 / ATLAS_W as f32,
            v0: y as f32 / ATLAS_H as f32,
            u1: (x + w) as f32 / ATLAS_W as f32,
            v1: (y + h) as f32 / ATLAS_H as f32,
        }
    }
}

pub struct Gfx {
    gl: Arc<glow::Context>,
    program: glow::Program,
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    u_screen: Option<glow::UniformLocation>,
    atlas: Atlas,
    /// End of the fixed (shapes) region; glyphs/icons above it can be evicted.
    static_shelves: usize,
    static_next_y: usize,
    fonts: [fontdue::Font; 2],
    glyphs: HashMap<(u8, u16, u16), Option<Glyph>>,
    icons: HashMap<(Icon, u16), UvRect>,
    white: UvRect,
    corners: Vec<(u32, UvRect)>,
    inv_corners: Vec<(u32, UvRect)>,
    shadows: Vec<(u32, UvRect)>,
    glow_uv: UvRect,
    verts: Vec<Vertex>,
    cmds: Vec<DrawCmd>,
    clip_stack: Vec<Rect>,
    pub width: f32,
    pub height: f32,
}

const VERT_SRC: &str = "#version 100
attribute vec2 a_pos;
attribute vec2 a_uv;
attribute vec4 a_color;
uniform vec2 u_screen;
varying vec2 v_uv;
varying vec4 v_color;
void main() {
    v_uv = a_uv;
    v_color = a_color;
    vec2 p = a_pos / u_screen * 2.0 - 1.0;
    gl_Position = vec4(p.x, -p.y, 0.0, 1.0);
}
";

const FRAG_SRC: &str = "#version 100
precision mediump float;
uniform sampler2D u_tex;
varying vec2 v_uv;
varying vec4 v_color;
void main() {
    gl_FragColor = texture2D(u_tex, v_uv) * v_color;
}
";

static FONT_REGULAR: &[u8] = include_bytes!("../../assets/fonts/Inter-Regular.ttf");
static FONT_BOLD: &[u8] = include_bytes!("../../assets/fonts/Inter-SemiBold.ttf");

impl Gfx {
    pub fn new(gl: Arc<glow::Context>, width: u32, height: u32) -> anyhow::Result<Self> {
        let font = |bytes: &[u8]| {
            fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default())
                .map_err(|e| anyhow::anyhow!("font: {e}"))
        };
        let fonts = [font(FONT_REGULAR)?, font(FONT_BOLD)?];

        unsafe {
            let vs = ruffle_render_glow::compile_shader(&gl, glow::VERTEX_SHADER, VERT_SRC)?;
            let fs = ruffle_render_glow::compile_shader(&gl, glow::FRAGMENT_SHADER, FRAG_SRC)?;
            let program = gl.create_program().map_err(anyhow::Error::msg)?;
            gl.attach_shader(program, vs);
            gl.attach_shader(program, fs);
            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                anyhow::bail!("UI shader link: {}", gl.get_program_info_log(program));
            }
            gl.delete_shader(vs);
            gl.delete_shader(fs);
            let attr = |name| {
                gl.get_attrib_location(program, name)
                    .ok_or_else(|| anyhow::anyhow!("missing attribute {name}"))
            };
            let (a_pos, a_uv, a_color) = (attr("a_pos")?, attr("a_uv")?, attr("a_color")?);
            let u_screen = gl.get_uniform_location(program, "u_screen");
            gl.use_program(Some(program));
            gl.uniform_1_i32(gl.get_uniform_location(program, "u_tex").as_ref(), 0);
            gl.use_program(None);

            let vao = gl.create_vertex_array().map_err(anyhow::Error::msg)?;
            let vbo = gl.create_buffer().map_err(anyhow::Error::msg)?;
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            let stride = std::mem::size_of::<Vertex>() as i32;
            gl.vertex_attrib_pointer_f32(a_pos, 2, glow::FLOAT, false, stride, 0);
            gl.enable_vertex_attrib_array(a_pos);
            gl.vertex_attrib_pointer_f32(a_uv, 2, glow::FLOAT, false, stride, 8);
            gl.enable_vertex_attrib_array(a_uv);
            gl.vertex_attrib_pointer_f32(a_color, 4, glow::UNSIGNED_BYTE, true, stride, 16);
            gl.enable_vertex_attrib_array(a_color);
            gl.bind_vertex_array(None);

            let tex = gl.create_texture().map_err(anyhow::Error::msg)?;
            gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            for (p, v) in [
                (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
            ] {
                gl.tex_parameter_i32(glow::TEXTURE_2D, p, v as i32);
            }
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA as i32,
                ATLAS_W as i32,
                ATLAS_H as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
            );

            let mut atlas = Atlas {
                tex,
                pixels: vec![0; ATLAS_W * ATLAS_H * 4],
                shelves: Vec::new(),
                next_y: 0,
                dirty: None,
            };

            // Solid white texel block for untextured shapes.
            let (wx, wy) = atlas.alloc(4, 4).unwrap();
            atlas.put_alpha(wx, wy, 4, 4, &[255; 16]);
            let white = atlas.uv(wx + 1, wy + 1, 2, 2);

            let mut corners = Vec::new();
            let mut inv_corners = Vec::new();
            for r in RADII {
                let n = (2 * r) as usize;
                let mut disc = vec![0u8; n * n];
                let mut inv = vec![0u8; n * n];
                for y in 0..n {
                    for x in 0..n {
                        let dx = x as f32 + 0.5 - r as f32;
                        let dy = y as f32 + 0.5 - r as f32;
                        let cov = (r as f32 - (dx * dx + dy * dy).sqrt() + 0.5).clamp(0.0, 1.0);
                        disc[y * n + x] = (cov * 255.0 + 0.5) as u8;
                        inv[y * n + x] = ((1.0 - cov) * 255.0 + 0.5) as u8;
                    }
                }
                let (x, y) = atlas.alloc(n, n).unwrap();
                atlas.put_alpha(x, y, n, n, &disc);
                corners.push((r, atlas.uv(x, y, n, n)));
                let (x, y) = atlas.alloc(n, n).unwrap();
                atlas.put_alpha(x, y, n, n, &inv);
                inv_corners.push((r, atlas.uv(x, y, n, n)));
            }

            let mut shadows = Vec::new();
            for b in BLURS {
                // Separable falloff: 2b ramp, 1px plateau, 2b ramp.
                let n = (4 * b + 1) as usize;
                let ramp = |i: usize| {
                    let t = if i <= 2 * b as usize { i } else { n - 1 - i } as f32 / (2 * b) as f32;
                    let t = t.clamp(0.0, 1.0);
                    t * t * (3.0 - 2.0 * t)
                };
                let mut data = vec![0u8; n * n];
                for y in 0..n {
                    for x in 0..n {
                        data[y * n + x] = (ramp(x) * ramp(y) * 255.0 + 0.5) as u8;
                    }
                }
                let (x, y) = atlas.alloc(n, n).unwrap();
                atlas.put_alpha(x, y, n, n, &data);
                shadows.push((b, atlas.uv(x, y, n, n)));
            }

            // Radial falloff for large soft glows (scaled up with linear filtering).
            let n = 64usize;
            let mut data = vec![0u8; n * n];
            for y in 0..n {
                for x in 0..n {
                    let dx = (x as f32 + 0.5) / n as f32 * 2.0 - 1.0;
                    let dy = (y as f32 + 0.5) / n as f32 * 2.0 - 1.0;
                    let t = (1.0 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0);
                    data[y * n + x] = (t * t * (3.0 - 2.0 * t) * 255.0 + 0.5) as u8;
                }
            }
            let (gx, gy) = atlas.alloc(n, n).unwrap();
            atlas.put_alpha(gx, gy, n, n, &data);
            let glow_uv = atlas.uv(gx, gy, n, n);

            let static_shelves = atlas.shelves.len();
            let static_next_y = atlas.next_y;
            Ok(Self {
                gl,
                program,
                vao,
                vbo,
                u_screen,
                atlas,
                static_shelves,
                static_next_y,
                fonts,
                glyphs: HashMap::new(),
                icons: HashMap::new(),
                white,
                corners,
                inv_corners,
                shadows,
                glow_uv,
                verts: Vec::with_capacity(16 * 1024),
                cmds: Vec::new(),
                clip_stack: Vec::new(),
                width: width as f32,
                height: height as f32,
            })
        }
    }

    /// Drops cached glyphs/icons when the atlas fills up.
    fn evict_dynamic(&mut self) {
        tracing::debug!("UI atlas full; evicting glyph cache");
        self.atlas.shelves.truncate(self.static_shelves);
        self.atlas.next_y = self.static_next_y;
        self.glyphs.clear();
        self.icons.clear();
    }

    fn alloc_dynamic(&mut self, w: usize, h: usize) -> Option<(usize, usize)> {
        if let Some(p) = self.atlas.alloc(w, h) {
            return Some(p);
        }
        self.evict_dynamic();
        self.atlas.alloc(w, h)
    }

    // ---------------------------------------------------------------- frame

    pub fn clear(&self, color: Color) {
        unsafe {
            self.gl.clear_color(color.r, color.g, color.b, 1.0);
            self.gl.clear(glow::COLOR_BUFFER_BIT);
        }
    }

    /// Uploads and draws everything queued since the last flush.
    pub fn flush(&mut self) {
        if self.cmds.is_empty() {
            return;
        }
        let gl = &self.gl;
        unsafe {
            if let Some((y0, y1)) = self.atlas.dirty.take() {
                gl.bind_texture(glow::TEXTURE_2D, Some(self.atlas.tex));
                gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
                gl.tex_sub_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    0,
                    y0 as i32,
                    ATLAS_W as i32,
                    (y1 - y0) as i32,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(Some(
                        &self.atlas.pixels[y0 * ATLAS_W * 4..y1 * ATLAS_W * 4],
                    )),
                );
            }

            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.viewport(0, 0, self.width as i32, self.height as i32);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::STENCIL_TEST);
            gl.disable(glow::CULL_FACE);
            gl.color_mask(true, true, true, true);
            gl.enable(glow::BLEND);
            gl.blend_equation(glow::FUNC_ADD);
            gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
            gl.use_program(Some(self.program));
            gl.uniform_2_f32(self.u_screen.as_ref(), self.width, self.height);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_vertex_array(Some(self.vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));
            let bytes = std::slice::from_raw_parts(
                self.verts.as_ptr() as *const u8,
                self.verts.len() * std::mem::size_of::<Vertex>(),
            );
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes, glow::STREAM_DRAW);

            let mut scissor_on = false;
            for cmd in &self.cmds {
                match cmd.clip {
                    Some(c) => {
                        if !scissor_on {
                            gl.enable(glow::SCISSOR_TEST);
                            scissor_on = true;
                        }
                        gl.scissor(
                            c.x as i32,
                            (self.height - c.bottom()) as i32,
                            c.w.ceil() as i32,
                            c.h.ceil() as i32,
                        );
                    }
                    None if scissor_on => {
                        gl.disable(glow::SCISSOR_TEST);
                        scissor_on = false;
                    }
                    None => {}
                }
                gl.bind_texture(glow::TEXTURE_2D, Some(cmd.texture));
                gl.draw_arrays(glow::TRIANGLES, cmd.first, cmd.count);
            }
            if scissor_on {
                gl.disable(glow::SCISSOR_TEST);
            }
            gl.bind_vertex_array(None);
            gl.use_program(None);
        }
        self.verts.clear();
        self.cmds.clear();
    }

    // ------------------------------------------------------------- clipping

    pub fn push_clip(&mut self, r: Rect) {
        let r = match self.clip_stack.last() {
            Some(outer) => outer.intersect(&r),
            None => r,
        };
        self.clip_stack.push(r);
    }

    pub fn pop_clip(&mut self) {
        self.clip_stack.pop();
    }

    // ----------------------------------------------------------- primitives

    fn push_quad(&mut self, texture: glow::Texture, p: [[f32; 2]; 4], uv: UvRect, colors: [Color; 4]) {
        let clip = self.clip_stack.last().copied();
        if let Some(c) = clip {
            if c.w <= 0.0 || c.h <= 0.0 {
                return;
            }
        }
        let start = self.verts.len() as i32;
        match self.cmds.last_mut() {
            Some(cmd) if cmd.texture == texture && cmd.clip == clip => cmd.count += 6,
            _ => self.cmds.push(DrawCmd { texture, clip, first: start, count: 6 }),
        }
        let c = colors.map(|c| c.premul());
        let uvs = [[uv.u0, uv.v0], [uv.u1, uv.v0], [uv.u1, uv.v1], [uv.u0, uv.v1]];
        for i in [0, 1, 2, 0, 2, 3] {
            self.verts.push(Vertex { pos: p[i], uv: uvs[i], color: c[i] });
        }
    }

    fn quad_uv(&mut self, r: Rect, uv: UvRect, color: Color) {
        let p = [[r.x, r.y], [r.right(), r.y], [r.right(), r.bottom()], [r.x, r.bottom()]];
        let tex = self.atlas.tex;
        self.push_quad(tex, p, uv, [color; 4]);
    }

    pub fn rect(&mut self, r: Rect, color: Color) {
        if r.w <= 0.0 || r.h <= 0.0 || color.a <= 0.0 {
            return;
        }
        let uv = self.white;
        self.quad_uv(r, uv, color);
    }

    /// Vertical gradient.
    pub fn rect_v(&mut self, r: Rect, top: Color, bottom: Color) {
        let p = [[r.x, r.y], [r.right(), r.y], [r.right(), r.bottom()], [r.x, r.bottom()]];
        let (tex, uv) = (self.atlas.tex, self.white);
        self.push_quad(tex, p, uv, [top, top, bottom, bottom]);
    }

    /// Horizontal gradient.
    pub fn rect_h(&mut self, r: Rect, left: Color, right: Color) {
        let p = [[r.x, r.y], [r.right(), r.y], [r.right(), r.bottom()], [r.x, r.bottom()]];
        let (tex, uv) = (self.atlas.tex, self.white);
        self.push_quad(tex, p, uv, [left, right, right, left]);
    }

    fn pick<'a>(list: &'a [(u32, UvRect)], want: f32) -> &'a (u32, UvRect) {
        list.iter()
            .min_by(|a, b| {
                (a.0 as f32 - want).abs().partial_cmp(&(b.0 as f32 - want).abs()).unwrap()
            })
            .unwrap()
    }

    /// Draws the four corners of `r` using quadrants of the given disc
    /// texture, returning the radius actually used.
    fn corners(&mut self, r: Rect, radius: f32, inverse: bool, color: Color) -> f32 {
        let list = if inverse { &self.inv_corners } else { &self.corners };
        let (rad, uv) = *Self::pick(list, radius.min(r.w * 0.5).min(r.h * 0.5));
        let rf = rad as f32;
        let (um, vm) = ((uv.u0 + uv.u1) * 0.5, (uv.v0 + uv.v1) * 0.5);
        let q = |u0, v0, u1, v1| UvRect { u0, v0, u1, v1 };
        self.quad_uv(Rect::new(r.x, r.y, rf, rf), q(uv.u0, uv.v0, um, vm), color);
        self.quad_uv(Rect::new(r.right() - rf, r.y, rf, rf), q(um, uv.v0, uv.u1, vm), color);
        self.quad_uv(Rect::new(r.x, r.bottom() - rf, rf, rf), q(uv.u0, vm, um, uv.v1), color);
        self.quad_uv(
            Rect::new(r.right() - rf, r.bottom() - rf, rf, rf),
            q(um, vm, uv.u1, uv.v1),
            color,
        );
        rf
    }

    pub fn rounded(&mut self, r: Rect, radius: f32, color: Color) {
        if r.w <= 0.0 || r.h <= 0.0 || color.a <= 0.0 {
            return;
        }
        if radius < 2.0 {
            return self.rect(r, color);
        }
        let rf = self.corners(r, radius, false, color);
        self.rect(Rect::new(r.x + rf, r.y, r.w - 2.0 * rf, r.h), color);
        self.rect(Rect::new(r.x, r.y + rf, rf, r.h - 2.0 * rf), color);
        self.rect(Rect::new(r.right() - rf, r.y + rf, rf, r.h - 2.0 * rf), color);
    }

    /// Rounded rectangle outline of the given thickness.
    pub fn rounded_outline(&mut self, r: Rect, radius: f32, thickness: f32, color: Color, fill: Color) {
        self.rounded(r, radius, color);
        self.rounded(r.inset(thickness), (radius - thickness).max(0.0), fill);
    }

    /// Paints the area outside a rounded rect's corners with `bg`, giving
    /// images drawn underneath rounded corners.
    pub fn round_corners(&mut self, r: Rect, radius: f32, bg: Color) {
        self.corners(r, radius, true, bg);
    }

    /// Soft drop shadow around `r`.
    pub fn shadow(&mut self, r: Rect, blur: f32, color: Color) {
        let (b, uv) = *Self::pick(&self.shadows, blur);
        let bf = b as f32;
        let outer = Rect::new(r.x - bf, r.y - bf, r.w + 2.0 * bf, r.h + 2.0 * bf);
        let e = 2.0 * bf; // corner extent
        let (um, vm) = ((uv.u0 + uv.u1) * 0.5, (uv.v0 + uv.v1) * 0.5);
        let xs = [outer.x, outer.x + e, outer.right() - e, outer.right()];
        let ys = [outer.y, outer.y + e, outer.bottom() - e, outer.bottom()];
        let us = [uv.u0, um, um, uv.u1];
        let vs = [uv.v0, vm, vm, uv.v1];
        if xs[2] < xs[1] || ys[2] < ys[1] {
            return;
        }
        for j in 0..3 {
            for i in 0..3 {
                let rr = Rect::new(xs[i], ys[j], xs[i + 1] - xs[i], ys[j + 1] - ys[j]);
                if rr.w > 0.0 && rr.h > 0.0 {
                    let q = UvRect { u0: us[i], v0: vs[j], u1: us[i + 1], v1: vs[j + 1] };
                    self.quad_uv(rr, q, color);
                }
            }
        }
    }

    /// Large soft radial glow centred on (`cx`, `cy`).
    pub fn glow(&mut self, cx: f32, cy: f32, rx: f32, ry: f32, color: Color) {
        let uv = self.glow_uv;
        self.quad_uv(Rect::new(cx - rx, cy - ry, rx * 2.0, ry * 2.0), uv, color);
    }

    pub fn circle(&mut self, cx: f32, cy: f32, radius: f32, color: Color) {
        let r = Rect::new(cx - radius, cy - radius, radius * 2.0, radius * 2.0);
        self.rounded(r, radius, color);
    }

    // ---------------------------------------------------------------- icons

    pub fn icon(&mut self, icon: Icon, x: f32, y: f32, size: f32, color: Color) {
        let px = size.round().max(4.0) as u16;
        let uv = match self.icons.get(&(icon, px)) {
            Some(uv) => *uv,
            None => {
                let n = px as usize;
                let alpha = icons::rasterize(icon, n);
                let Some((ax, ay)) = self.alloc_dynamic(n, n) else { return };
                self.atlas.put_alpha(ax, ay, n, n, &alpha);
                let uv = self.atlas.uv(ax, ay, n, n);
                self.icons.insert((icon, px), uv);
                uv
            }
        };
        let r = Rect::new(x.round(), y.round(), px as f32, px as f32);
        self.quad_uv(r, uv, color);
    }

    /// Draws an icon centred on (`cx`, `cy`) rotated by `angle` radians.
    pub fn icon_rotated(&mut self, icon: Icon, cx: f32, cy: f32, size: f32, angle: f32, color: Color) {
        // Rasterise via the axis-aligned path to populate the cache.
        let px = size.round().max(4.0) as u16;
        if !self.icons.contains_key(&(icon, px)) {
            let n = px as usize;
            let alpha = icons::rasterize(icon, n);
            let Some((ax, ay)) = self.alloc_dynamic(n, n) else { return };
            self.atlas.put_alpha(ax, ay, n, n, &alpha);
            let uv = self.atlas.uv(ax, ay, n, n);
            self.icons.insert((icon, px), uv);
        }
        let uv = self.icons[&(icon, px)];
        let h = px as f32 * 0.5;
        let (s, c) = angle.sin_cos();
        let rot = |x: f32, y: f32| [cx + x * c - y * s, cy + x * s + y * c];
        let p = [rot(-h, -h), rot(h, -h), rot(h, h), rot(-h, h)];
        let tex = self.atlas.tex;
        self.push_quad(tex, p, uv, [color; 4]);
    }

    // ----------------------------------------------------------------- text

    fn glyph(&mut self, font: FontId, index: u16, px: f32) -> Option<Glyph> {
        let key = (font as u8, index, (px * 4.0) as u16);
        if let Some(g) = self.glyphs.get(&key) {
            return *g;
        }
        let f = &self.fonts[font as usize];
        let (m, bitmap) = f.rasterize_indexed(index, px);
        let glyph = if m.width == 0 || m.height == 0 {
            Some(Glyph {
                uv: self.white,
                w: 0.0,
                h: 0.0,
                xmin: 0.0,
                ymin: 0.0,
                advance: m.advance_width,
            })
        } else {
            self.alloc_dynamic(m.width, m.height).map(|(x, y)| {
                self.atlas.put_alpha(x, y, m.width, m.height, &bitmap);
                Glyph {
                    uv: self.atlas.uv(x, y, m.width, m.height),
                    w: m.width as f32,
                    h: m.height as f32,
                    xmin: m.xmin as f32,
                    ymin: m.ymin as f32,
                    advance: m.advance_width,
                }
            })
        };
        self.glyphs.insert(key, glyph);
        glyph
    }

    fn glyph_index(&self, font: FontId, c: char) -> u16 {
        let f = &self.fonts[font as usize];
        match f.lookup_glyph_index(c) {
            0 => f.lookup_glyph_index('?'),
            i => i,
        }
    }

    pub fn measure(&mut self, font: FontId, px: f32, text: &str) -> f32 {
        let mut w = 0.0;
        let mut prev = None;
        for c in text.chars() {
            let idx = self.glyph_index(font, c);
            if let Some(p) = prev {
                w += self.fonts[font as usize].horizontal_kern_indexed(p, idx, px).unwrap_or(0.0);
            }
            if let Some(g) = self.glyph(font, idx, px) {
                w += g.advance;
            }
            prev = Some(idx);
        }
        w
    }

    /// Draws `text` with its baseline at `baseline`; returns the advance.
    pub fn text(&mut self, font: FontId, px: f32, x: f32, baseline: f32, color: Color, text: &str) -> f32 {
        let mut pen = x;
        let mut prev = None;
        let base = baseline.round();
        for c in text.chars() {
            let idx = self.glyph_index(font, c);
            if let Some(p) = prev {
                pen += self.fonts[font as usize].horizontal_kern_indexed(p, idx, px).unwrap_or(0.0);
            }
            let Some(g) = self.glyph(font, idx, px) else { continue };
            if g.w > 0.0 {
                let r = Rect::new((pen + g.xmin).round(), base - g.ymin - g.h, g.w, g.h);
                self.quad_uv(r, g.uv, color);
            }
            pen += g.advance;
            prev = Some(idx);
        }
        pen - x
    }

    /// Draws text vertically centred on `cy` (cap-height centred).
    pub fn text_mid(&mut self, font: FontId, px: f32, x: f32, cy: f32, color: Color, s: &str) -> f32 {
        let baseline = cy + px * 0.36;
        self.text(font, px, x, baseline, color, s)
    }

    pub fn text_mid_right(&mut self, font: FontId, px: f32, right: f32, cy: f32, color: Color, s: &str) -> f32 {
        let w = self.measure(font, px, s);
        self.text_mid(font, px, right - w, cy, color, s);
        w
    }

    pub fn text_mid_center(&mut self, font: FontId, px: f32, cx: f32, cy: f32, color: Color, s: &str) {
        let w = self.measure(font, px, s);
        self.text_mid(font, px, cx - w * 0.5, cy, color, s);
    }

    /// Shortens `text` with an ellipsis so it fits in `max_w`.
    pub fn ellipsize(&mut self, font: FontId, px: f32, text: &str, max_w: f32) -> String {
        if self.measure(font, px, text) <= max_w {
            return text.to_owned();
        }
        let ell = "\u{2026}";
        let budget = max_w - self.measure(font, px, ell);
        let mut out = String::new();
        let mut w = 0.0;
        for c in text.chars() {
            let cw = self.measure(font, px, c.encode_utf8(&mut [0; 4]));
            if w + cw > budget {
                break;
            }
            w += cw;
            out.push(c);
        }
        out.truncate(out.trim_end().len());
        out.push_str(ell);
        out
    }

    /// Greedy word wrap into at most `max_lines` lines (the last ellipsized).
    pub fn wrap(&mut self, font: FontId, px: f32, text: &str, max_w: f32, max_lines: usize) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        let mut cur = String::new();
        let words: Vec<&str> = text.split_whitespace().collect();
        let mut i = 0;
        while i < words.len() {
            let candidate = if cur.is_empty() {
                words[i].to_owned()
            } else {
                format!("{cur} {}", words[i])
            };
            if self.measure(font, px, &candidate) <= max_w || cur.is_empty() {
                cur = candidate;
                i += 1;
            } else {
                lines.push(std::mem::take(&mut cur));
                if lines.len() == max_lines {
                    break;
                }
            }
        }
        if !cur.is_empty() && lines.len() < max_lines {
            lines.push(cur);
        }
        if i < words.len() || lines.last().is_some_and(|l| self.measure(font, px, l) > max_w) {
            if let Some(last) = lines.last_mut() {
                let rest = if i < words.len() {
                    format!("{last} {}", words[i..].join(" "))
                } else {
                    last.clone()
                };
                *last = self.ellipsize(font, px, &rest, max_w);
            }
        }
        lines
    }

    // --------------------------------------------------------------- images

    pub fn create_texture(&self, w: u32, h: u32, rgba: &[u8]) -> Option<Texture> {
        unsafe {
            let tex = self.gl.create_texture().ok()?;
            self.gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            for (p, v) in [
                (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
            ] {
                self.gl.tex_parameter_i32(glow::TEXTURE_2D, p, v as i32);
            }
            self.gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            self.gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA as i32,
                w as i32,
                h as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(rgba)),
            );
            Some(Texture { gl: self.gl.clone(), tex, w, h })
        }
    }

    /// Draws a sub-rectangle (in 0..1 texture space) of an image.
    pub fn image_uv(&mut self, tex: &Texture, dst: Rect, uv: [f32; 4], tint: Color) {
        let p = [[dst.x, dst.y], [dst.right(), dst.y], [dst.right(), dst.bottom()], [dst.x, dst.bottom()]];
        let uv = UvRect { u0: uv[0], v0: uv[1], u1: uv[2], v1: uv[3] };
        self.push_quad(tex.tex, p, uv, [tint; 4]);
    }

    /// Fills `dst` with the image, cropping to preserve its aspect ratio.
    pub fn image_cover(&mut self, tex: &Texture, dst: Rect, tint: Color) {
        let img_aspect = tex.w as f32 / tex.h.max(1) as f32;
        let dst_aspect = dst.w / dst.h.max(1.0);
        let uv = if img_aspect > dst_aspect {
            let f = dst_aspect / img_aspect;
            [0.5 - f * 0.5, 0.0, 0.5 + f * 0.5, 1.0]
        } else {
            let f = img_aspect / dst_aspect;
            [0.0, 0.5 - f * 0.5, 1.0, 0.5 + f * 0.5]
        };
        self.image_uv(tex, dst, uv, tint);
    }

    /// Fits the image inside `dst`, returning the rectangle it occupies.
    pub fn image_contain(&mut self, tex: &Texture, dst: Rect, tint: Color) -> Rect {
        let scale = (dst.w / tex.w as f32).min(dst.h / tex.h as f32);
        let (w, h) = ((tex.w as f32 * scale).round(), (tex.h as f32 * scale).round());
        let r = Rect::new(
            (dst.x + (dst.w - w) * 0.5).round(),
            (dst.y + (dst.h - h) * 0.5).round(),
            w,
            h,
        );
        self.image_uv(tex, r, [0.0, 0.0, 1.0, 1.0], tint);
        r
    }
}
