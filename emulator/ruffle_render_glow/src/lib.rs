//! OpenGL ES 2 (glow) render backend for Ruffle, tuned for vitaGL on PS Vita.
//!
//! Derived from Ruffle's WebGL backend. Differences worth knowing about:
//! - GL state is fully re-established at the start of every frame, so other
//!   GL users (RuffleVita's UI overlay) can draw between frames safely.
//! - Redundant state changes (stencil, texture parameters, gradient uniforms)
//!   are cached away; on vitaGL every GL call costs CPU time.
//! - `update_texture` / `resolve_sync_handle` handle partial regions
//!   correctly (BitmapData-heavy games rely on both).
//! - Shaders are GLSL ES 1.00; on desktop core-profile GL they are translated
//!   on the fly (see [`adapt_shader_source`]) so the app can be developed on a PC.
#![allow(clippy::arc_with_non_send_sync)]

use bytemuck::{Pod, Zeroable};
use glow::*;
use ruffle_render::backend::{
    BitmapCacheEntry, Context3D, Context3DProfile, PixelBenderOutput, PixelBenderTarget,
    RenderBackend, ShapeHandle, ShapeHandleImpl, ViewportDimensions,
};
use ruffle_render::bitmap::{
    Bitmap, BitmapFormat, BitmapHandle, BitmapHandleImpl, BitmapSource, PixelRegion, PixelSnapping,
    RgbaBufRead, SyncHandle,
};
use ruffle_render::commands::{CommandHandler, CommandList, RenderBlendMode};
use ruffle_render::error::Error as BitmapError;
use ruffle_render::matrix::Matrix;
use ruffle_render::quality::StageQuality;
use ruffle_render::shape_utils::{DistilledShape, GradientType};
use ruffle_render::tessellator::{
    Gradient as TessGradient, ShapeTessellator, Vertex as TessVertex,
};
use ruffle_render::transform::Transform;
use std::any::Any;
use std::borrow::Cow;
use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use swf::{BlendMode, Color, Twips};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Couldn't compile shader: {0}")]
    ShaderCompile(String),

    #[error("Couldn't link shader program: {0}")]
    LinkingShaderProgram(String),

    #[error("GL object creation failed: {0}")]
    GlCreate(String),
}

const COLOR_VERTEX_GLSL: &str = include_str!("../shaders/color.vert");
const COLOR_FRAGMENT_GLSL: &str = include_str!("../shaders/color.frag");
const TEXTURE_VERTEX_GLSL: &str = include_str!("../shaders/texture.vert");
const GRADIENT_FRAGMENT_GLSL: &str = include_str!("../shaders/gradient.frag");
const BITMAP_FRAGMENT_GLSL: &str = include_str!("../shaders/bitmap.frag");
const BATCH_VERTEX_GLSL: &str = include_str!("../shaders/batch.vert");
const BATCH_BITMAP_VERTEX_GLSL: &str = include_str!("../shaders/batch_bitmap.vert");
const BATCH_GRADIENT_VERTEX_GLSL: &str = include_str!("../shaders/batch_gradient.vert");
const BATCH_GRADIENT_FRAGMENT_GLSL: &str = include_str!("../shaders/batch_gradient.frag");

/// Translates a GLSL ES 1.00 shader into GLSL 1.50 when running on a desktop
/// core-profile context (macOS only offers 3.2+ core). On GLES (vitaGL) the
/// source is returned unchanged.
pub fn adapt_shader_source(gl: &glow::Context, stage: u32, src: &str) -> String {
    if gl.version().is_embedded {
        return src.to_owned();
    }
    let body = src.replace("#version 100", "");
    let mut out = String::from("#version 150\n");
    if stage == glow::VERTEX_SHADER {
        out.push_str("#define attribute in\n#define varying out\n");
    } else {
        out.push_str("#define varying in\n#define texture2D texture\nout vec4 fv_frag_color;\n");
    }
    out.push_str(&body.replace("gl_FragColor", "fv_frag_color"));
    out
}

/// Compiles a single shader stage, returning the info log on failure.
pub fn compile_shader(gl: &glow::Context, stage: u32, src: &str) -> Result<glow::Shader, Error> {
    unsafe {
        let shader = gl.create_shader(stage).map_err(Error::GlCreate)?;
        gl.shader_source(shader, &adapt_shader_source(gl, stage, src));
        gl.compile_shader(shader);
        if !gl.get_shader_compile_status(shader) {
            let log = gl.get_shader_info_log(shader);
            gl.delete_shader(shader);
            return Err(Error::ShaderCompile(log));
        }
        Ok(shader)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MaskState {
    NoMask,
    DrawMaskStencil,
    DrawMaskedContent,
    ClearMaskStencil,
}

static DRAW_CALLS: AtomicU32 = AtomicU32::new(0);

/// `BitmapData.draw` renders through one fixed-size framebuffer, in tiles,
/// and copies the result into the bitmap's texture. vitaGL creates a
/// render target and a depth buffer (4 bytes per pixel) whenever a
/// framebuffer's texture changes size, and frees them only a few frames
/// later: attaching each bitmap in turn ran the Vita out of memory when a
/// game drew into dozens of large bitmaps at once.
const SCRATCH_SIZE: u32 = 1024;
/// Bitmaps drawn into more often than this (like a per-frame canvas) get a
/// framebuffer of their own, which skips the copies, if they fit in
/// `SCRATCH_SIZE`.
const DEDICATED_AFTER_DRAWS: u32 = 3;
const MAX_DEDICATED_FRAMEBUFFERS: u32 = 8;
static DEDICATED_FRAMEBUFFERS: AtomicU32 = AtomicU32::new(0);

/// Describes free memory for the log; set by the app (vitaGL pools on Vita).
pub static MEMORY_PROBE: std::sync::OnceLock<fn() -> Option<String>> = std::sync::OnceLock::new();

/// Logs work on large textures together with free memory, so that running
/// out of memory while a game builds huge bitmaps leaves a trail in the log.
fn log_large_texture(what: &str, width: u32, height: u32) {
    const LARGE: u64 = 4 * 1024 * 1024;
    // Some games draw into a large bitmap every frame; don't log forever.
    static LOGGED: AtomicU32 = AtomicU32::new(0);
    if u64::from(width) * u64::from(height) * 4 >= LARGE
        && LOGGED.fetch_add(1, Ordering::Relaxed) < 300
    {
        let memory = MEMORY_PROBE.get().and_then(|probe| probe()).unwrap_or_default();
        log::info!(target: "rufflevita", "{what} {width}x{height} texture \u{b7} {memory}");
    }
}

/// RuffleVita: solid-colour shape draws with at most this many vertices stay
/// on the CPU and are drawn in batches (see `ColorBatch`); bigger ones keep
/// their own GPU buffers, where one draw call costs less than transforming
/// every vertex on the CPU each frame.
const BATCHABLE_MAX_VERTICES: usize = 1024;
/// A batch uses 16-bit indices.
const BATCH_MAX_VERTICES: usize = u16::MAX as usize;

/// RuffleVita: what the draw calls were, for tuning batching (timedemo).
pub static RENDER_STATS: [AtomicU32; 11] = [const { AtomicU32::new(0) }; 11];
pub const RENDER_STAT_NAMES: [&str; 11] = [
    "colour batches",
    "batched colour",
    "big colour",
    "gradient",
    "big bitmap fill",
    "bitmap batches",
    "line",
    "flush:mask",
    "batched bitmap",
    "gradient batches",
    "batched gradient",
];
fn stat(i: usize) {
    RENDER_STATS[i].fetch_add(1, Ordering::Relaxed);
}

/// Draw calls issued since the previous call, for the performance overlay.
pub fn take_draw_calls() -> u32 {
    DRAW_CALLS.swap(0, Ordering::Relaxed)
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct Vertex {
    position: [f32; 2],
    color: u32,
}

impl From<TessVertex> for Vertex {
    fn from(vertex: TessVertex) -> Self {
        Self {
            position: [vertex.x, vertex.y],
            color: u32::from_le_bytes([
                vertex.color.r,
                vertex.color.g,
                vertex.color.b,
                vertex.color.a,
            ]),
        }
    }
}

#[derive(Debug)]
pub struct QueueSyncHandle {
    texture: BitmapHandle,
    bounds: PixelRegion,
}

impl SyncHandle for QueueSyncHandle {}

pub struct GlowRenderBackend {
    gl: Arc<glow::Context>,

    // The frame buffers used for resolving MSAA (desktop only).
    msaa_buffers: Option<MsaaBuffers>,
    msaa_sample_count: u32,

    max_texture_size: u32,

    /// The framebuffer `BitmapData.draw` renders through, with
    /// `scratch_texture` (`SCRATCH_SIZE` square) attached once it's needed.
    offscreen_framebuffer: glow::Framebuffer,
    scratch_texture: Option<glow::Texture>,

    color_program: ShaderProgram,
    bitmap_program: ShaderProgram,
    gradient_program: ShaderProgram,
    batch_program: ShaderProgram,
    bitmap_batch_program: ShaderProgram,
    gradient_batch_program: ShaderProgram,

    /// Solid-colour shapes waiting to be drawn in one call.
    batch: ColorBatch,
    /// Bitmap fills sharing a texture and colour transform, waiting likewise.
    /// At most one of the two batches holds anything, to keep drawing order.
    bitmap_batch: BitmapBatch,
    gradient_batch: GradientBatch,
    ramp_atlas: RampAtlas,
    /// Batch small shapes (always on, except for A/B testing with
    /// `RUFFLEVITA_NO_BATCH`).
    batching: bool,

    shape_tessellator: ShapeTessellator,

    color_quad_draws: Vec<Draw>,
    bitmap_quad_draws: Vec<Draw>,

    mask_state: MaskState,
    num_masks: u32,
    mask_state_dirty: bool,
    is_transparent: bool,

    active_program: *const ShaderProgram,
    blend_modes: Vec<RenderBlendMode>,
    mult_color: Option<[f32; 4]>,
    add_color: Option<[f32; 4]>,

    renderbuffer_width: i32,
    renderbuffer_height: i32,
    view_matrix: [[f32; 4]; 4],

    viewport_scale_factor: f64,
}

#[derive(Debug)]
struct RegistryData {
    gl: Arc<glow::Context>,
    width: u32,
    height: u32,
    texture: glow::Texture,
    /// Currently applied (filter, wrap) sampler parameters, to skip redundant
    /// `glTexParameter` calls.
    params: Cell<(u32, u32)>,
    /// `BitmapData.draw` calls into this texture so far.
    offscreen_draws: Cell<u32>,
    /// A framebuffer of its own, once this texture is drawn into often.
    framebuffer: Cell<Option<glow::Framebuffer>>,
}

impl RegistryData {
    /// Binds the texture to unit 0 and applies the sampler state if it changed.
    fn bind(&self, gl: &glow::Context, filter: u32, wrap: u32) {
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(self.texture));
            let (cur_filter, cur_wrap) = self.params.get();
            if cur_filter != filter {
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, filter as i32);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, filter as i32);
            }
            if cur_wrap != wrap {
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, wrap as i32);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, wrap as i32);
            }
            self.params.set((filter, wrap));
        }
    }
}

impl Drop for RegistryData {
    fn drop(&mut self) {
        unsafe {
            if let Some(framebuffer) = self.framebuffer.take() {
                self.gl.delete_framebuffer(framebuffer);
                DEDICATED_FRAMEBUFFERS.fetch_sub(1, Ordering::Relaxed);
            }
            self.gl.delete_texture(self.texture);
        }
    }
}

impl BitmapHandleImpl for RegistryData {}

fn as_registry_data(handle: &BitmapHandle) -> &RegistryData {
    <dyn Any>::downcast_ref(&*handle.0).expect("Bitmap handle must be glow RegistryData")
}


