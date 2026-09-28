//! OpenGL ES 2 (glow) render backend for Ruffle, tuned for vitaGL on PS Vita.
//!
//! Derived from Ruffle's WebGL backend. Differences worth knowing about:
//! - GL state is fully re-established at the start of every frame, so other
//!   GL users (FlashVita's UI overlay) can draw between frames safely.
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

    offscreen_framebuffer: glow::Framebuffer,

    color_program: ShaderProgram,
    bitmap_program: ShaderProgram,
    gradient_program: ShaderProgram,

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

            let color_program = ShaderProgram::new(&gl, color_vertex, color_fragment)?;
            let bitmap_program = ShaderProgram::new(&gl, texture_vertex, bitmap_fragment)?;
            let gradient_program = ShaderProgram::new(&gl, texture_vertex, gradient_fragment)?;
            for shader in [
                color_vertex,
                texture_vertex,
                color_fragment,
                bitmap_fragment,
                gradient_fragment,
            ] {
                gl.delete_shader(shader);
            }

            let offscreen_framebuffer = gl.create_framebuffer().map_err(Error::GlCreate)?;

            let mut renderer = Self {
                gl,
                msaa_buffers: None,
                msaa_sample_count,
                max_texture_size,
                offscreen_framebuffer,
                color_program,
                gradient_program,
                bitmap_program,
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
    ) -> Result<Vec<Draw>, Error> {
        use ruffle_render::tessellator::DrawType as TessDrawType;

        let lyon_mesh = self.shape_tessellator.tessellate_shape(shape, bitmap_source);

        let mut draws = Vec::with_capacity(lyon_mesh.draws.len());
        for draw in lyon_mesh.draws {
            let num_indices = draw.indices.len() as i32;
            let num_mask_indices = draw.mask_index_count as i32;

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

                draws.push(Draw {
                    draw_type,
                    vao,
                    vertex_buffer: Buffer { gl: self.gl.clone(), buffer: vertex_buffer },
                    index_buffer: Buffer { gl: self.gl.clone(), buffer: index_buffer },
                    num_indices,
                    num_mask_indices,
                });
            }
        }

        Ok(draws)
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
            gl.draw_elements(glow::TRIANGLE_FAN, quad.num_indices, glow::UNSIGNED_INT, 0);
            gl.bind_vertex_array(None);
            self.apply_blend_mode(&self.current_blend_mode());
        }
    }

    fn push_blend_mode(&mut self, blend: RenderBlendMode) {
        if !same_blend_mode(self.blend_modes.last(), &blend) {
            self.apply_blend_mode(&blend);
        }
        self.blend_modes.push(blend);
    }

    fn pop_blend_mode(&mut self) {
        let old = self.blend_modes.pop();
        let current = self.current_blend_mode();
        if !same_blend_mode(old.as_ref(), &current) {
            self.apply_blend_mode(&current);
        }
    }

    /// Switches programs if needed, invalidating the cached uniforms.
    fn use_program(&mut self, which: ProgramKind) {
        let program = match which {
            ProgramKind::Color => &self.color_program,
            ProgramKind::Bitmap => &self.bitmap_program,
            ProgramKind::Gradient => &self.gradient_program,
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
        let world_matrix = world_matrix(&matrix);
        let mult_color = [
            color.r as f32 / 255.0,
            color.g as f32 / 255.0,
            color.b as f32 / 255.0,
            color.a as f32 / 255.0,
        ];

        self.set_stencil_state();
        self.use_program(ProgramKind::Color);
        self.set_color_uniforms(&world_matrix, mult_color, [0.0; 4]);

        let quad = &self.color_quad_draws[0];
        let count = if COUNT < 0 { quad.num_indices } else { COUNT };
        unsafe {
            self.gl.bind_vertex_array(Some(quad.vao));
            self.gl.draw_elements(MODE, count, glow::UNSIGNED_INT, 0);
        }
    }
}