impl GlowRenderBackend {
    pub fn new(
        glow_context: Arc<glow::Context>,
        is_transparent: bool,
        quality: StageQuality,
    ) -> Result<Self, Error> {
        log::info!("Creating glow renderer");
        unsafe {
            let gl = glow_context;

            // vitaGL renders straight to the display; MSAA is only used on desktop.
            let msaa_sample_count = if cfg!(target_os = "vita") {
                1
            } else {
                let max_samples = gl.get_parameter_i32(glow::MAX_SAMPLES).max(1) as u32;
                quality.sample_count().clamp(1, 4).min(max_samples)
            };

            let max_texture_size = gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE) as u32;

            let color_vertex = compile_shader(&gl, glow::VERTEX_SHADER, COLOR_VERTEX_GLSL)?;
            let texture_vertex = compile_shader(&gl, glow::VERTEX_SHADER, TEXTURE_VERTEX_GLSL)?;
            let color_fragment = compile_shader(&gl, glow::FRAGMENT_SHADER, COLOR_FRAGMENT_GLSL)?;
            let bitmap_fragment =
                compile_shader(&gl, glow::FRAGMENT_SHADER, BITMAP_FRAGMENT_GLSL)?;
            let gradient_fragment =
                compile_shader(&gl, glow::FRAGMENT_SHADER, GRADIENT_FRAGMENT_GLSL)?;
            let batch_vertex = compile_shader(&gl, glow::VERTEX_SHADER, BATCH_VERTEX_GLSL)?;
            let batch_bitmap_vertex =
                compile_shader(&gl, glow::VERTEX_SHADER, BATCH_BITMAP_VERTEX_GLSL)?;
            let batch_gradient_vertex =
                compile_shader(&gl, glow::VERTEX_SHADER, BATCH_GRADIENT_VERTEX_GLSL)?;
            let batch_gradient_fragment =
                compile_shader(&gl, glow::FRAGMENT_SHADER, BATCH_GRADIENT_FRAGMENT_GLSL)?;

            let color_program = ShaderProgram::new(&gl, color_vertex, color_fragment)?;
            let bitmap_program = ShaderProgram::new(&gl, texture_vertex, bitmap_fragment)?;
            let gradient_program = ShaderProgram::new(&gl, texture_vertex, gradient_fragment)?;
            let batch_program = ShaderProgram::new(&gl, batch_vertex, color_fragment)?;
            let bitmap_batch_program =
                ShaderProgram::new(&gl, batch_bitmap_vertex, bitmap_fragment)?;
            let gradient_batch_program =
                ShaderProgram::new(&gl, batch_gradient_vertex, batch_gradient_fragment)?;
            for shader in [
                batch_vertex,
                batch_bitmap_vertex,
                batch_gradient_vertex,
                batch_gradient_fragment,
                color_vertex,
                texture_vertex,
                color_fragment,
                bitmap_fragment,
                gradient_fragment,
            ] {
                gl.delete_shader(shader);
            }

            let offscreen_framebuffer = gl.create_framebuffer().map_err(Error::GlCreate)?;
            let batch = ColorBatch::new(&gl, &batch_program)?;
            let bitmap_batch = BitmapBatch::new(&gl, &bitmap_batch_program)?;
            let gradient_batch = GradientBatch::new(&gl, &gradient_batch_program)?;
            let ramp_atlas = RampAtlas::new(&gl)?;

            let mut renderer = Self {
                gl,
                msaa_buffers: None,
                msaa_sample_count,
                max_texture_size,
                offscreen_framebuffer,
                scratch_texture: None,
                color_program,
                gradient_program,
                bitmap_program,
                batch_program,
                bitmap_batch_program,
                gradient_batch_program,
                batch,
                bitmap_batch,
                gradient_batch,
                ramp_atlas,
                batching: std::env::var_os("RUFFLEVITA_NO_BATCH").is_none(),
                shape_tessellator: ShapeTessellator::new(),
                color_quad_draws: vec![],
                bitmap_quad_draws: vec![],
                renderbuffer_width: 1,
                renderbuffer_height: 1,
                view_matrix: [[0.0; 4]; 4],
                mask_state: MaskState::NoMask,
                num_masks: 0,
                mask_state_dirty: true,
                is_transparent,
                active_program: std::ptr::null(),
                blend_modes: vec![],
                mult_color: None,
                add_color: None,
                viewport_scale_factor: 1.0,
            };

            renderer.push_blend_mode(RenderBlendMode::Builtin(BlendMode::Normal));

            let color_quad = renderer.build_quad_mesh(&renderer.color_program, false)?;
            let bitmap_quad = renderer.build_quad_mesh(&renderer.bitmap_program, true)?;
            renderer.color_quad_draws.push(color_quad);
            renderer.bitmap_quad_draws.push(bitmap_quad);

            renderer.set_viewport_dimensions(ViewportDimensions {
                width: 1,
                height: 1,
                scale_factor: 1.0,
            });

            Ok(renderer)
        }
    }

    fn build_quad_mesh(&self, program: &ShaderProgram, is_bitmap: bool) -> Result<Draw, Error> {
        unsafe {
            let vao = self.gl.create_vertex_array().map_err(Error::GlCreate)?;
            self.gl.bind_vertex_array(Some(vao));

            let vertex_buffer = self.gl.create_buffer().map_err(Error::GlCreate)?;
            self.gl.bind_buffer(glow::ARRAY_BUFFER, Some(vertex_buffer));
            let white = 0xffff_ffff;
            self.gl.buffer_data_u8_slice(
                glow::ARRAY_BUFFER,
                bytemuck::cast_slice(&[
                    Vertex { position: [0.0, 0.0], color: white },
                    Vertex { position: [1.0, 0.0], color: white },
                    Vertex { position: [1.0, 1.0], color: white },
                    Vertex { position: [0.0, 1.0], color: white },
                ]),
                glow::STATIC_DRAW,
            );

            let index_buffer = self.gl.create_buffer().map_err(Error::GlCreate)?;
            self.gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(index_buffer));
            self.gl.buffer_data_u8_slice(
                glow::ELEMENT_ARRAY_BUFFER,
                bytemuck::cast_slice(&[0u32, 1, 2, 3]),
                glow::STATIC_DRAW,
            );

            program.bind_vertex_layout(&self.gl);
            self.gl.bind_vertex_array(None);

            Ok(Draw {
                draw_type: if is_bitmap {
                    DrawType::Bitmap(BitmapDraw {
                        matrix: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                        handle: None,
                        is_smoothed: true,
                        is_repeating: false,
                    })
                } else {
                    DrawType::Color
                },
                vao,
                vertex_buffer: Buffer { gl: self.gl.clone(), buffer: vertex_buffer },
                index_buffer: Buffer { gl: self.gl.clone(), buffer: index_buffer },
                num_indices: 4,
                num_mask_indices: 4,
            })
        }
    }

    fn build_msaa_buffers(&mut self) -> Result<(), Error> {
        unsafe {
            let gl = self.gl.as_ref();

            if let Some(msaa_buffers) = self.msaa_buffers.take() {
                gl.delete_renderbuffer(msaa_buffers.color_renderbuffer);
                gl.delete_renderbuffer(msaa_buffers.stencil_renderbuffer);
                gl.delete_framebuffer(msaa_buffers.render_framebuffer);
                gl.delete_framebuffer(msaa_buffers.color_framebuffer);
                gl.delete_texture(msaa_buffers.framebuffer_texture);
            }

            if self.msaa_sample_count <= 1 {
                gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                gl.bind_renderbuffer(glow::RENDERBUFFER, None);
                return Ok(());
            }

            let render_framebuffer = gl.create_framebuffer().map_err(Error::GlCreate)?;
            let color_framebuffer = gl.create_framebuffer().map_err(Error::GlCreate)?;

            let color_renderbuffer = gl.create_renderbuffer().map_err(Error::GlCreate)?;
            gl.bind_renderbuffer(glow::RENDERBUFFER, Some(color_renderbuffer));
            gl.renderbuffer_storage_multisample(
                glow::RENDERBUFFER,
                self.msaa_sample_count as i32,
                glow::RGBA8,
                self.renderbuffer_width,
                self.renderbuffer_height,
            );

            let stencil_renderbuffer = gl.create_renderbuffer().map_err(Error::GlCreate)?;
            gl.bind_renderbuffer(glow::RENDERBUFFER, Some(stencil_renderbuffer));
            gl.renderbuffer_storage_multisample(
                glow::RENDERBUFFER,
                self.msaa_sample_count as i32,
                glow::STENCIL_INDEX8,
                self.renderbuffer_width,
                self.renderbuffer_height,
            );

            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(render_framebuffer));
            gl.framebuffer_renderbuffer(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::RENDERBUFFER,
                Some(color_renderbuffer),
            );
            gl.framebuffer_renderbuffer(
                glow::FRAMEBUFFER,
                glow::STENCIL_ATTACHMENT,
                glow::RENDERBUFFER,
                Some(stencil_renderbuffer),
            );

            let framebuffer_texture = gl.create_texture().map_err(Error::GlCreate)?;
            gl.bind_texture(glow::TEXTURE_2D, Some(framebuffer_texture));
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA as i32,
                self.renderbuffer_width,
                self.renderbuffer_height,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
            );
            gl.bind_texture(glow::TEXTURE_2D, None);

            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(color_framebuffer));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(framebuffer_texture),
                0,
            );
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);

            self.msaa_buffers = Some(MsaaBuffers {
                color_renderbuffer,
                stencil_renderbuffer,
                render_framebuffer,
                color_framebuffer,
                framebuffer_texture,
            });

            Ok(())
        }
    }

    fn register_shape_internal(
        &mut self,
        shape: DistilledShape,
        bitmap_source: &dyn BitmapSource,
    ) -> Result<Vec<MeshDraw>, Error> {
        use ruffle_render::tessellator::DrawType as TessDrawType;

        let lyon_mesh = self.shape_tessellator.tessellate_shape(shape, bitmap_source);

        let mut draws = Vec::with_capacity(lyon_mesh.draws.len());
        for draw in lyon_mesh.draws {
            let num_indices = draw.indices.len() as i32;
            let num_mask_indices = draw.mask_index_count as i32;

            if self.batching
                && matches!(draw.draw_type, TessDrawType::Color)
                && draw.vertices.len() <= BATCHABLE_MAX_VERTICES
            {
                draws.push(MeshDraw::Cpu(CpuDraw::new(
                    draw.vertices.into_iter().map(Vertex::from).collect(),
                    &draw.indices,
                    draw.mask_index_count as usize,
                )));
                continue;
            }
            if let TessDrawType::Gradient { matrix, gradient } = &draw.draw_type {
                if self.batching && draw.vertices.len() <= BATCHABLE_MAX_VERTICES {
                    let tess_gradient = &lyon_mesh.gradients[*gradient];
                    if let Some(row) = self.ramp_atlas.allocate(&ramp_pixels(tess_gradient)) {
                        let (gradient_type, repeat_mode, focal_point) = gradient_params(tess_gradient);
                        let v = (f32::from(row.row) + 0.5) / RAMP_ATLAS_ROWS as f32;
                        draws.push(MeshDraw::CpuGradient(CpuGradientDraw {
                            positions: draw.vertices.iter().map(|v| [v.x, v.y]).collect(),
                            uvs: draw.vertices.iter().map(|v| apply_uv_matrix(matrix, v.x, v.y)).collect(),
                            indices: draw.indices.iter().map(|&i| i as u16).collect(),
                            num_mask_indices: draw.mask_index_count as usize,
                            params: [v, gradient_type, repeat_mode, focal_point],
                            _row: row,
                        }));
                        continue;
                    }
                }
            }
            if let TessDrawType::Bitmap(bitmap) = &draw.draw_type {
                if self.batching && draw.vertices.len() <= BATCHABLE_MAX_VERTICES {
                    // The shader's `u_matrix * vec3(position, 1.0)`, done once here.
                    let uv = |x: f32, y: f32| apply_uv_matrix(&bitmap.matrix, x, y);
                    draws.push(MeshDraw::CpuBitmap(CpuBitmapDraw {
                        positions: draw.vertices.iter().map(|v| [v.x, v.y]).collect(),
                        uvs: draw.vertices.iter().map(|v| uv(v.x, v.y)).collect(),
                        indices: draw.indices.iter().map(|&i| i as u16).collect(),
                        num_mask_indices: draw.mask_index_count as usize,
                        handle: bitmap_source.bitmap_handle(bitmap.bitmap_id, self),
                        is_smoothed: bitmap.is_smoothed,
                        is_repeating: bitmap.is_repeating,
                    }));
                    continue;
                }
            }

            unsafe {
                let vao = self.gl.create_vertex_array().map_err(Error::GlCreate)?;
                self.gl.bind_vertex_array(Some(vao));

                let vertex_buffer = self.gl.create_buffer().map_err(Error::GlCreate)?;
                self.gl.bind_buffer(glow::ARRAY_BUFFER, Some(vertex_buffer));
                let vertices: Vec<Vertex> = draw.vertices.into_iter().map(Vertex::from).collect();
                self.gl.buffer_data_u8_slice(
                    glow::ARRAY_BUFFER,
                    bytemuck::cast_slice(&vertices),
                    glow::STATIC_DRAW,
                );

                let index_buffer = self.gl.create_buffer().map_err(Error::GlCreate)?;
                self.gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(index_buffer));
                self.gl.buffer_data_u8_slice(
                    glow::ELEMENT_ARRAY_BUFFER,
                    bytemuck::cast_slice(&draw.indices),
                    glow::STATIC_DRAW,
                );

                let program = match draw.draw_type {
                    TessDrawType::Color => &self.color_program,
                    TessDrawType::Gradient { .. } => &self.gradient_program,
                    TessDrawType::Bitmap(_) => &self.bitmap_program,
                };
                // Attribute locations are not guaranteed to match between
                // programs in GLSL ES 1.00, so bind per program.
                program.bind_vertex_layout(&self.gl);
                self.gl.bind_vertex_array(None);

                let draw_type = match draw.draw_type {
                    TessDrawType::Color => DrawType::Color,
                    TessDrawType::Gradient { matrix, gradient } => DrawType::Gradient(Box::new(
                        Gradient::new(self.gl.clone(), lyon_mesh.gradients[gradient].clone(), matrix)?,
                    )),
                    TessDrawType::Bitmap(bitmap) => DrawType::Bitmap(BitmapDraw {
                        matrix: bitmap.matrix,
                        handle: bitmap_source.bitmap_handle(bitmap.bitmap_id, self),
                        is_smoothed: bitmap.is_smoothed,
                        is_repeating: bitmap.is_repeating,
                    }),
                };

                draws.push(MeshDraw::Gpu(Draw {
                    draw_type,
                    vao,
                    vertex_buffer: Buffer { gl: self.gl.clone(), buffer: vertex_buffer },
                    index_buffer: Buffer { gl: self.gl.clone(), buffer: index_buffer },
                    num_indices,
                    num_mask_indices,
                }));
            }
        }

        Ok(draws)
    }

    /// Draws the pending batch, if any, with the state that was current
    /// when its shapes were added. Every state change calls this first.
    fn flush_batch(&mut self) {
        self.flush_color_batch();
        self.flush_bitmap_batch();
        self.flush_gradient_batch();
    }

    fn flush_gradient_batch(&mut self) {
        if self.gradient_batch.indices.is_empty() {
            return;
        }
        let _zone = rv_prof::zone(rv_prof::Zone::GlFlush);
        let (mult, add) = self.gradient_batch.state.take().expect("a non-empty gradient batch has a colour transform");
        self.use_program(ProgramKind::GradientBatch);
        let program = &self.gradient_batch_program;
        if Some(mult) != self.mult_color {
            program.uniform4fv(&self.gl, ShaderUniform::MultColor, &mult);
            self.mult_color = Some(mult);
        }
        if Some(add) != self.add_color {
            program.uniform4fv(&self.gl, ShaderUniform::AddColor, &add);
            self.add_color = Some(add);
        }
        let batch = &mut self.gradient_batch;
        unsafe {
            let gl = &self.gl;
            self.ramp_atlas.bind(gl);
            gl.bind_vertex_array(Some(batch.vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(batch.vertex_buffer));
            gl.buffer_data_u8_slice(
                glow::ARRAY_BUFFER,
                bytemuck::cast_slice(&batch.vertices),
                glow::STREAM_DRAW,
            );
            self.gradient_batch_program.bind_gradient_layout(gl);
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(batch.index_buffer));
            gl.buffer_data_u8_slice(
                glow::ELEMENT_ARRAY_BUFFER,
                bytemuck::cast_slice(&batch.indices),
                glow::STREAM_DRAW,
            );
            DRAW_CALLS.fetch_add(1, Ordering::Relaxed);
            stat(9);
            gl.draw_elements(glow::TRIANGLES, batch.indices.len() as i32, glow::UNSIGNED_SHORT, 0);
        }
        batch.vertices.clear();
        batch.indices.clear();
    }

    /// Gets the gradient batch ready for `vertices` more vertices drawn with
    /// this colour transform.
    fn begin_gradient_batch(&mut self, mult: [f32; 4], add: [f32; 4], vertices: usize) {
        self.flush_color_batch();
        self.flush_bitmap_batch();
        let same = self.gradient_batch.state == Some((mult, add));
        if !same || self.gradient_batch.vertices.len() + vertices > BATCH_MAX_VERTICES {
            self.flush_gradient_batch();
        }
        self.gradient_batch.state = Some((mult, add));
    }

    fn flush_color_batch(&mut self) {
        if self.batch.indices.is_empty() {
            return;
        }
        let _zone = rv_prof::zone(rv_prof::Zone::GlFlush);
        self.use_program(ProgramKind::Batch);
        let batch = &mut self.batch;
        unsafe {
            let gl = &self.gl;
            gl.bind_vertex_array(Some(batch.vao));
            // Respecify ("orphan") the buffers each time, so the driver hands
            // out fresh memory instead of waiting for the GPU to finish with
            // the previous batch.
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(batch.vertex_buffer));
            gl.buffer_data_u8_slice(
                glow::ARRAY_BUFFER,
                bytemuck::cast_slice(&batch.vertices),
                glow::STREAM_DRAW,
            );
            // vitaGL may resolve attribute addresses when they're set, so set
            // them again for the new storage.
            self.batch_program.bind_vertex_layout(gl);
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(batch.index_buffer));
            gl.buffer_data_u8_slice(
                glow::ELEMENT_ARRAY_BUFFER,
                bytemuck::cast_slice(&batch.indices),
                glow::STREAM_DRAW,
            );
            DRAW_CALLS.fetch_add(1, Ordering::Relaxed);
            stat(0);
            gl.draw_elements(glow::TRIANGLES, batch.indices.len() as i32, glow::UNSIGNED_SHORT, 0);
        }
        batch.vertices.clear();
        batch.indices.clear();
    }

    fn flush_bitmap_batch(&mut self) {
        if self.bitmap_batch.indices.is_empty() {
            return;
        }
        let _zone = rv_prof::zone(rv_prof::Zone::GlFlush);
        let state = self.bitmap_batch.state.take().expect("a non-empty bitmap batch has a texture");
        self.use_program(ProgramKind::BitmapBatch);
        let program = &self.bitmap_batch_program;
        if Some(state.mult) != self.mult_color {
            program.uniform4fv(&self.gl, ShaderUniform::MultColor, &state.mult);
            self.mult_color = Some(state.mult);
        }
        if Some(state.add) != self.add_color {
            program.uniform4fv(&self.gl, ShaderUniform::AddColor, &state.add);
            self.add_color = Some(state.add);
        }
        program.uniform1f(&self.gl, ShaderUniform::Repeat, if state.repeat { 1.0 } else { 0.0 });
        as_registry_data(&state.handle).bind(&self.gl, state.filter, glow::CLAMP_TO_EDGE);
        let batch = &mut self.bitmap_batch;
        unsafe {
            let gl = &self.gl;
            gl.bind_vertex_array(Some(batch.vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(batch.vertex_buffer));
            gl.buffer_data_u8_slice(
                glow::ARRAY_BUFFER,
                bytemuck::cast_slice(&batch.vertices),
                glow::STREAM_DRAW,
            );
            self.bitmap_batch_program.bind_uv_layout(gl);
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(batch.index_buffer));
            gl.buffer_data_u8_slice(
                glow::ELEMENT_ARRAY_BUFFER,
                bytemuck::cast_slice(&batch.indices),
                glow::STREAM_DRAW,
            );
            DRAW_CALLS.fetch_add(1, Ordering::Relaxed);
            stat(5);
            gl.draw_elements(glow::TRIANGLES, batch.indices.len() as i32, glow::UNSIGNED_SHORT, 0);
        }
        batch.vertices.clear();
        batch.indices.clear();
    }

    /// Gets the colour batch ready for `vertices` more vertices.
    fn begin_color_batch(&mut self, vertices: usize) {
        self.flush_bitmap_batch();
        self.flush_gradient_batch();
        if self.batch.vertices.len() + vertices > BATCH_MAX_VERTICES {
            self.flush_color_batch();
        }
    }

    /// Gets the bitmap batch ready for `vertices` more vertices drawn with
    /// this texture, sampling and colour transform.
    fn begin_bitmap_batch(
        &mut self,
        handle: &BitmapHandle,
        filter: u32,
        repeat: bool,
        mult: [f32; 4],
        add: [f32; 4],
        vertices: usize,
    ) {
        self.flush_color_batch();
        self.flush_gradient_batch();
        let same = self.bitmap_batch.state.as_ref().is_some_and(|s| {
            std::ptr::addr_eq(Arc::as_ptr(&s.handle.0), Arc::as_ptr(&handle.0))
                && s.filter == filter
                && s.repeat == repeat
                && s.mult == mult
                && s.add == add
        });
        if !same || self.bitmap_batch.vertices.len() + vertices > BATCH_MAX_VERTICES {
            self.flush_bitmap_batch();
        }
        if self.bitmap_batch.state.is_none() {
            self.bitmap_batch.state =
                Some(BitmapBatchState { handle: handle.clone(), filter, repeat, mult, add });
        }
    }

    /// Downscales bitmaps that exceed the GPU's maximum texture size.
    fn clamp_bitmap(&self, bitmap: &mut Bitmap, format: u32) -> bool {
        let max_size = self.max_texture_size;
        if bitmap.width() <= max_size && bitmap.height() <= max_size {
            return false;
        }
        let ratio = bitmap.width() as f32 / bitmap.height() as f32;
        let mut width = bitmap.width();
        let mut height = bitmap.height();
        if width > max_size {
            width = max_size;
            height = (max_size as f32 / ratio) as u32;
        }
        if height > max_size {
            height = max_size;
            width = (max_size as f32 * ratio) as u32;
        }
        let filter = image::imageops::FilterType::Triangle;
        if format == glow::RGBA {
            let image =
                image::RgbaImage::from_raw(bitmap.width(), bitmap.height(), bitmap.data().to_vec())
                    .expect("Width and height of bitmap must match bitmap data");
            let resized = image::imageops::resize(&image, width, height, filter);
            *bitmap = Bitmap::new(width, height, BitmapFormat::Rgba, resized.into_raw());
        } else {
            let image =
                image::RgbImage::from_raw(bitmap.width(), bitmap.height(), bitmap.data().to_vec())
                    .expect("Width and height of bitmap must match bitmap data");
            let resized = image::imageops::resize(&image, width, height, filter);
            *bitmap = Bitmap::new(width, height, BitmapFormat::Rgb, resized.into_raw());
        }
        true
    }

    fn set_stencil_state(&mut self) {
        if !self.mask_state_dirty {
            return;
        }
        if !self.batch.indices.is_empty() {
            stat(7);
        }
        self.flush_batch();
        self.mask_state_dirty = false;
        unsafe {
            match self.mask_state {
                MaskState::NoMask => {
                    self.gl.disable(glow::STENCIL_TEST);
                    self.gl.color_mask(true, true, true, true);
                }
                MaskState::DrawMaskStencil => {
                    self.gl.enable(glow::STENCIL_TEST);
                    self.gl.stencil_func(glow::EQUAL, (self.num_masks - 1) as i32, 0xff);
                    self.gl.stencil_op(glow::KEEP, glow::KEEP, glow::INCR);
                    self.gl.color_mask(false, false, false, false);
                }
                MaskState::DrawMaskedContent => {
                    self.gl.enable(glow::STENCIL_TEST);
                    self.gl.stencil_func(glow::EQUAL, self.num_masks as i32, 0xff);
                    self.gl.stencil_op(glow::KEEP, glow::KEEP, glow::KEEP);
                    self.gl.color_mask(true, true, true, true);
                }
                MaskState::ClearMaskStencil => {
                    self.gl.enable(glow::STENCIL_TEST);
                    self.gl.stencil_func(glow::EQUAL, self.num_masks as i32, 0xff);
                    self.gl.stencil_op(glow::KEEP, glow::KEEP, glow::DECR);
                    self.gl.color_mask(false, false, false, false);
                }
            }
        }
    }

    fn apply_blend_mode(&self, mode: &RenderBlendMode) {
        // All colours are premultiplied. Modes that need the backdrop in a
        // shader (overlay, hardlight, difference, ...) fall back to normal.
        let (op, src_rgb, dst_rgb, src_a, dst_a) = match mode {
            RenderBlendMode::Builtin(BlendMode::Add) => {
                (glow::FUNC_ADD, glow::ONE, glow::ONE, glow::ONE, glow::ONE)
            }
            RenderBlendMode::Builtin(BlendMode::Subtract) => (
                glow::FUNC_REVERSE_SUBTRACT,
                glow::ONE,
                glow::ONE,
                glow::ONE,
                glow::ONE,
            ),
            RenderBlendMode::Builtin(BlendMode::Multiply) => (
                glow::FUNC_ADD,
                glow::DST_COLOR,
                glow::ONE_MINUS_SRC_ALPHA,
                glow::ONE,
                glow::ONE_MINUS_SRC_ALPHA,
            ),
            RenderBlendMode::Builtin(BlendMode::Screen) => (
                glow::FUNC_ADD,
                glow::ONE,
                glow::ONE_MINUS_SRC_COLOR,
                glow::ONE,
                glow::ONE_MINUS_SRC_ALPHA,
            ),
            RenderBlendMode::Builtin(BlendMode::Lighten) => {
                (glow::MAX, glow::ONE, glow::ONE, glow::ONE, glow::ONE)
            }
            RenderBlendMode::Builtin(BlendMode::Darken) => {
                (glow::MIN, glow::ONE, glow::ONE, glow::ONE, glow::ONE)
            }
            // Erase/Alpha only affect the parent *layer*, which we don't
            // have; applied to the screen they would paint black. Skip the
            // drawing instead (the effect is missing but nothing breaks).
            RenderBlendMode::Builtin(BlendMode::Erase | BlendMode::Alpha) => {
                (glow::FUNC_ADD, glow::ZERO, glow::ONE, glow::ZERO, glow::ONE)
            }
            _ => (
                glow::FUNC_ADD,
                glow::ONE,
                glow::ONE_MINUS_SRC_ALPHA,
                glow::ONE,
                glow::ONE_MINUS_SRC_ALPHA,
            ),
        };
        let alpha_op = if op == glow::MAX || op == glow::MIN {
            op
        } else {
            glow::FUNC_ADD
        };
        unsafe {
            self.gl.blend_equation_separate(op, alpha_op);
            self.gl.blend_func_separate(src_rgb, dst_rgb, src_a, dst_a);
        }
    }

    fn current_blend_mode(&self) -> RenderBlendMode {
        self.blend_modes
            .last()
            .cloned()
            .unwrap_or(RenderBlendMode::Builtin(BlendMode::Normal))
    }

    /// Resets every piece of GL state this renderer depends on and forgets
    /// cached state, so anything drawn in between frames can't leak in.
    fn reset_gl_state(&mut self) {
        self.flush_batch();
        self.active_program = std::ptr::null();
        self.mask_state = MaskState::NoMask;
        self.num_masks = 0;
        self.mask_state_dirty = true;
        self.mult_color = None;
        self.add_color = None;
        unsafe {
            let gl = &self.gl;
            gl.enable(glow::BLEND);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::CULL_FACE);
            gl.active_texture(glow::TEXTURE0);
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
        }
        self.apply_blend_mode(&self.current_blend_mode());
    }

    /// Gives `entry` a framebuffer of its own (see `DEDICATED_AFTER_DRAWS`).
    fn create_dedicated_framebuffer(&self, entry: &RegistryData) -> Option<glow::Framebuffer> {
        unsafe {
            let framebuffer = self.gl.create_framebuffer().ok()?;
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            self.gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(entry.texture),
                0,
            );
            let complete =
                self.gl.check_framebuffer_status(glow::FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE;
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            if !complete {
                self.gl.delete_framebuffer(framebuffer);
                return None;
            }
            DEDICATED_FRAMEBUFFERS.fetch_add(1, Ordering::Relaxed);
            entry.framebuffer.set(Some(framebuffer));
            Some(framebuffer)
        }
    }

    /// Binds `offscreen_framebuffer` with the scratch texture attached,
    /// creating the texture on first use.
    fn bind_scratch_framebuffer(&mut self) -> bool {
        unsafe {
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.offscreen_framebuffer));
            if self.scratch_texture.is_some() {
                return true;
            }
            let Ok(texture) = self.gl.create_texture() else {
                return false;
            };
            self.gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            self.gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA as i32,
                SCRATCH_SIZE as i32,
                SCRATCH_SIZE as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
            );
            for (param, value) in [
                (glow::TEXTURE_MIN_FILTER, glow::NEAREST),
                (glow::TEXTURE_MAG_FILTER, glow::NEAREST),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
            ] {
                self.gl.tex_parameter_i32(glow::TEXTURE_2D, param, value as i32);
            }
            self.gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(texture),
                0,
            );
            if self.gl.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
                log::error!("Scratch framebuffer incomplete; skipping BitmapData.draw");
                self.gl.delete_texture(texture);
                self.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                return false;
            }
            self.scratch_texture = Some(texture);
            true
        }
    }

    /// Draws `commands` into `entry` tile by tile through the scratch
    /// framebuffer: each tile starts as a copy of the bitmap, gets the
    /// commands drawn over it, and is copied back.
    fn render_offscreen_tiled(
        &mut self,
        entry: &RegistryData,
        commands: CommandList,
        bounds: PixelRegion,
    ) -> bool {
        if !self.bind_scratch_framebuffer() {
            return false;
        }
        let mut region = bounds;
        region.clamp(entry.width, entry.height);
        for (x, y, w, h) in tiles(region) {
            unsafe {
                self.gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.offscreen_framebuffer));
                self.gl.viewport(0, 0, w as i32, h as i32);
            }
            self.blit_texture_tile(entry, x, y, w, h);
            self.render_tile(commands.clone(), x, y, w, h);
            self.copy_scratch_to_texture(entry, x, y, w, h);
        }
        true
    }

    /// Copies the rendered tile from the scratch texture into `entry`.
    fn copy_scratch_to_texture(&self, entry: &RegistryData, x: u32, y: u32, w: u32, h: u32) {
        // vitaGL's glCopyTexSubImage2D goes through glTexSubImage2D, which
        // reallocates the *whole* texture when the GPU used it recently (as
        // the tile copy just did): a full copy of a huge bitmap per tile.
        // Instead, wait for the GPU and copy the texels with a GPU transfer
        // (or the CPU if that fails). Both textures are linear RGBA with rows
        // padded to 8 pixels, and framebuffer row 0 is texture row 0.
        #[cfg(target_os = "vita")]
        unsafe {
            use std::ffi::c_void;
            unsafe extern "C" {
                fn vglGetTexDataPointer(target: u32) -> *mut u8;
                #[allow(clippy::too_many_arguments)]
                fn sceGxmTransferCopy(
                    width: u32,
                    height: u32,
                    color_key_value: u32,
                    color_key_mask: u32,
                    color_key_mode: u32,
                    src_format: u32,
                    src_type: u32,
                    src: *const c_void,
                    src_x: u32,
                    src_y: u32,
                    src_stride: i32,
                    dst_format: u32,
                    dst_type: u32,
                    dst: *mut c_void,
                    dst_x: u32,
                    dst_y: u32,
                    dst_stride: i32,
                    sync_object: *mut c_void,
                    sync_flags: u32,
                    notification: *const c_void,
                ) -> i32;
                fn sceGxmTransferFinish() -> i32;
            }
            const TRANSFER_FORMAT_RAW32: u32 = 0x0011_0000;
            const TRANSFER_LINEAR: u32 = 0;
            const COLORKEY_NONE: u32 = 0;
            let row_stride = |width: u32| (width.div_ceil(8) * 8 * 4) as usize;
            self.gl.finish();
            self.gl.bind_texture(glow::TEXTURE_2D, self.scratch_texture);
            let src = vglGetTexDataPointer(glow::TEXTURE_2D);
            entry.bind(&self.gl, glow::NEAREST, glow::CLAMP_TO_EDGE);
            let dst = vglGetTexDataPointer(glow::TEXTURE_2D);
            if !src.is_null() && !dst.is_null() {
                let (src_stride, dst_stride) = (row_stride(SCRATCH_SIZE), row_stride(entry.width));
                let result = sceGxmTransferCopy(
                    w,
                    h,
                    0,
                    0,
                    COLORKEY_NONE,
                    TRANSFER_FORMAT_RAW32,
                    TRANSFER_LINEAR,
                    src as *const c_void,
                    0,
                    0,
                    src_stride as i32,
                    TRANSFER_FORMAT_RAW32,
                    TRANSFER_LINEAR,
                    dst as *mut c_void,
                    x,
                    y,
                    dst_stride as i32,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null(),
                );
                if result >= 0 {
                    sceGxmTransferFinish();
                    return;
                }
                for row in 0..h as usize {
                    std::ptr::copy_nonoverlapping(
                        src.add(row * src_stride),
                        dst.add((y as usize + row) * dst_stride + x as usize * 4),
                        w as usize * 4,
                    );
                }
                return;
            }
        }
        unsafe {
            entry.bind(&self.gl, glow::NEAREST, glow::CLAMP_TO_EDGE);
            self.gl.copy_tex_sub_image_2d(
                glow::TEXTURE_2D,
                0,
                x as i32,
                y as i32,
                0,
                0,
                w as i32,
                h as i32,
            );
        }
    }

    /// Runs `commands` into the bound framebuffer, which shows the bitmap
    /// area `x..x + w`, `y..y + h` (in pixels, row 0 at the top).
    fn render_tile(&mut self, commands: CommandList, x: u32, y: u32, w: u32, h: u32) {
        let (w, h) = (w.max(1) as f32, h.max(1) as f32);
        unsafe { self.gl.viewport(0, 0, w as i32, h as i32) };
        // Note: un-flipped Y, so texture row 0 is the top of the bitmap.
        self.view_matrix = [
            [2.0 / w, 0.0, 0.0, 0.0],
            [0.0, 2.0 / h, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [-1.0 - 2.0 * x as f32 / w, -1.0 - 2.0 * y as f32 / h, 0.0, 1.0],
        ];
        self.reset_gl_state();
        self.set_stencil_state();
        commands.execute(self);
        self.flush_batch();
    }

    /// Copies the `w` x `h` area of `entry` at `x`, `y` to the bound
    /// framebuffer's viewport, replacing its contents.
    fn blit_texture_tile(&mut self, entry: &RegistryData, x: u32, y: u32, w: u32, h: u32) {
        self.flush_batch();
        let (tw, th) = (entry.width.max(1) as f32, entry.height.max(1) as f32);
        unsafe {
            let gl = &self.gl;
            gl.disable(glow::STENCIL_TEST);
            self.mask_state_dirty = true;
            let program = &self.bitmap_program;
            gl.use_program(Some(program.program));
            self.active_program = std::ptr::null();
            program.uniform_matrix4fv(
                gl,
                ShaderUniform::WorldMatrix,
                &[
                    [2.0, 0.0, 0.0, 0.0],
                    [0.0, 2.0, 0.0, 0.0],
                    [0.0, 0.0, 1.0, 0.0],
                    [-1.0, -1.0, 0.0, 1.0],
                ],
            );
            program.uniform_matrix4fv(
                gl,
                ShaderUniform::ViewMatrix,
                &[
                    [1.0, 0.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0, 0.0],
                    [0.0, 0.0, 1.0, 0.0],
                    [0.0, 0.0, 0.0, 1.0],
                ],
            );
            program.uniform4fv(gl, ShaderUniform::MultColor, &[1.0, 1.0, 1.0, 1.0]);
            program.uniform4fv(gl, ShaderUniform::AddColor, &[0.0, 0.0, 0.0, 0.0]);
            program.uniform1f(gl, ShaderUniform::Repeat, 0.0);
            program.uniform_matrix3fv(
                gl,
                ShaderUniform::TextureMatrix,
                &[
                    [w as f32 / tw, 0.0, 0.0],
                    [0.0, h as f32 / th, 0.0],
                    [x as f32 / tw, y as f32 / th, 1.0],
                ],
            );
            entry.bind(gl, glow::NEAREST, glow::CLAMP_TO_EDGE);
            gl.blend_func(glow::ONE, glow::ZERO);
            let quad = &self.bitmap_quad_draws[0];
            gl.bind_vertex_array(Some(quad.vao));
            DRAW_CALLS.fetch_add(1, Ordering::Relaxed);
            gl.draw_elements(glow::TRIANGLE_FAN, quad.num_indices, glow::UNSIGNED_INT, 0);
            gl.bind_vertex_array(None);
        }
        self.apply_blend_mode(&self.current_blend_mode());
    }

    fn begin_frame(&mut self, clear: Color) {
        self.reset_gl_state();
        unsafe {
            let fb = self.msaa_buffers.as_ref().map(|b| b.render_framebuffer);
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, fb);
            self.gl.viewport(0, 0, self.renderbuffer_width, self.renderbuffer_height);

            self.set_stencil_state();
            if self.is_transparent {
                self.gl.clear_color(0.0, 0.0, 0.0, 0.0);
            } else {
                self.gl.clear_color(
                    clear.r as f32 / 255.0,
                    clear.g as f32 / 255.0,
                    clear.b as f32 / 255.0,
                    clear.a as f32 / 255.0,
                );
            }
            self.gl.stencil_mask(0xff);
            self.gl.clear(glow::COLOR_BUFFER_BIT | glow::STENCIL_BUFFER_BIT);
        }
    }

    fn end_frame(&mut self) {
        self.flush_batch();
        unsafe {
            self.gl.disable(glow::STENCIL_TEST);
            self.gl.color_mask(true, true, true, true);
            self.mask_state_dirty = true;

            let Some(msaa_buffers) = &self.msaa_buffers else {
                return;
            };
            let gl = &self.gl;
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(msaa_buffers.render_framebuffer));
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(msaa_buffers.color_framebuffer));
            gl.blit_framebuffer(
                0,
                0,
                self.renderbuffer_width,
                self.renderbuffer_height,
                0,
                0,
                self.renderbuffer_width,
                self.renderbuffer_height,
                glow::COLOR_BUFFER_BIT,
                glow::NEAREST,
            );
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.viewport(0, 0, self.renderbuffer_width, self.renderbuffer_height);

            // Draw the resolved texture as a fullscreen quad.
            let program = &self.bitmap_program;
            gl.use_program(Some(program.program));
            self.active_program = std::ptr::null();
            program.uniform_matrix4fv(
                gl,
                ShaderUniform::WorldMatrix,
                &[
                    [2.0, 0.0, 0.0, 0.0],
                    [0.0, 2.0, 0.0, 0.0],
                    [0.0, 0.0, 1.0, 0.0],
                    [-1.0, -1.0, 0.0, 1.0],
                ],
            );
            program.uniform_matrix4fv(
                gl,
                ShaderUniform::ViewMatrix,
                &[
                    [1.0, 0.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0, 0.0],
                    [0.0, 0.0, 1.0, 0.0],
                    [0.0, 0.0, 0.0, 1.0],
                ],
            );
            program.uniform4fv(gl, ShaderUniform::MultColor, &[1.0, 1.0, 1.0, 1.0]);
            program.uniform4fv(gl, ShaderUniform::AddColor, &[0.0, 0.0, 0.0, 0.0]);
            program.uniform1f(gl, ShaderUniform::Repeat, 0.0);
            program.uniform_matrix3fv(
                gl,
                ShaderUniform::TextureMatrix,
                &[[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            );
            gl.bind_texture(glow::TEXTURE_2D, Some(msaa_buffers.framebuffer_texture));
            gl.blend_func(glow::ONE, glow::ZERO);
            let quad = &self.bitmap_quad_draws[0];
            gl.bind_vertex_array(Some(quad.vao));
            DRAW_CALLS.fetch_add(1, Ordering::Relaxed);
            gl.draw_elements(glow::TRIANGLE_FAN, quad.num_indices, glow::UNSIGNED_INT, 0);
            gl.bind_vertex_array(None);
            self.apply_blend_mode(&self.current_blend_mode());
        }
    }

    fn push_blend_mode(&mut self, blend: RenderBlendMode) {
        if !same_blend_mode(self.blend_modes.last(), &blend) {
            self.flush_batch();
            self.apply_blend_mode(&blend);
        }
        self.blend_modes.push(blend);
    }

    fn pop_blend_mode(&mut self) {
        let old = self.blend_modes.pop();
        let current = self.current_blend_mode();
        if !same_blend_mode(old.as_ref(), &current) {
            self.flush_batch();
            self.apply_blend_mode(&current);
        }
    }

    /// Switches programs if needed, invalidating the cached uniforms.
    fn use_program(&mut self, which: ProgramKind) {
        // Anything but the batch program is about to draw on its own: the
        // batched shapes come first.
        if !matches!(
            which,
            ProgramKind::Batch | ProgramKind::BitmapBatch | ProgramKind::GradientBatch
        ) {
            self.flush_batch();
        }
        let program = match which {
            ProgramKind::Color => &self.color_program,
            ProgramKind::Bitmap => &self.bitmap_program,
            ProgramKind::Gradient => &self.gradient_program,
            ProgramKind::Batch => &self.batch_program,
            ProgramKind::BitmapBatch => &self.bitmap_batch_program,
            ProgramKind::GradientBatch => &self.gradient_batch_program,
        };
        if std::ptr::eq(program, self.active_program) {
            return;
        }
        unsafe {
            self.gl.use_program(Some(program.program));
        }
        program.uniform_matrix4fv(&self.gl, ShaderUniform::ViewMatrix, &self.view_matrix);
        self.active_program = program as *const ShaderProgram;
        self.mult_color = None;
        self.add_color = None;
    }

    fn set_color_uniforms(&mut self, world_matrix: &[[f32; 4]; 4], mult: [f32; 4], add: [f32; 4]) {
        // SAFETY: `use_program` always runs first, pointing this at one of our
        // own programs, which live as long as `self`.
        let program = unsafe { &*self.active_program };
        program.uniform_matrix4fv(&self.gl, ShaderUniform::WorldMatrix, world_matrix);
        if Some(mult) != self.mult_color {
            program.uniform4fv(&self.gl, ShaderUniform::MultColor, &mult);
            self.mult_color = Some(mult);
        }
        if Some(add) != self.add_color {
            program.uniform4fv(&self.gl, ShaderUniform::AddColor, &add);
            self.add_color = Some(add);
        }
    }

    fn draw_quad<const MODE: u32, const COUNT: i32>(&mut self, color: Color, matrix: Matrix) {
        // Filled rectangles join the batch; lines need their own primitive.
        if MODE == glow::TRIANGLE_FAN && self.batching {
            self.set_stencil_state();
            let color = premultiply(u32::from_le_bytes([color.r, color.g, color.b, color.a]));
            if self.gradient_batch.open_for(4) {
                self.gradient_batch.append_rect(&matrix, color);
            } else {
                self.begin_color_batch(4);
                self.batch.append_rect(&matrix, color);
            }
            return;
        }
        let world_matrix = world_matrix(&matrix);
        let mult_color = [
            color.r as f32 / 255.0,
            color.g as f32 / 255.0,
            color.b as f32 / 255.0,
            color.a as f32 / 255.0,
        ];

        stat(6);
        self.set_stencil_state();
        self.use_program(ProgramKind::Color);
        self.set_color_uniforms(&world_matrix, mult_color, [0.0; 4]);

        let quad = &self.color_quad_draws[0];
        let count = if COUNT < 0 { quad.num_indices } else { COUNT };
        unsafe {
            self.gl.bind_vertex_array(Some(quad.vao));
            DRAW_CALLS.fetch_add(1, Ordering::Relaxed);
            self.gl.draw_elements(MODE, count, glow::UNSIGNED_INT, 0);
        }
    }
}

/// Splits `region` into tiles of at most `SCRATCH_SIZE` pixels square, as
/// (x, y, width, height).
fn tiles(region: PixelRegion) -> impl Iterator<Item = (u32, u32, u32, u32)> {
    let step = SCRATCH_SIZE as usize;
    (region.y_min..region.y_max).step_by(step).flat_map(move |y| {
        (region.x_min..region.x_max).step_by(step).map(move |x| {
            let w = (region.x_max - x).min(SCRATCH_SIZE);
            let h = (region.y_max - y).min(SCRATCH_SIZE);
            (x, y, w, h)
        })
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ProgramKind {
    Color,
    Bitmap,
    Gradient,
    Batch,
    BitmapBatch,
    GradientBatch,
}

fn world_matrix(matrix: &Matrix) -> [[f32; 4]; 4] {
    [
        [matrix.a, matrix.b, 0.0, 0.0],
        [matrix.c, matrix.d, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [matrix.tx.to_pixels() as f32, matrix.ty.to_pixels() as f32, 0.0, 1.0],
    ]
}

fn same_blend_mode(first: Option<&RenderBlendMode>, second: &RenderBlendMode) -> bool {
    match (first, second) {
        (Some(RenderBlendMode::Builtin(old)), RenderBlendMode::Builtin(new)) => old == new,
        _ => false,
    }
}

fn texture_format(format: BitmapFormat) -> u32 {
    match format {
        BitmapFormat::Rgb | BitmapFormat::Yuv420p => glow::RGB,
        BitmapFormat::Rgba | BitmapFormat::Yuva420p => glow::RGBA,
    }
}

impl RenderBackend for GlowRenderBackend {
    fn render_offscreen(
        &mut self,
        handle: BitmapHandle,
        commands: CommandList,
        _quality: StageQuality,
        bounds: PixelRegion,
    ) -> Option<Box<dyn SyncHandle>> {
        self.flush_batch();
        let entry = as_registry_data(&handle);
        let (width, height) = (entry.width, entry.height);
        log_large_texture("Drawing into", width, height);
        let draws = entry.offscreen_draws.get() + 1;
        entry.offscreen_draws.set(draws);

        self.reset_gl_state();
        let saved_view = self.view_matrix;
        let framebuffer = match entry.framebuffer.get() {
            Some(framebuffer) => Some(framebuffer),
            None if draws > DEDICATED_AFTER_DRAWS
                && width <= SCRATCH_SIZE
                && height <= SCRATCH_SIZE
                && DEDICATED_FRAMEBUFFERS.load(Ordering::Relaxed) < MAX_DEDICATED_FRAMEBUFFERS =>
            {
                self.create_dedicated_framebuffer(entry)
            }
            None => None,
        };
        let drawn = match framebuffer {
            Some(framebuffer) => {
                unsafe { self.gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer)) };
                self.render_tile(commands, 0, 0, width, height);
                true
            }
            None => self.render_offscreen_tiled(entry, commands, bounds),
        };
        self.view_matrix = saved_view;
        self.active_program = std::ptr::null();
        unsafe {
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            self.gl.viewport(0, 0, self.renderbuffer_width, self.renderbuffer_height);
        }
        drawn.then(|| Box::new(QueueSyncHandle { texture: handle, bounds }) as Box<dyn SyncHandle>)
    }

    fn viewport_dimensions(&self) -> ViewportDimensions {
        ViewportDimensions {
            width: self.renderbuffer_width as u32,
            height: self.renderbuffer_height as u32,
            scale_factor: self.viewport_scale_factor,
        }
    }

    fn set_viewport_dimensions(&mut self, dimensions: ViewportDimensions) {
        self.flush_batch();
        self.view_matrix = [
            [1.0 / (dimensions.width as f32 / 2.0), 0.0, 0.0, 0.0],
            [0.0, -1.0 / (dimensions.height as f32 / 2.0), 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [-1.0, 1.0, 0.0, 1.0],
        ];
        self.renderbuffer_width = dimensions.width.max(1) as i32;
        self.renderbuffer_height = dimensions.height.max(1) as i32;
        self.active_program = std::ptr::null();

        if let Err(e) = self.build_msaa_buffers() {
            log::error!("Couldn't create MSAA buffers: {e}");
        }
        unsafe {
            self.gl.viewport(0, 0, self.renderbuffer_width, self.renderbuffer_height);
        }
        self.viewport_scale_factor = dimensions.scale_factor
    }

    fn register_shape(
        &mut self,
        shape: DistilledShape,
        bitmap_source: &dyn BitmapSource,
    ) -> ShapeHandle {
        let draws = match self.register_shape_internal(shape, bitmap_source) {
            Ok(draws) => draws,
            Err(e) => {
                log::error!("Couldn't register shape: {e:?}");
                vec![]
            }
        };
        ShapeHandle(Arc::new(Mesh { draws, gl: self.gl.clone() }))
    }

    fn submit_frame(
        &mut self,
        clear: Color,
        commands: CommandList,
        _cache_entries: Vec<BitmapCacheEntry>,
    ) {
        // `is_offscreen_supported` is false, so the core never produces cache entries.
        self.begin_frame(clear);
        commands.execute(self);
        self.end_frame();
    }

    fn register_bitmap(&mut self, bitmap: Bitmap<'_>) -> Result<BitmapHandle, BitmapError> {
        let format = texture_format(bitmap.format());
        let mut bitmap = if format == glow::RGB {
            bitmap.to_rgb()
        } else {
            bitmap.to_rgba()
        };
        self.clamp_bitmap(&mut bitmap, format);
        log_large_texture("Uploading", bitmap.width(), bitmap.height());
        unsafe {
            let texture = self
                .gl
                .create_texture()
                .map_err(|e| BitmapError::Unimplemented(e.into()))?;
            self.gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            self.gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                format as i32,
                bitmap.width() as i32,
                bitmap.height() as i32,
                0,
                format,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(bitmap.data())),
            );
            let entry = RegistryData {
                gl: self.gl.clone(),
                width: bitmap.width(),
                height: bitmap.height(),
                texture,
                params: Cell::new((0, 0)),
                offscreen_draws: Cell::new(0),
                framebuffer: Cell::new(None),
            };
            entry.bind(&self.gl, glow::LINEAR, glow::CLAMP_TO_EDGE);
            Ok(BitmapHandle(Arc::new(entry)))
        }
    }

    fn update_texture(
        &mut self,
        handle: &BitmapHandle,
        bitmap: Bitmap<'_>,
        region: PixelRegion,
    ) -> Result<(), BitmapError> {
        let entry = as_registry_data(handle);
        let format = texture_format(bitmap.format());
        let mut bitmap = if format == glow::RGB {
            bitmap.to_rgb()
        } else {
            bitmap.to_rgba()
        };
        let resized = self.clamp_bitmap(&mut bitmap, format);
        let bpp: usize = if format == glow::RGB { 3 } else { 4 };

        unsafe {
            self.gl.bind_texture(glow::TEXTURE_2D, Some(entry.texture));
            let whole = resized
                || region.width() >= bitmap.width() && region.height() >= bitmap.height();
            if whole {
                self.gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    format as i32,
                    bitmap.width() as i32,
                    bitmap.height() as i32,
                    0,
                    format,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(Some(bitmap.data())),
                );
            } else {
                // GLES2 has no UNPACK_ROW_LENGTH, so gather the dirty rows.
                let (x, y, w, h) = (
                    region.x_min as usize,
                    region.y_min as usize,
                    region.width() as usize,
                    region.height() as usize,
                );
                let stride = bitmap.width() as usize * bpp;
                let data = bitmap.data();
                let pixels: Vec<u8> = if x == 0 && w == bitmap.width() as usize {
                    data[y * stride..(y + h) * stride].to_vec()
                } else {
                    let mut buf = Vec::with_capacity(w * h * bpp);
                    for row in y..y + h {
                        let start = row * stride + x * bpp;
                        buf.extend_from_slice(&data[start..start + w * bpp]);
                    }
                    buf
                };
                self.gl.tex_sub_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    x as i32,
                    y as i32,
                    w as i32,
                    h as i32,
                    format,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(Some(&pixels)),
                );
            }
        }
        Ok(())
    }

    fn create_context3d(
        &mut self,
        _profile: Context3DProfile,
    ) -> Result<Box<dyn Context3D>, BitmapError> {
        Err(BitmapError::Unimplemented("createContext3D".into()))
    }

    fn debug_info(&self) -> Cow<'static, str> {
        Cow::Borrowed("Renderer: glow")
    }

    fn name(&self) -> &'static str {
        "glow"
    }

    fn set_quality(&mut self, _quality: StageQuality) {}

    fn compile_pixelbender_shader(
        &mut self,
        _shader: ruffle_render::pixel_bender::PixelBenderShader,
    ) -> Result<ruffle_render::pixel_bender::PixelBenderShaderHandle, BitmapError> {
        Err(BitmapError::Unimplemented("compile_pixelbender_shader".into()))
    }

    fn resolve_sync_handle(
        &mut self,
        handle: Box<dyn SyncHandle>,
        with_rgba: RgbaBufRead,
    ) -> Result<(), ruffle_render::error::Error> {
        let handle = Box::<dyn Any>::downcast::<QueueSyncHandle>(handle)
            .map_err(|_| BitmapError::Unimplemented("foreign sync handle".into()))?;
        let entry = as_registry_data(&handle.texture);
        let bounds = handle.bounds;
        let (width, height) = (bounds.width(), bounds.height());
        log_large_texture("Reading back", width, height);
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        self.reset_gl_state();
        unsafe {
            // The buffer covers exactly `bounds`, rows `width * 4` bytes apart,
            // which is the layout `copy_pixels_to_bitmapdata` expects.
            if let Some(framebuffer) = entry.framebuffer.get() {
                self.gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
                self.gl.read_pixels(
                    bounds.x_min as i32,
                    bounds.y_min as i32,
                    width as i32,
                    height as i32,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    PixelPackData::Slice(Some(&mut pixels)),
                );
            } else if self.bind_scratch_framebuffer() {
                // Copy each tile into the scratch texture and read it from there.
                let mut tile_pixels = Vec::new();
                for (x, y, w, h) in tiles(bounds) {
                    self.gl.viewport(0, 0, w as i32, h as i32);
                    self.blit_texture_tile(entry, x, y, w, h);
                    tile_pixels.resize((w * h * 4) as usize, 0);
                    self.gl.read_pixels(
                        0,
                        0,
                        w as i32,
                        h as i32,
                        glow::RGBA,
                        glow::UNSIGNED_BYTE,
                        PixelPackData::Slice(Some(&mut tile_pixels)),
                    );
                    let row_bytes = (w * 4) as usize;
                    for row in 0..h {
                        let src = (row * w * 4) as usize;
                        let dst = (((y - bounds.y_min + row) * width + (x - bounds.x_min)) * 4) as usize;
                        pixels[dst..dst + row_bytes].copy_from_slice(&tile_pixels[src..src + row_bytes]);
                    }
                }
            }
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            self.gl.viewport(0, 0, self.renderbuffer_width, self.renderbuffer_height);
        }
        self.active_program = std::ptr::null();
        with_rgba(&pixels, width * 4);
        Ok(())
    }

    fn run_pixelbender_shader(
        &mut self,
        _handle: ruffle_render::pixel_bender::PixelBenderShaderHandle,
        _arguments: &[ruffle_render::pixel_bender_support::PixelBenderShaderArgument],
        _target: &PixelBenderTarget,
    ) -> Result<PixelBenderOutput, BitmapError> {
        Err(BitmapError::Unimplemented("run_pixelbender_shader".into()))
    }

    fn create_empty_texture(
        &mut self,
        width: u32,
        height: u32,
    ) -> Result<BitmapHandle, BitmapError> {
        unsafe {
            let texture = self
                .gl
                .create_texture()
                .map_err(|e| BitmapError::Unimplemented(e.into()))?;
            self.gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            log_large_texture("Creating", width, height);
            // Allocate storage so the texture can back an offscreen framebuffer.
            // It must start out transparent: vitaGL clears new textures, but
            // desktop GL leaves them undefined.
            let zeros = (!self.gl.version().is_embedded)
                .then(|| vec![0u8; width as usize * height as usize * 4]);
            self.gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA as i32,
                width as i32,
                height as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(zeros.as_deref()),
            );
            let entry = RegistryData {
                gl: self.gl.clone(),
                width,
                height,
                texture,
                params: Cell::new((0, 0)),
                offscreen_draws: Cell::new(0),
                framebuffer: Cell::new(None),
            };
            entry.bind(&self.gl, glow::LINEAR, glow::CLAMP_TO_EDGE);
            Ok(BitmapHandle(Arc::new(entry)))
        }
    }
}