#[derive(Clone, Copy)]
enum ProgramKind {
    Color,
    Bitmap,
    Gradient,
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
        let (texture, width, height) = {
            let entry = as_registry_data(&handle);
            (entry.texture, entry.width, entry.height)
        };

        self.reset_gl_state();
        unsafe {
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.offscreen_framebuffer));
            self.gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(texture),
                0,
            );
            if self.gl.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
                log::error!("Offscreen framebuffer incomplete; skipping BitmapData.draw");
                self.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                return None;
            }

            self.gl.viewport(0, 0, width as i32, height as i32);
            let saved_view = self.view_matrix;
            // Note: un-flipped Y, so texture row 0 is the top of the bitmap.
            self.view_matrix = [
                [2.0 / width as f32, 0.0, 0.0, 0.0],
                [0.0, 2.0 / height as f32, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [-1.0, -1.0, 0.0, 1.0],
            ];

            self.set_stencil_state();
            commands.execute(self);

            self.view_matrix = saved_view;
            self.active_program = std::ptr::null();
            self.gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                None,
                0,
            );
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            self.gl.viewport(0, 0, self.renderbuffer_width, self.renderbuffer_height);
        }
        Some(Box::new(QueueSyncHandle { texture: handle, bounds }))
    }

    fn viewport_dimensions(&self) -> ViewportDimensions {
        ViewportDimensions {
            width: self.renderbuffer_width as u32,
            height: self.renderbuffer_height as u32,
            scale_factor: self.viewport_scale_factor,
        }
    }

    fn set_viewport_dimensions(&mut self, dimensions: ViewportDimensions) {
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
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        unsafe {
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.offscreen_framebuffer));
            self.gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(entry.texture),
                0,
            );
            self.gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
            // The buffer covers exactly `bounds`, rows `width * 4` bytes apart,
            // which is the layout `copy_pixels_to_bitmapdata` expects.
            self.gl.read_pixels(
                bounds.x_min as i32,
                bounds.y_min as i32,
                width as i32,
                height as i32,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                PixelPackData::Slice(Some(&mut pixels)),
            );
            self.gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                None,
                0,
            );
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
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
            // Allocate storage so the texture can back an offscreen framebuffer.
            self.gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA as i32,
                width as i32,
                height as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
            );
            let entry = RegistryData {
                gl: self.gl.clone(),
                width,
                height,
                texture,
                params: Cell::new((0, 0)),
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
        let world_matrix = world_matrix(&matrix);

        let mult_color = transform.color_transform.mult_rgba_normalized();
        let add_color = transform.color_transform.add_rgba_normalized();

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
        for draw in &mesh.draws {
            // Ignore strokes when drawing a mask stencil.
            let num_indices = if drawing_stencil {
                draw.num_mask_indices
            } else {
                draw.num_indices
            };
            if num_indices == 0 {
                continue;
            }

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

impl Gradient {
    fn new(gl: Arc<glow::Context>, gradient: TessGradient, matrix: [[f32; 3]; 3]) -> Result<Self, Error> {
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

        // Straight (non-premultiplied) colours; the shader applies the colour
        // transform and premultiplies.
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

        Ok(Self {
            gl,
            matrix,
            gradient_type: match gradient.gradient_type {
                GradientType::Linear => 0.0,
                GradientType::Radial => 1.0,
                GradientType::Focal => 2.0,
            },
            repeat_mode: match gradient.repeat_mode {
                swf::GradientSpread::Pad => 0.0,
                swf::GradientSpread::Repeat => 1.0,
                swf::GradientSpread::Reflect => 2.0,
            },
            focal_point: gradient.focal_point.to_f32().clamp(-0.98, 0.98),
            ramp,
        })
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
    draws: Vec<Draw>,
}

impl Drop for Mesh {
    fn drop(&mut self) {
        unsafe {
            for draw in &self.draws {
                self.gl.delete_vertex_array(draw.vao);
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