impl CommandHandler for GlowRenderBackend {
    fn render_bitmap(
        &mut self,
        bitmap: BitmapHandle,
        transform: Transform,
        smoothing: bool,
        pixel_snapping: PixelSnapping,
    ) {
        self.set_stencil_state();
        let entry = as_registry_data(&bitmap);

        let mut matrix = transform.matrix;
        pixel_snapping.apply(&mut matrix);
        matrix *= Matrix::scale(entry.width as f32, entry.height as f32);

        let mult_color = transform.color_transform.mult_rgba_normalized();
        let add_color = transform.color_transform.add_rgba_normalized();

        if self.batching {
            let filter = if smoothing { glow::LINEAR } else { glow::NEAREST };
            self.begin_bitmap_batch(&bitmap, filter, false, mult_color, add_color, 4);
            self.bitmap_batch.append_quad(&matrix);
            stat(8);
            return;
        }
        let world_matrix = world_matrix(&matrix);

        self.use_program(ProgramKind::Bitmap);
        self.set_color_uniforms(&world_matrix, mult_color, add_color);

        let draw = &self.bitmap_quad_draws[0];
        if let DrawType::Bitmap(BitmapDraw { matrix, .. }) = &draw.draw_type {
            self.bitmap_program
                .uniform_matrix3fv(&self.gl, ShaderUniform::TextureMatrix, matrix);
            self.bitmap_program.uniform1f(&self.gl, ShaderUniform::Repeat, 0.0);
        }
        let filter = if smoothing { glow::LINEAR } else { glow::NEAREST };
        entry.bind(&self.gl, filter, glow::CLAMP_TO_EDGE);

        unsafe {
            self.gl.bind_vertex_array(Some(draw.vao));
            DRAW_CALLS.fetch_add(1, Ordering::Relaxed);
            self.gl
                .draw_elements(glow::TRIANGLE_FAN, draw.num_indices, glow::UNSIGNED_INT, 0);
        }
    }

    fn render_shape(&mut self, shape: ShapeHandle, transform: Transform) {
        let world_matrix = world_matrix(&transform.matrix);
        let mult_color = transform.color_transform.mult_rgba_normalized();
        let add_color = transform.color_transform.add_rgba_normalized();

        self.set_stencil_state();
        let drawing_stencil = self.mask_state == MaskState::DrawMaskStencil
            || self.mask_state == MaskState::ClearMaskStencil;

        let mesh = as_mesh(&shape);
        let mut color: Option<ColorTransformer> = None;
        for draw in &mesh.draws {
            let draw = match draw {
                MeshDraw::Cpu(cpu) => {
                    // Ignore strokes when drawing a mask stencil.
                    let count = if drawing_stencil { cpu.num_mask_indices } else { cpu.indices.len() };
                    if count > 0 {
                        let color = color
                            .get_or_insert_with(|| ColorTransformer::new(mult_color, add_color));
                        if self.gradient_batch.open_for(cpu.positions.len()) {
                            self.gradient_batch.append_color(cpu, count, &transform.matrix, color);
                        } else {
                            self.begin_color_batch(cpu.positions.len());
                            self.batch.append(cpu, count, &transform.matrix, color);
                        }
                        stat(1);
                    }
                    continue;
                }
                MeshDraw::CpuBitmap(cpu) => {
                    let count = if drawing_stencil { cpu.num_mask_indices } else { cpu.indices.len() };
                    let Some(handle) = &cpu.handle else {
                        log::warn!("Tried to render a handleless bitmap");
                        continue;
                    };
                    if count > 0 {
                        let filter = if cpu.is_smoothed { glow::LINEAR } else { glow::NEAREST };
                        self.begin_bitmap_batch(
                            handle,
                            filter,
                            cpu.is_repeating,
                            mult_color,
                            add_color,
                            cpu.positions.len(),
                        );
                        self.bitmap_batch.append(cpu, count, &transform.matrix);
                        stat(8);
                    }
                    continue;
                }
                MeshDraw::CpuGradient(cpu) => {
                    let count = if drawing_stencil { cpu.num_mask_indices } else { cpu.indices.len() };
                    if count > 0 {
                        self.begin_gradient_batch(mult_color, add_color, cpu.positions.len());
                        self.gradient_batch.append(cpu, count, &transform.matrix);
                        stat(10);
                    }
                    continue;
                }
                MeshDraw::Gpu(draw) => draw,
            };
            // Ignore strokes when drawing a mask stencil.
            let num_indices = if drawing_stencil {
                draw.num_mask_indices
            } else {
                draw.num_indices
            };
            if num_indices == 0 {
                continue;
            }

            let _zone = rv_prof::zone(rv_prof::Zone::GlDraw);
            stat(match &draw.draw_type {
                DrawType::Color => 2,
                DrawType::Gradient(_) => 3,
                DrawType::Bitmap(_) => 4,
            });
            let kind = match &draw.draw_type {
                DrawType::Color => ProgramKind::Color,
                DrawType::Gradient(_) => ProgramKind::Gradient,
                DrawType::Bitmap(_) => ProgramKind::Bitmap,
            };
            self.use_program(kind);
            self.set_color_uniforms(&world_matrix, mult_color, add_color);

            match &draw.draw_type {
                DrawType::Color => (),
                DrawType::Gradient(gradient) => {
                    let program = &self.gradient_program;
                    let gl = &self.gl;
                    program.uniform_matrix3fv(gl, ShaderUniform::TextureMatrix, &gradient.matrix);
                    program.uniform1f(gl, ShaderUniform::GradientType, gradient.gradient_type);
                    program.uniform1f(gl, ShaderUniform::GradientRepeatMode, gradient.repeat_mode);
                    program.uniform1f(gl, ShaderUniform::GradientFocalPoint, gradient.focal_point);
                    unsafe { gl.bind_texture(glow::TEXTURE_2D, Some(gradient.ramp)) };
                }
                DrawType::Bitmap(bitmap) => {
                    let Some(handle) = &bitmap.handle else {
                        log::warn!("Tried to render a handleless bitmap");
                        continue;
                    };
                    self.bitmap_program.uniform_matrix3fv(
                        &self.gl,
                        ShaderUniform::TextureMatrix,
                        &bitmap.matrix,
                    );
                    let filter = if bitmap.is_smoothed { glow::LINEAR } else { glow::NEAREST };
                    self.bitmap_program.uniform1f(
                        &self.gl,
                        ShaderUniform::Repeat,
                        if bitmap.is_repeating { 1.0 } else { 0.0 },
                    );
                    as_registry_data(handle).bind(&self.gl, filter, glow::CLAMP_TO_EDGE);
                }
            }

            unsafe {
                self.gl.bind_vertex_array(Some(draw.vao));
                DRAW_CALLS.fetch_add(1, Ordering::Relaxed);
                self.gl.draw_elements(glow::TRIANGLES, num_indices, glow::UNSIGNED_INT, 0);
            }
        }
    }

    fn render_stage3d(&mut self, _bitmap: BitmapHandle, _transform: Transform) {
        log::warn!("Stage3D is not supported by the glow backend");
    }

    fn draw_rect(&mut self, color: Color, matrix: Matrix) {
        self.draw_quad::<{ glow::TRIANGLE_FAN }, -1>(color, matrix)
    }

    fn draw_line(&mut self, color: Color, mut matrix: Matrix) {
        matrix.tx += Twips::HALF_PX;
        matrix.ty += Twips::HALF_PX;
        self.draw_quad::<{ glow::LINE_STRIP }, 2>(color, matrix)
    }

    fn draw_line_rect(&mut self, color: Color, mut matrix: Matrix) {
        matrix.tx += Twips::HALF_PX;
        matrix.ty += Twips::HALF_PX;
        self.draw_quad::<{ glow::LINE_LOOP }, -1>(color, matrix)
    }

    fn push_mask(&mut self) {
        debug_assert!(
            self.mask_state == MaskState::NoMask || self.mask_state == MaskState::DrawMaskedContent
        );
        self.num_masks += 1;
        self.mask_state = MaskState::DrawMaskStencil;
        self.mask_state_dirty = true;
    }

    fn activate_mask(&mut self) {
        debug_assert!(self.num_masks > 0 && self.mask_state == MaskState::DrawMaskStencil);
        self.mask_state = MaskState::DrawMaskedContent;
        self.mask_state_dirty = true;
    }

    fn deactivate_mask(&mut self) {
        debug_assert!(self.num_masks > 0 && self.mask_state == MaskState::DrawMaskedContent);
        self.mask_state = MaskState::ClearMaskStencil;
        self.mask_state_dirty = true;
    }

    fn pop_mask(&mut self) {
        debug_assert!(self.num_masks > 0 && self.mask_state == MaskState::ClearMaskStencil);
        self.num_masks -= 1;
        self.mask_state = if self.num_masks == 0 {
            MaskState::NoMask
        } else {
            MaskState::DrawMaskedContent
        };
        self.mask_state_dirty = true;
    }

    fn blend(&mut self, commands: CommandList, blend: RenderBlendMode) {
        self.push_blend_mode(blend);
        commands.execute(self);
        self.pop_blend_mode();
    }

    fn render_alpha_mask(&mut self, maskee_commands: CommandList, _mask_commands: CommandList) {
        // Alpha masks need offscreen compositing; draw the maskee unmasked.
        maskee_commands.execute(self);
    }
}

/// A gradient fill: its parameters plus a 256x1 RGBA colour ramp texture.
#[derive(Debug)]
struct Gradient {
    gl: Arc<glow::Context>,
    matrix: [[f32; 3]; 3],
    gradient_type: f32,
    repeat_mode: f32,
    focal_point: f32,
    ramp: glow::Texture,
}

const RAMP_SIZE: usize = 256;

/// The colour ramp of `gradient` as `RAMP_SIZE` straight (not premultiplied)
/// RGBA8 texels; the shader applies the colour transform and premultiplies.
fn ramp_pixels(gradient: &TessGradient) -> Vec<u8> {
    let linear = gradient.interpolation == swf::GradientInterpolation::LinearRgb;
    let stops: Vec<(f32, [f32; 4])> = gradient
        .records
        .iter()
        .map(|r| {
            let mut c = [
                f32::from(r.color.r) / 255.0,
                f32::from(r.color.g) / 255.0,
                f32::from(r.color.b) / 255.0,
                f32::from(r.color.a) / 255.0,
            ];
            if linear {
                srgb_to_linear(&mut c);
            }
            (f32::from(r.ratio) / 255.0, c)
        })
        .collect();

    let mut pixels = vec![0u8; RAMP_SIZE * 4];
    for (i, px) in pixels.chunks_exact_mut(4).enumerate() {
        let t = i as f32 / (RAMP_SIZE - 1) as f32;
        let mut c = match stops.as_slice() {
            [] => [0.0; 4],
            [only] => only.1,
            _ => {
                let first = stops[0];
                let last = stops[stops.len() - 1];
                if t <= first.0 {
                    first.1
                } else if t >= last.0 {
                    last.1
                } else {
                    let k = stops.windows(2).position(|w| t <= w[1].0).unwrap_or(0);
                    let (r0, c0) = stops[k];
                    let (r1, c1) = stops[k + 1];
                    let f = if r1 > r0 { (t - r0) / (r1 - r0) } else { 0.0 };
                    [0, 1, 2, 3].map(|j| c0[j] + (c1[j] - c0[j]) * f)
                }
            }
        };
        if linear {
            linear_to_srgb(&mut c);
        }
        for j in 0..4 {
            px[j] = (c[j].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        }
    }
    pixels
}

/// (type, repeat mode, focal point) as the gradient shaders take them.
fn gradient_params(gradient: &TessGradient) -> (f32, f32, f32) {
    (
        match gradient.gradient_type {
            GradientType::Linear => 0.0,
            GradientType::Radial => 1.0,
            GradientType::Focal => 2.0,
        },
        match gradient.repeat_mode {
            swf::GradientSpread::Pad => 0.0,
            swf::GradientSpread::Repeat => 1.0,
            swf::GradientSpread::Reflect => 2.0,
        },
        gradient.focal_point.to_f32().clamp(-0.98, 0.98),
    )
}

impl Gradient {
    fn new(gl: Arc<glow::Context>, gradient: TessGradient, matrix: [[f32; 3]; 3]) -> Result<Self, Error> {
        let pixels = ramp_pixels(&gradient);
        let (gradient_type, repeat_mode, focal_point) = gradient_params(&gradient);
        let ramp = unsafe {
            let tex = gl.create_texture().map_err(Error::GlCreate)?;
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
                RAMP_SIZE as i32,
                1,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(&pixels)),
            );
            tex
        };

        Ok(Self { gl, matrix, gradient_type, repeat_mode, focal_point, ramp })
    }
}

impl Drop for Gradient {
    fn drop(&mut self) {
        unsafe { self.gl.delete_texture(self.ramp) }
    }
}

#[derive(Clone, Debug)]
struct BitmapDraw {
    matrix: [[f32; 3]; 3],
    handle: Option<BitmapHandle>,
    is_repeating: bool,
    is_smoothed: bool,
}

#[derive(Debug)]
struct Mesh {
    gl: Arc<glow::Context>,
    draws: Vec<MeshDraw>,
}

impl Drop for Mesh {
    fn drop(&mut self) {
        unsafe {
            for draw in &self.draws {
                if let MeshDraw::Gpu(draw) = draw {
                    self.gl.delete_vertex_array(draw.vao);
                }
            }
        }
    }
}

impl ShapeHandleImpl for Mesh {}

fn as_mesh(handle: &ShapeHandle) -> &Mesh {
    <dyn Any>::downcast_ref(&*handle.0).expect("Shape handle must be a glow Mesh")
}

#[derive(Debug)]
struct Buffer {
    gl: Arc<glow::Context>,
    buffer: glow::Buffer,
}

impl Drop for Buffer {
    fn drop(&mut self) {
        unsafe {
            self.gl.delete_buffer(self.buffer);
        }
    }
}

#[derive(Debug)]
struct Draw {
    draw_type: DrawType,
    #[expect(dead_code)]
    vertex_buffer: Buffer,
    #[expect(dead_code)]
    index_buffer: Buffer,
    vao: glow::VertexArray,
    num_indices: i32,
    num_mask_indices: i32,
}

/// One draw of a registered shape: its own GPU buffers, or (small solid
/// colour draws) CPU data that goes through `ColorBatch`.
#[derive(Debug)]
enum MeshDraw {
    Gpu(Draw),
    Cpu(CpuDraw),
    CpuBitmap(CpuBitmapDraw),
    CpuGradient(CpuGradientDraw),
}

/// Rows in the gradient ramp atlas (512 KiB). Fills with the same ramp share
/// a row; a row is free again once no shape uses it.
const RAMP_ATLAS_ROWS: usize = 512;

/// One texture holding many gradient ramps, a row each, so that gradient
/// fills can be batched.
struct RampAtlas {
    texture: glow::Texture,
    /// A CPU copy: new ramps go here, and the changed rows are uploaded in
    /// one go before the atlas is next drawn with. Writing into a texture the
    /// GPU used recently makes vitaGL copy all of it, so once per frame at
    /// most, however many gradients a game creates.
    pixels: Vec<u8>,
    dirty: Option<(usize, usize)>,
    rows: Arc<std::sync::Mutex<RampRows>>,
}

/// Who uses which atlas row.
struct RampRows {
    free: Vec<u16>,
    refs: Vec<u32>,
    /// Row by ramp hash, to share rows between identical ramps.
    by_hash: std::collections::HashMap<u64, u16>,
    hash: Vec<u64>,
}

impl RampAtlas {
    fn new(gl: &glow::Context) -> Result<Self, Error> {
        unsafe {
            let texture = gl.create_texture().map_err(Error::GlCreate)?;
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            for (p, v) in [
                (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
            ] {
                gl.tex_parameter_i32(glow::TEXTURE_2D, p, v as i32);
            }
            let pixels = vec![0u8; RAMP_SIZE * RAMP_ATLAS_ROWS * 4];
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA as i32,
                RAMP_SIZE as i32,
                RAMP_ATLAS_ROWS as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(&pixels)),
            );
            Ok(Self {
                texture,
                pixels,
                dirty: None,
                rows: Arc::new(std::sync::Mutex::new(RampRows {
                    free: (0..RAMP_ATLAS_ROWS as u16).rev().collect(),
                    refs: vec![0; RAMP_ATLAS_ROWS],
                    by_hash: Default::default(),
                    hash: vec![0; RAMP_ATLAS_ROWS],
                })),
            })
        }
    }

    /// A row holding this ramp: a shared one if it's already there, else a
    /// free one, if any is left.
    fn allocate(&mut self, pixels: &[u8]) -> Option<RampRow> {
        use std::hash::{Hash, Hasher};
        let stride = RAMP_SIZE * 4;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        pixels.hash(&mut hasher);
        let hash = hasher.finish();
        let mut rows = self.rows.lock().ok()?;
        if let Some(&row) = rows.by_hash.get(&hash) {
            let r = usize::from(row);
            if self.pixels[r * stride..(r + 1) * stride] == *pixels {
                rows.refs[r] += 1;
                return Some(RampRow { row, rows: self.rows.clone() });
            }
        }
        let row = rows.free.pop()?;
        let r = usize::from(row);
        rows.refs[r] = 1;
        rows.hash[r] = hash;
        rows.by_hash.insert(hash, row);
        drop(rows);
        self.pixels[r * stride..(r + 1) * stride].copy_from_slice(pixels);
        self.dirty = Some(match self.dirty {
            Some((first, last)) => (first.min(r), last.max(r)),
            None => (r, r),
        });
        Some(RampRow { row, rows: self.rows.clone() })
    }

    /// Binds the atlas to unit 0, uploading rows that changed.
    fn bind(&mut self, gl: &glow::Context) {
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(self.texture));
            if let Some((first, last)) = self.dirty.take() {
                let stride = RAMP_SIZE * 4;
                gl.tex_sub_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    0,
                    first as i32,
                    RAMP_SIZE as i32,
                    (last - first + 1) as i32,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(Some(&self.pixels[first * stride..(last + 1) * stride])),
                );
            }
        }
    }
}

/// A row of the ramp atlas, given back when its shape goes away.
struct RampRow {
    row: u16,
    rows: Arc<std::sync::Mutex<RampRows>>,
}

impl std::fmt::Debug for RampRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RampRow({})", self.row)
    }
}

impl Drop for RampRow {
    fn drop(&mut self) {
        if let Ok(mut rows) = self.rows.lock() {
            let r = usize::from(self.row);
            rows.refs[r] -= 1;
            if rows.refs[r] == 0 {
                let hash = rows.hash[r];
                if rows.by_hash.get(&hash) == Some(&self.row) {
                    rows.by_hash.remove(&hash);
                }
                rows.free.push(self.row);
            }
        }
    }
}

/// A gradient fill kept on the CPU for batching.
#[derive(Debug)]
struct CpuGradientDraw {
    positions: Box<[[f32; 2]]>,
    /// In gradient space (the shader's `u_matrix * position`).
    uvs: Box<[[f32; 2]]>,
    indices: Box<[u16]>,
    num_mask_indices: usize,
    /// Atlas row v, type, repeat mode, focal point.
    params: [f32; 4],
    _row: RampRow,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct GradientVertex {
    position: [f32; 2],
    uv: [f32; 2],
    params: [f32; 4],
    /// Premultiplied colour, for solid shapes (params type 3).
    color: u32,
}

/// `GradientVertex::params` of a solid-colour shape in a gradient batch.
const SOLID_IN_GRADIENT_BATCH: [f32; 4] = [0.0, 3.0, 0.0, 0.0];

/// Gradient fills sharing a colour transform, in stage pixels, drawn with
/// one call from the ramp atlas.
struct GradientBatch {
    vertices: Vec<GradientVertex>,
    indices: Vec<u16>,
    /// The shared colour transform (mult, add).
    state: Option<([f32; 4], [f32; 4])>,
    vao: glow::VertexArray,
    vertex_buffer: glow::Buffer,
    index_buffer: glow::Buffer,
}

impl GradientBatch {
    fn new(gl: &glow::Context, program: &ShaderProgram) -> Result<Self, Error> {
        unsafe {
            let vao = gl.create_vertex_array().map_err(Error::GlCreate)?;
            let vertex_buffer = gl.create_buffer().map_err(Error::GlCreate)?;
            let index_buffer = gl.create_buffer().map_err(Error::GlCreate)?;
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vertex_buffer));
            program.bind_gradient_layout(gl);
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(index_buffer));
            gl.bind_vertex_array(None);
            Ok(Self {
                vertices: Vec::with_capacity(4096),
                indices: Vec::with_capacity(8192),
                state: None,
                vao,
                vertex_buffer,
                index_buffer,
            })
        }
    }

    fn append(&mut self, draw: &CpuGradientDraw, count: usize, matrix: &Matrix) {
        let base = self.vertices.len() as u16;
        let (a, b, c, d) = (matrix.a, matrix.b, matrix.c, matrix.d);
        let (tx, ty) = (matrix.tx.to_pixels() as f32, matrix.ty.to_pixels() as f32);
        let params = draw.params;
        self.vertices.extend(draw.positions.iter().zip(draw.uvs.iter()).map(|(p, &uv)| {
            GradientVertex {
                position: [a * p[0] + c * p[1] + tx, b * p[0] + d * p[1] + ty],
                uv,
                params,
                color: 0,
            }
        }));
        self.indices.extend(draw.indices[..count].iter().map(|&i| i + base));
    }

    /// Adds a solid-colour draw, so that fills alternating between gradients
    /// and colours (typical vector art) stay in one batch.
    fn append_color(
        &mut self,
        draw: &CpuDraw,
        count: usize,
        matrix: &Matrix,
        color: &mut ColorTransformer,
    ) {
        let base = self.vertices.len() as u16;
        let (a, b, c, d) = (matrix.a, matrix.b, matrix.c, matrix.d);
        let (tx, ty) = (matrix.tx.to_pixels() as f32, matrix.ty.to_pixels() as f32);
        for (i, p) in draw.positions.iter().enumerate() {
            let rgba = if color.identity { draw.premultiplied[i] } else { color.apply(draw.colors[i]) };
            self.vertices.push(GradientVertex {
                position: [a * p[0] + c * p[1] + tx, b * p[0] + d * p[1] + ty],
                uv: [0.0; 2],
                params: SOLID_IN_GRADIENT_BATCH,
                color: rgba,
            });
        }
        self.indices.extend(draw.indices[..count].iter().map(|&i| i + base));
    }

    fn append_rect(&mut self, matrix: &Matrix, color: u32) {
        let base = self.vertices.len() as u16;
        let (tx, ty) = (matrix.tx.to_pixels() as f32, matrix.ty.to_pixels() as f32);
        for (x, y) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
            self.vertices.push(GradientVertex {
                position: [matrix.a * x + matrix.c * y + tx, matrix.b * x + matrix.d * y + ty],
                uv: [0.0; 2],
                params: SOLID_IN_GRADIENT_BATCH,
                color,
            });
        }
        self.indices.extend([0, 1, 2, 0, 2, 3].map(|i| i + base));
    }

    /// Whether a solid shape of `vertices` vertices can join this batch now.
    fn open_for(&self, vertices: usize) -> bool {
        !self.indices.is_empty() && self.vertices.len() + vertices <= BATCH_MAX_VERTICES
    }
}

/// `matrix * vec3(x, y, 1)` for a column-major 3x3 matrix, as in the shaders.
fn apply_uv_matrix(m: &[[f32; 3]; 3], x: f32, y: f32) -> [f32; 2] {
    [m[0][0] * x + m[1][0] * y + m[2][0], m[0][1] * x + m[1][1] * y + m[2][1]]
}

/// A bitmap fill kept on the CPU for batching, with its UVs worked out.
#[derive(Debug)]
struct CpuBitmapDraw {
    positions: Box<[[f32; 2]]>,
    uvs: Box<[[f32; 2]]>,
    indices: Box<[u16]>,
    num_mask_indices: usize,
    handle: Option<BitmapHandle>,
    is_smoothed: bool,
    is_repeating: bool,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct UvVertex {
    position: [f32; 2],
    uv: [f32; 2],
}

/// What every shape in a `BitmapBatch` shares.
struct BitmapBatchState {
    handle: BitmapHandle,
    filter: u32,
    repeat: bool,
    mult: [f32; 4],
    add: [f32; 4],
}

/// Bitmap fills (and Bitmap objects) with the same texture, sampling and
/// colour transform, in stage pixels, drawn with one call.
struct BitmapBatch {
    vertices: Vec<UvVertex>,
    indices: Vec<u16>,
    state: Option<BitmapBatchState>,
    vao: glow::VertexArray,
    vertex_buffer: glow::Buffer,
    index_buffer: glow::Buffer,
}

impl BitmapBatch {
    fn new(gl: &glow::Context, program: &ShaderProgram) -> Result<Self, Error> {
        unsafe {
            let vao = gl.create_vertex_array().map_err(Error::GlCreate)?;
            let vertex_buffer = gl.create_buffer().map_err(Error::GlCreate)?;
            let index_buffer = gl.create_buffer().map_err(Error::GlCreate)?;
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vertex_buffer));
            program.bind_uv_layout(gl);
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(index_buffer));
            gl.bind_vertex_array(None);
            Ok(Self {
                vertices: Vec::with_capacity(4096),
                indices: Vec::with_capacity(8192),
                state: None,
                vao,
                vertex_buffer,
                index_buffer,
            })
        }
    }

    fn append(&mut self, draw: &CpuBitmapDraw, count: usize, matrix: &Matrix) {
        let base = self.vertices.len() as u16;
        let (a, b, c, d) = (matrix.a, matrix.b, matrix.c, matrix.d);
        let (tx, ty) = (matrix.tx.to_pixels() as f32, matrix.ty.to_pixels() as f32);
        self.vertices.extend(draw.positions.iter().zip(draw.uvs.iter()).map(|(p, &uv)| UvVertex {
            position: [a * p[0] + c * p[1] + tx, b * p[0] + d * p[1] + ty],
            uv,
        }));
        self.indices.extend(draw.indices[..count].iter().map(|&i| i + base));
    }

    /// The whole texture on the unit square under `matrix`.
    fn append_quad(&mut self, matrix: &Matrix) {
        let base = self.vertices.len() as u16;
        let (tx, ty) = (matrix.tx.to_pixels() as f32, matrix.ty.to_pixels() as f32);
        for (x, y) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
            self.vertices.push(UvVertex {
                position: [matrix.a * x + matrix.c * y + tx, matrix.b * x + matrix.d * y + ty],
                uv: [x, y],
            });
        }
        self.indices.extend([0, 1, 2, 0, 2, 3].map(|i| i + base));
    }
}

/// A solid-colour draw kept on the CPU for batching.
#[derive(Debug)]
struct CpuDraw {
    /// In shape space.
    positions: Box<[[f32; 2]]>,
    /// Straight RGBA, as the tessellator made them.
    colors: Box<[u32]>,
    /// `colors` premultiplied: the result of an identity colour transform.
    premultiplied: Box<[u32]>,
    /// Fills first, then strokes.
    indices: Box<[u16]>,
    /// Masks draw only the fills: this many indices.
    num_mask_indices: usize,
}

impl CpuDraw {
    fn new(vertices: Vec<Vertex>, indices: &[u32], num_mask_indices: usize) -> Self {
        let colors: Box<[u32]> = vertices.iter().map(|v| v.color).collect();
        Self {
            positions: vertices.iter().map(|v| v.position).collect(),
            premultiplied: colors.iter().map(|&c| premultiply(c)).collect(),
            colors,
            indices: indices.iter().map(|&i| i as u16).collect(),
            num_mask_indices,
        }
    }
}

/// Straight RGBA8 to premultiplied, rounded like the GPU's 8-bit output.
fn premultiply(color: u32) -> u32 {
    let [r, g, b, a] = color.to_le_bytes();
    let mul = |c: u8| ((u32::from(c) * u32::from(a) + 127) / 255) as u8;
    u32::from_le_bytes([mul(r), mul(g), mul(b), a])
}

/// Applies a colour transform on the CPU exactly like `color.vert`:
/// `clamp(color * mult + add)`, then premultiplies. Shapes use few distinct
/// colours, so the last result is cached.
struct ColorTransformer {
    identity: bool,
    mult: [f32; 4],
    add: [f32; 4],
    last: Option<(u32, u32)>,
}

impl ColorTransformer {
    fn new(mult: [f32; 4], add: [f32; 4]) -> Self {
        Self { identity: mult == [1.0; 4] && add == [0.0; 4], mult, add, last: None }
    }

    #[inline]
    fn apply(&mut self, color: u32) -> u32 {
        if let Some((from, to)) = self.last {
            if from == color {
                return to;
            }
        }
        let [r, g, b, a] = color.to_le_bytes();
        let channel =
            |c: u8, i: usize| (f32::from(c) / 255.0 * self.mult[i] + self.add[i]).clamp(0.0, 1.0);
        let (r, g, b, a) = (channel(r, 0), channel(g, 1), channel(b, 2), channel(a, 3));
        let to_u8 = |v: f32| (v * 255.0 + 0.5) as u8;
        let out = u32::from_le_bytes([to_u8(r * a), to_u8(g * a), to_u8(b * a), to_u8(a)]);
        self.last = Some((color, out));
        out
    }
}

/// Solid-colour shapes collected in drawing order, already in stage pixels
/// with their colour transforms applied, drawn with one call when anything
/// else needs the GPU (`GlowRenderBackend::flush_batch`).
struct ColorBatch {
    vertices: Vec<Vertex>,
    indices: Vec<u16>,
    vao: glow::VertexArray,
    vertex_buffer: glow::Buffer,
    index_buffer: glow::Buffer,
}

impl ColorBatch {
    fn new(gl: &glow::Context, program: &ShaderProgram) -> Result<Self, Error> {
        unsafe {
            let vao = gl.create_vertex_array().map_err(Error::GlCreate)?;
            let vertex_buffer = gl.create_buffer().map_err(Error::GlCreate)?;
            let index_buffer = gl.create_buffer().map_err(Error::GlCreate)?;
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vertex_buffer));
            program.bind_vertex_layout(gl);
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(index_buffer));
            gl.bind_vertex_array(None);
            Ok(Self {
                vertices: Vec::with_capacity(8192),
                indices: Vec::with_capacity(16384),
                vao,
                vertex_buffer,
                index_buffer,
            })
        }
    }

    /// Adds the first `count` indices of `draw` (and all its vertices).
    fn append(&mut self, draw: &CpuDraw, count: usize, matrix: &Matrix, color: &mut ColorTransformer) {
        let base = self.vertices.len() as u16;
        let (a, b, c, d) = (matrix.a, matrix.b, matrix.c, matrix.d);
        let (tx, ty) = (matrix.tx.to_pixels() as f32, matrix.ty.to_pixels() as f32);
        let place = |p: &[f32; 2]| [a * p[0] + c * p[1] + tx, b * p[0] + d * p[1] + ty];
        if color.identity {
            self.vertices.extend(
                draw.positions
                    .iter()
                    .zip(draw.premultiplied.iter())
                    .map(|(p, &color)| Vertex { position: place(p), color }),
            );
        } else {
            self.vertices.extend(
                draw.positions
                    .iter()
                    .zip(draw.colors.iter())
                    .map(|(p, &raw)| Vertex { position: place(p), color: color.apply(raw) }),
            );
        }
        self.indices.extend(draw.indices[..count].iter().map(|&i| i + base));
    }

    /// Adds the unit square under `matrix` in one premultiplied colour.
    fn append_rect(&mut self, matrix: &Matrix, color: u32) {
        let base = self.vertices.len() as u16;
        let (tx, ty) = (matrix.tx.to_pixels() as f32, matrix.ty.to_pixels() as f32);
        for (x, y) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
            self.vertices.push(Vertex {
                position: [matrix.a * x + matrix.c * y + tx, matrix.b * x + matrix.d * y + ty],
                color,
            });
        }
        self.indices.extend([0, 1, 2, 0, 2, 3].map(|i| i + base));
    }
}

#[derive(Debug)]
enum DrawType {
    Color,
    Gradient(Box<Gradient>),
    Bitmap(BitmapDraw),
}

struct MsaaBuffers {
    color_renderbuffer: glow::Renderbuffer,
    stencil_renderbuffer: glow::Renderbuffer,
    render_framebuffer: glow::Framebuffer,
    color_framebuffer: glow::Framebuffer,
    framebuffer_texture: glow::Texture,
}

// Every shader looks up every uniform; missing ones resolve to `None`.
struct ShaderProgram {
    program: glow::Program,
    uniforms: [Option<glow::UniformLocation>; NUM_UNIFORMS],
    vertex_position_location: Option<u32>,
    vertex_color_location: Option<u32>,
    vertex_uv_location: Option<u32>,
    vertex_params_location: Option<u32>,
}

const NUM_UNIFORMS: usize = 10;
const UNIFORM_NAMES: [&str; NUM_UNIFORMS] = [
    "world_matrix",
    "view_matrix",
    "mult_color",
    "add_color",
    "u_matrix",
    "u_gradient_type",
    "u_repeat_mode",
    "u_focal_point",
    "u_texture",
    "u_repeat",
];

enum ShaderUniform {
    WorldMatrix = 0,
    ViewMatrix,
    MultColor,
    AddColor,
    TextureMatrix,
    GradientType,
    GradientRepeatMode,
    GradientFocalPoint,
    BitmapTexture,
    Repeat,
}

impl ShaderProgram {
    fn new(
        gl: &glow::Context,
        vertex_shader: glow::Shader,
        fragment_shader: glow::Shader,
    ) -> Result<Self, Error> {
        unsafe {
            let program = gl.create_program().map_err(Error::GlCreate)?;
            gl.attach_shader(program, vertex_shader);
            gl.attach_shader(program, fragment_shader);
            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                return Err(Error::LinkingShaderProgram(gl.get_program_info_log(program)));
            }

            let mut uniforms: [Option<glow::UniformLocation>; NUM_UNIFORMS] = Default::default();
            for (slot, name) in uniforms.iter_mut().zip(UNIFORM_NAMES) {
                *slot = gl.get_uniform_location(program, name);
            }

            let this = ShaderProgram {
                program,
                uniforms,
                vertex_position_location: gl.get_attrib_location(program, "position"),
                vertex_color_location: gl.get_attrib_location(program, "color"),
                vertex_uv_location: gl.get_attrib_location(program, "uv"),
                vertex_params_location: gl.get_attrib_location(program, "params"),
            };
            // The sampler always reads unit 0; set it once instead of per draw.
            gl.use_program(Some(program));
            this.uniform1i(gl, ShaderUniform::BitmapTexture, 0);
            gl.use_program(None);
            Ok(this)
        }
    }

    /// Configures the currently bound VAO's attributes for this program.
    fn bind_vertex_layout(&self, gl: &glow::Context) {
        unsafe {
            if let Some(loc) = self.vertex_position_location {
                gl.vertex_attrib_pointer_f32(loc, 2, glow::FLOAT, false, 12, 0);
                gl.enable_vertex_attrib_array(loc);
            }
            if let Some(loc) = self.vertex_color_location {
                gl.vertex_attrib_pointer_f32(loc, 4, glow::UNSIGNED_BYTE, true, 12, 8);
                gl.enable_vertex_attrib_array(loc);
            }
        }
    }

    /// Like `bind_vertex_layout`, for `UvVertex` (the bitmap batch).
    fn bind_uv_layout(&self, gl: &glow::Context) {
        unsafe {
            if let Some(loc) = self.vertex_position_location {
                gl.vertex_attrib_pointer_f32(loc, 2, glow::FLOAT, false, 16, 0);
                gl.enable_vertex_attrib_array(loc);
            }
            if let Some(loc) = self.vertex_uv_location {
                gl.vertex_attrib_pointer_f32(loc, 2, glow::FLOAT, false, 16, 8);
                gl.enable_vertex_attrib_array(loc);
            }
        }
    }

    /// Like `bind_vertex_layout`, for `GradientVertex` (the gradient batch).
    fn bind_gradient_layout(&self, gl: &glow::Context) {
        unsafe {
            for (loc, size, offset) in [
                (self.vertex_position_location, 2, 0),
                (self.vertex_uv_location, 2, 8),
                (self.vertex_params_location, 4, 16),
            ] {
                if let Some(loc) = loc {
                    gl.vertex_attrib_pointer_f32(loc, size, glow::FLOAT, false, 36, offset);
                    gl.enable_vertex_attrib_array(loc);
                }
            }
            if let Some(loc) = self.vertex_color_location {
                gl.vertex_attrib_pointer_f32(loc, 4, glow::UNSIGNED_BYTE, true, 36, 32);
                gl.enable_vertex_attrib_array(loc);
            }
        }
    }

    fn uniform1f(&self, gl: &glow::Context, uniform: ShaderUniform, value: f32) {
        unsafe {
            gl.uniform_1_f32(self.uniforms[uniform as usize].as_ref(), value);
        }
    }

    fn uniform1i(&self, gl: &glow::Context, uniform: ShaderUniform, value: i32) {
        unsafe {
            gl.uniform_1_i32(self.uniforms[uniform as usize].as_ref(), value);
        }
    }

    fn uniform4fv(&self, gl: &glow::Context, uniform: ShaderUniform, values: &[f32]) {
        unsafe {
            gl.uniform_4_f32_slice(self.uniforms[uniform as usize].as_ref(), values);
        }
    }

    fn uniform_matrix3fv(&self, gl: &glow::Context, uniform: ShaderUniform, values: &[[f32; 3]; 3]) {
        unsafe {
            gl.uniform_matrix_3_f32_slice(
                self.uniforms[uniform as usize].as_ref(),
                false,
                bytemuck::cast_slice(values),
            );
        }
    }

    fn uniform_matrix4fv(&self, gl: &glow::Context, uniform: ShaderUniform, values: &[[f32; 4]; 4]) {
        unsafe {
            gl.uniform_matrix_4_f32_slice(
                self.uniforms[uniform as usize].as_ref(),
                false,
                bytemuck::cast_slice(values),
            );
        }
    }
}

/// Converts an RGBA color from linear back to sRGB space.
fn linear_to_srgb(color: &mut [f32; 4]) {
    for n in &mut color[..3] {
        let v = n.max(0.0);
        *n = if v <= 0.0031308 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
    }
}

/// Converts an RGBA color from sRGB space to linear color space.
fn srgb_to_linear(color: &mut [f32; 4]) {
    for n in &mut color[..3] {
        *n = if *n <= 0.04045 {
            *n / 12.92
        } else {
            f32::powf((*n + 0.055) / 1.055, 2.4)
        };
    }
}
