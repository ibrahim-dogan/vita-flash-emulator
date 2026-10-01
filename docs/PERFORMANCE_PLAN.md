# RuffleVita performance plan: phase 1

RuffleVita / İbrahim Doğan. Status: the measurement tools, draw-call batching and the first interpreter fast paths are implemented (see **Results so far** below). The rest is design.

The long-term goal is ahead-of-time compilation of ActionScript 3: capture the optimized AVM2 code on a PC, translate it to C and load it on the Vita as a `.suprx`. That only helps AS3 games, and only the games someone has captured. The first weeks go to work that speeds up every game:

1. **Measurement**, so every change below shows up as a number.
2. **Draw-call batching** in `ruffle_render_glow`. This helps every game, especially AS1/AS2 games, which are mostly drawing-bound.
3. **Fast paths in the AVM2 interpreter**, for AS3 games until AOT exists, and for all the methods AOT won't cover.
4. **The AOT C ABI** (`emulator/aot/rv_api.h`), written now so the interpreter work doesn't drift away from it.

Baseline numbers from 1.0.1 and 1.1.0:

| Game | Where the time goes on the Vita | Desktop |
| --- | --- | --- |
| A Koopa's Revenge 2 (AS2) | Before 1.0.1: tick about 350 ms, render about 11 ms at 290–300 draws. After 1.0.1: 12–13 fps | |
| Happy Wheels (AS3) | tick 200–270 ms, render about 2 ms, 4–5 fps | tick about 7 ms. `run_actions` is 64% of self time, `exec` 8%, coercions 7%. About 6,000 allocations per frame (580 KB). GC takes about 5% |

AS3 games are interpreter-bound. AS2 games with many small shapes are partly draw-bound: about 11 ms for about 300 draws is roughly 35 µs per draw, all of it CPU time inside vitaGL.

## Results so far

All numbers come from the desktop timedemo (see *Measuring* below): the same input on the same frames every run. **Instructions** are retired instructions of the main thread per frame during `tick`. That count is repeatable to about 0.02%, and is a better stand-in for the Vita's CPU than milliseconds on an M1, which hide most interpreter overhead.

**Interpreter** (Happy Wheels, frames 800–2000, riding):

| Change | Instructions per frame |
| --- | --- |
| 1.1.0 | 64.71 M |
| + superinstructions (`GetLocalSlot`, `GetLocal2`, `SetLocalGetLocal`, fused compare-and-branch), timeout checks only on backward jumps | 61.17 M |
| + inline Number/int paths in `coerce_to_number` / `coerce_to_i32` / `coerce_to_u32` / `coerce_to_type`, Number×Number in `multiply`, no `is_of_type` assert per call | 55.07 M |
| + only locals cleared in new stack frames, inline `resolve_info` check | 54.99 M |

That is 15% fewer instructions for the same work.

**AVM1** (Koopa's Revenge 2, frames 1000–2400): `ActionConstantPool` decoded and interned its strings again every time a frame script or function started. Decoded pools are now cached, keyed by the address, length and encoding of their bytes. The bytes are compared before use, so a reused address can't return stale strings. That takes 18.95 M instructions per frame to 17.89 M (−5.6%).

Two further changes speed up the Vita without showing up on the desktop:

- **ToInt32 / ToUint32 in one instruction.** They used `fmod` from newlib on 32-bit ARM. aarch64 already has `fjcvtzs`.
- **Fewer stores per call.** Less of each new stack frame is cleared.

Ruffle's own regression suite (3,884 tests, AVM1 and AVM2) passes with every change.

**Draw-call batching.** These are draw calls per frame, before and after. Screenshots of the same frames are identical below the FPS overlay, or at most 1–2/255 apart from colour-transform rounding.

| Game | Before | After |
| --- | --- | --- |
| A Koopa's Revenge 2 | 233 | 59 |
| Happy Wheels | 65 | 23 |
| Bloons Tower Defense 3 | 627 | 85 |
| aqua-energizer | 194 | 9 |
| gold01 | 192 | 2 |
| 1-on-1 Soccer Brazil | 110 | 11 |
| n.swf | 396 | 6 |
| Road of the Dead | 72 | 4 |
| Age of War | 26 | 3 |

Four batches are implemented, all in `ruffle_render_glow/src/lib.rs`:

- **Solid colours** (`ColorBatch`), as designed in 1.2.
- **Bitmap fills and Bitmap objects** (`BitmapBatch`). Fills are grouped by texture, filter, repeat mode and colour transform, and their UVs are worked out when the shape is registered.
- **Gradient fills** (`GradientBatch`). Every ramp lives in one 256×512 atlas (`RampAtlas`). Identical ramps share a row, and changed rows are uploaded at most once before each draw that uses the atlas. Type, repeat mode, focal point and atlas row travel with the vertices, so consecutive gradients batch whatever they look like.
- **Solid colours inside a gradient batch.** Vector art alternates gradient fills and solid strokes, so solid shapes that come while a gradient batch is open join it as "type 3" vertices.

At most one of the three batches holds anything at a time, which keeps the drawing order exact.

### Measuring

These only exist in the desktop build.

- `RUFFLEVITA_HIDDEN=1` runs without a window, a Dock icon or focus stealing.
- `RUFFLEVITA_BENCH=<first>-<last>` is the timedemo mode:
  - every tick runs exactly one frame, as fast as possible;
  - the SWF loads completely up front;
  - `getTimer()` follows game time and `Math.random` has a fixed seed (`ruffle_core::rv_clock`);
  - over the given frame range it prints time, instructions and draw calls per frame, plus what the draws were.
- `RUFFLEVITA_SCRIPT` takes `at <frame>`, which waits for an absolute game frame, so input lands on the same frames every run.
- `RUFFLEVITA_NO_BATCH=1` turns batching off, for A/B screenshots.
- `cargo build --features opstats` logs the most executed AVM2 ops and op pairs.
- `cargo build --features prof` keeps op handlers out of line, so a sampling profiler can see them.

---

## 0. Measurement (week 1)

We already have `DRAW_CALLS` (`ruffle_render_glow/src/lib.rs:100`), `take_draw_calls()` and the perf line in `src/session.rs`, which logs `tick`, `render` and `present` every 10 s when the FPS counter is on. Add the following.

**Renderer counters.** These are the same kind of atomics as `DRAW_CALLS`. They are printed on the perf line and reset each time it prints.

- `SHAPES`: `render_shape` calls. `BATCHED_SHAPES`: shapes that went into a batch.
- `FLUSHES[reason]`: one counter per `FlushReason`, listed in 1.3.
- `BATCH_VERTS`: vertices written to the stream buffer.
- The perf line becomes, for example: `render 6.1ms draws 41 (shapes 612, batched 575; flush prog 18 mask 9 blend 2 full 0 end 1)`.

**Interpreter counters** go behind a cargo feature `rv_opstats`, so release builds pay nothing.

- **Op histogram.** In `run_actions` (`patches/ruffle/core/src/avm2/activation.rs:613`), increment `OP_COUNTS[op_index(op)]`. `op_index` can be `std::mem::discriminant` hashed into a fixed table, or a generated `match` that returns `u8`.
- **Op-pair histogram.** Count `(previous op, op)` pairs. This decides which superinstructions to build (2.6).
- **Per-method time.** In `function.rs:exec`, time `run_actions` with `sceKernelGetProcessTimeWide` on the Vita and `Instant` on desktop, keyed by `Method` pointer. Every 10 s, dump the top 30 methods by self time and call count to `log.txt`. The same table later tells AOT which methods matter.

**Benchmark protocol.** Use fixed scenes:

- Koopa's Revenge 2, 30 s after character select.
- Happy Wheels, the first level, standing still, then riding.
- One AS2 game that uses many masks, such as a scrolling menu.
- One AS3 game that uses `BitmapData` blitting.

For each scene, average 10 s of the perf line, on the Vita and on desktop, before and after every change. Keep the results in a table in this file.

---

## 1. Draw-call batching

### 1.1 What a draw costs today

`render_shape` (`lib.rs:1580`) runs this sequence for every `Draw` of every shape:

```
set_stencil_state()                       // no-op unless the mask state changed
use_program(kind)                         // cached; a switch re-uploads view_matrix
uniform_matrix4fv(world_matrix)           // always: 16 floats
uniform4fv(mult_color) / (add_color)      // only when they changed
[gradient: 1 mat3 + 3 floats + bind ramp] / [bitmap: 1 mat3 + 1 float + bind texture]
bind_vertex_array(draw.vao)
draw_elements(TRIANGLES, n, UNSIGNED_INT)
```

On vitaGL every `draw_elements` does all of the following on the CPU:

- validates state,
- reserves space in the uniform ring for each program stage and copies every dirty uniform into it,
- sets the vertex streams,
- emits the GXM draw.

The world matrix changes for nearly every shape, so the per-draw minimum is one uniform upload plus one draw.

Shapes are tessellated once, in `register_shape_internal` (`lib.rs:493`). Each `Draw` gets its own VAO, VBO and IBO in `STATIC_DRAW` memory. Vertices are `Vertex { position: [f32; 2], color: u32 }` (12 bytes) in shape space, and indices are `u32`.

The number of draws is the number of `Draw`s the lyon tessellator produced, summed over every shape drawn this frame, plus one per `draw_rect` and `render_bitmap`. A typical AS2 scene is dozens to hundreds of small, solid-colour shapes: backgrounds, HUD pieces, characters built from parts, text drawn as shapes.

### 1.2 Design: a streaming batch for solid-colour shapes

Solid-colour draws (`DrawType::Color`) are the common case, and the only kind whose shader output depends on nothing but the vertex and two uniforms. Move that work to the CPU and draw many shapes at once:

- **Transform on the CPU.** When a small colour shape is drawn, transform its vertices into pixel space (2×3 affine), apply the colour transform and premultiply, and append the results to a per-frame stream buffer.
- **One shader, one draw per batch.** A new `batched_color` program takes only `view_matrix`. It has no world matrix and no colour uniforms, and draws the whole buffer with one `draw_elements`.
- **Flush on any state change.** When the next command can't join the batch (another program, a stencil or blend change, an offscreen pass, the end of the frame), draw the batch first. Commands stay in order, so painter's order is preserved by construction. Nothing is ever reordered.
- **Big meshes stay static.** A shape with many vertices keeps its GPU buffers and the existing path. For those the per-vertex CPU work would cost more than the draw call.

#### Data

```rust
/// A Color draw kept on the CPU for batching (replaces the GPU buffers).
struct CpuDraw {
    positions: Box<[[f32; 2]]>, // shape space, as the tessellator produced them
    colors: Box<[u32]>,         // straight RGBA8, as in Vertex
    premultiplied: Box<[u32]>,  // colors, premultiplied: used when the colour transform is identity
    indices: Box<[u16]>,        // fills first, then strokes (as today)
    num_mask_indices: u32,
}

enum Geometry {
    Gpu { vao: glow::VertexArray, vertex_buffer: Buffer, index_buffer: Buffer },
    Cpu(CpuDraw),
}

struct Draw { draw_type: DrawType, geometry: Geometry, num_indices: i32, num_mask_indices: i32 }

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct BatchVertex { position: [f32; 2], color: u32 } // pixel space, premultiplied

struct ColorBatch {
    vertices: Vec<BatchVertex>, // capacity MAX_BATCH_VERTICES, reused every frame
    indices: Vec<u16>,
    shapes: u32,                // for the counters
    vbo: glow::Buffer,
    ibo: glow::Buffer,
    vao: glow::VertexArray,
}

const BATCHABLE_MAX_VERTICES: usize = 1024; // per Draw; tune with measurements
const MAX_BATCH_VERTICES: usize = 65_535;   // u16 indices
```

`Geometry::Cpu` also means no VAO, VBO or IBO for most shapes. That takes pressure off the vitaGL VAO pool that ran out in 1.0.1, and off GPU memory. A `Geometry::Cpu` mesh is 16 bytes per vertex plus 2 bytes per index, all in main RAM.

#### Shader

`shaders/batched_color.vert`, used with the existing `color.frag`:

```glsl
#version 100
uniform mat4 view_matrix;
attribute vec2 position;
attribute vec4 color;
varying vec4 frag_color;
void main() {
    frag_color = color;                       // already transformed and premultiplied
    gl_Position = view_matrix * vec4(position, 0.0, 1.0);
}
```

#### CPU work per vertex

It must match `color.vert` exactly: `clamp(color * mult + add, 0, 1)`, then premultiply by alpha.

```rust
fn append(&mut self, d: &CpuDraw, m: &Matrix, ct: &ColorTransform, stencil: bool) {
    let n_idx = if stencil { d.num_mask_indices as usize } else { d.indices.len() };
    let base = self.vertices.len() as u16;
    let (a, b, c, dd) = (m.a, m.b, m.c, m.d);
    let (tx, ty) = (m.tx.to_pixels() as f32, m.ty.to_pixels() as f32);
    let identity = ct.is_identity();
    let mut memo = (u32::MAX, 0u32); // shapes use few colours: cache the last one
    for (i, p) in d.positions.iter().enumerate() {
        let color = if identity {
            d.premultiplied[i]
        } else {
            let raw = d.colors[i];
            if raw != memo.0 { memo = (raw, apply_ct_premultiply(raw, ct)); }
            memo.1
        };
        self.vertices.push(BatchVertex {
            position: [a * p[0] + c * p[1] + tx, b * p[0] + dd * p[1] + ty],
            color,
        });
    }
    self.indices.extend(d.indices[..n_idx].iter().map(|&i| i + base));
    self.shapes += 1;
}
```

That is 4 multiplies and 4 adds per vertex, plus a colour lookup that is nearly always a hit. At about 20 cycles per vertex, 30,000 vertices per frame cost about 1.4 ms at 444 MHz, which is less than a hundred draws cost today. If the counters show otherwise, lower `BATCHABLE_MAX_VERTICES`. Later, a NEON loop that does 4 vertices at a time is an option.

`draw_rect` (`lib.rs:1652`, a `TRIANGLE_FAN` of the unit quad) goes into the batch too, as 4 vertices and 6 indices. `draw_line` and `draw_line_rect` use line primitives and flush.

### 1.3 Flush rules

Every GL entry point that doesn't append to the batch first calls `self.flush_batch(reason)`. `flush_batch` does nothing if the batch is empty. Otherwise it does the following, then clears the `Vec`s:

- `use_program(ProgramKind::BatchedColor)`
- uploads the vertices and indices (see 1.4)
- one `draw_elements(TRIANGLES, n, UNSIGNED_SHORT, 0)`

| Where (`ruffle_render_glow/src/lib.rs`) | What changes | `FlushReason` |
| --- | --- | --- |
| `set_stencil_state` (599), only when `mask_state_dirty` | stencil func and op, colour mask | `Mask` |
| `apply_blend_mode` (632), via `push_blend_mode` / `pop_blend_mode` (1084 / 1091) | blend equation | `Blend` |
| `render_shape` (1580), on a Gradient or Bitmap draw, or a big Color draw | program, uniforms, textures | `Program` |
| `render_bitmap` (1542) | bitmap program and texture | `Program` |
| `draw_line`, `draw_line_rect` (1656 / 1662) | line primitive | `Program` |
| `blend` (1700), `render_alpha_mask` (1706) | nested command lists and blend | `Blend` |
| `render_offscreen` (1200), `render_offscreen_tiled` (797), and any `bind_framebuffer` | render target | `Offscreen` |
| `end_frame` (1014), `submit_frame` (1286) | the frame ends | `EndFrame` |
| the append would pass `MAX_BATCH_VERTICES` | | `Full` |

**Masks.** While Ruffle draws a mask (`MaskState::DrawMaskStencil`), shapes only write stencil. Consecutive mask shapes still batch with each other: the state is the same, and the indices are the `num_mask_indices` prefix, as `render_shape` already uses. `push_mask`, `activate_mask`, `deactivate_mask` and `pop_mask` only set `mask_state_dirty`. The flush happens in `set_stencil_state` right before the next draw, which is the latest point at which it is still correct.

**Blend modes.** `Normal`, `Layer`, `Add`, `Subtract`, `Screen`, `Multiply`, `Lighten` and `Darken` are fixed-function state. A batch is valid for one blend state, and the flush happens on push and pop.

### 1.4 Uploading the stream on vitaGL

Start with standard GL so the desktop build behaves the same: a ring of three `(vbo, ibo)` pairs, and one `buffer_data(..., STREAM_DRAW)` per flush. The call re-specifies the store, so the driver can give us new memory instead of waiting on the GPU. What to check on the device:

- **Does vitaGL keep old buffer data alive until the GPU is done with it?** vitaGL defers frees of GPU memory still in use, so a `glBufferData` on a buffer the GPU is reading should get fresh memory. Check this, and look for memory growth over a long session.
- **Client-side vertex arrays** (no VBO bound, a pointer to our `Vec`) are the alternative. vitaGL copies them into its own per-frame pool, which is sized by our `vglInitWithCustomThreshold` and `vglSetVertexAttribPoolSize` settings (`src/main.rs:217-218`). They may be cheaper than `glBufferData`. Measure both.
- If both are slow, vitaGL has helpers that let us write vertices straight into GPU-mapped memory. That would be a Vita-only path behind `cfg(target_os = "vita")`.

### 1.5 Code changes, step by step

1. `register_shape_internal` (493). For `TessDrawType::Color` with at most `BATCHABLE_MAX_VERTICES` vertices, build a `CpuDraw` instead of GPU buffers: `u32` indices to `u16`, and precompute the premultiplied colours. Everything else is unchanged.
2. `Draw`, `DrawType`, `Mesh` (1836-1897). Add `Geometry`. `Mesh::drop` deletes the VAO only for `Geometry::Gpu`.
3. Add `ColorBatch`, `batched_color_program` and `ProgramKind::BatchedColor`. They are created next to the other programs in `GlowRenderBackend::new`.
4. `render_shape` (1580). `Geometry::Cpu` draws call `set_stencil_state()` (which may flush), then `self.batch.append(...)`. Everything else calls `flush_batch(Program)`, then runs the existing code.
5. Add a `flush_batch` call at the top of every row in the table in 1.3.
6. `draw_quad` (1133). `TRIANGLE_FAN` goes through the batch; the line modes flush and keep the current code.
7. Counters (section 0).
8. Behind a runtime switch, which can be an env var on desktop or a hidden setting, keep the old path. Then A/B-compare screenshots from `RUFFLEVITA_SCRIPT` `shot` runs pixel by pixel.

Expected result: on shape-heavy AS2 scenes, draws should drop from hundreds to tens. Render time should fall by the per-draw overhead minus the CPU transform cost. These are estimates; the counters will give the real numbers.

### 1.6 Next steps, in order of expected payoff

1. **Rectangular masks as scissor.** Many AS2 games mask with plain rectangles: scroll areas, HUD panels, viewports. Today every such mask costs the following, plus a stencil state change each time:
   - a stencil draw at `push_mask`,
   - the masked content,
   - a stencil clear draw at `deactivate_mask`.

   The fix:
   - At registration, mark a Color mesh that is a single axis-aligned rectangle (`rect: Option<Rectangle<f32>>`).
   - In `DrawMaskStencil`, defer the mask shapes until `activate_mask` instead of drawing them.
   - If exactly one rectangle mesh was deferred, and its matrix has `b == c == 0`, set `glScissor` to the transformed rectangle and skip the stencil entirely.
   - `deactivate_mask` / `pop_mask` restore the previous scissor. Nested masks intersect the rectangles.
   - Anything else draws the deferred shapes to stencil as today.
2. **Batch bitmaps by texture.** `render_bitmap` and `DrawType::Bitmap` draws that share a texture, filter, repeat flag and colour transform go into a second batch whose vertices carry UVs. The UVs are computed on the CPU from the texture matrix the shader uses today. This helps AS3 games that draw sprite sheets through `Bitmap` objects.
3. **Fewer uniforms on the remaining path.** Replace the `mat4 world_matrix` with the 2×3 affine already multiplied by `view_matrix` on the CPU. Two `vec3` uniforms replace 16 + 16 floats and a matrix multiply per vertex. Skip the upload when the matrix hasn't changed.
4. **`cacheAsBitmap`.** Turn on `is_offscreen_supported()` using the scratch-FBO tiling from 1.0.1. This is a large win for games that cache complex vector art, but a memory risk. It needs a per-game switch and a texture budget.

---

## 2. AVM2 interpreter fast paths

`Activation::run_actions` (`activation.rs:603`) is a `loop { let op = &opcodes[ip]; ip += 1; match op { ... } }` over Ruffle's 135-variant `Op` (16 bytes each, `op.rs:424`). Each arm calls an `op_*` method that returns `Result<(), Error>`. `Error` is boxed, so the `Result` is one register. The operand stack and locals live in a `StackFrame` of `Cell<Value>` with a `stack_pointer: Cell<usize>` (`stack.rs:94`). `Value` is 16 bytes: tag plus an 8-byte payload.

The items below are ordered by value for effort. Each one comes from the current code, not from generic interpreter advice.

### 2.1 ToInt32 / ToUint32 without `fmod` (small, every AS3 game)

`ecma_conversions.rs:32` implements ToUint32 as `n.trunc().rem_euclid(4294967296.0) as u32`. ToInt32 goes through it on everything that isn't aarch64. On the Vita, `rem_euclid` on `f64` is a libm `fmod` call into newlib's software loop. That cost is paid for:

- every `int(x)`,
- every `x | 0`,
- every bitwise op on a Number,
- every `coerce_to_i32` / `coerce_to_u32`, which covers typed `int` / `uint` slots, arguments and `CoerceI`,
- every domain-memory address.

Almost every value is already in range, so add the one-instruction path first:

```rust
pub fn f64_to_wrapping_u32(n: f64) -> u32 {
    if n > -1.0 && n < 4294967296.0 {
        return n as u32; // VCVT; truncates toward zero, NaN fails the test
    }
    if !n.is_finite() { 0 } else { n.trunc().rem_euclid(4294967296.0) as u32 }
}

fn f64_to_wrapping_i32_generic(n: f64) -> i32 {
    if n > -2147483649.0 && n < 2147483648.0 {
        return n as i32;
    }
    f64_to_wrapping_u32(n) as i32
}
```

`emulator/aot/example/test_example.c` checks the same logic in C (`rv_f64_to_i32`) against the reference definition on the edge cases: ±0, ±0.5, ±2³¹, ±2³², 2⁵³+1, ±∞, NaN.

### 2.2 Fused compare-and-branch (every loop)

`verify.rs:1137-1232` lowers every conditional branch into two ops. For example, `iflt L` becomes `LessThan` then `IfTrue { L }`. Every loop test therefore pays for:

- two dispatches,
- pushing a `Value::Bool` and popping it,
- `coerce_to_boolean`,
- a `timeout_check`,
- and `abstract_lt`, which has a fast path only for `Integer, Integer`. `Number, Number` goes through `coerce_to_primitive` twice and `coerce_to_number` twice (`value.rs:1808`).

Changes:

1. **Add a Number fast path to `abstract_lt`.** This helps every `<`, `>`, `<=`, `>=`, even unfused:
   ```rust
   (Value::Number(a), Value::Number(b)) => Ok(if a.is_nan() || b.is_nan() { None } else { Some(a < b) }),
   (Value::Integer(a), Value::Number(b)) => { let a = *a as f64; Ok(if b.is_nan() { None } else { Some(a < *b) }) }
   (Value::Number(a), Value::Integer(b)) => { let b = *b as f64; Ok(if a.is_nan() { None } else { Some(*a < b) }) }
   ```
2. **Add fused ops** `IfLt`, `IfNlt`, `IfLe`, `IfNle`, `IfGt`, `IfNgt`, `IfGe`, `IfNge`, `IfEq`, `IfNe`, `IfStrictEq` and `IfStrictNe`, each `{ offset: usize }`. The `Op` stays 16 bytes. Build them in `postprocess_peephole` (`optimizer/peephole.rs:28`): the pattern is a comparison followed by `IfTrue` / `IfFalse` where the branch is not in `jump_targets`. Rewrite the pair to `Nop` plus the fused op. `remove_nops` already runs afterwards (`optimizer.rs:50`) and fixes jump and exception offsets. The type-aware pass runs before this and needs no changes.
3. **Keep NaN semantics exact.** Each fused op must produce the same result as the pair it replaces:

   | Fused op | Jumps when | NaN |
   | --- | --- | --- |
   | `IfLt` | `lt(a, b) == Some(true)` | no jump |
   | `IfNlt` | `lt(a, b) != Some(true)` | jump |
   | `IfGe` | `lt(a, b) == Some(false)` | no jump |
   | `IfNge` | `lt(a, b) != Some(false)` | jump |
   | `IfGt` | `lt(b, a) == Some(true)` | no jump |
   | `IfNgt` | `lt(b, a) != Some(true)` | jump |
   | `IfLe` | `lt(b, a) == Some(false)` | no jump |
   | `IfNle` | `lt(b, a) != Some(false)` | jump |

In the handler, match `(Integer, Integer)` and `(Number, Number)` inline and call the generic `abstract_lt` out of line. This removes one dispatch, one push and one pop per loop iteration, and the conversions whenever the values are Numbers.

### 2.3 Timeout checks only on backward branches

`timeout_check` (`activation.rs:900`) runs on every `Jump`, `IfTrue`, `IfFalse`, `PopJump` and `LookupSwitch`. Each check is a load of `self.context`, a load of the `&mut u32`, an increment, a store and a compare. A forward branch can't form a loop, so the check is needed only when `offset <= ip`. For a `LookupSwitch`, that means when any of its targets is backward.

- **Know the direction up front.** In `postprocess_peephole`, turn backward `Jump { offset }` into `JumpBack { offset }`. Do the same for `IfTrue` / `IfFalse` and the fused ops, or add a `back: bool` field where the op has room. Only the `*Back` variants poll.
- **Keep the counter local.** Use a `let mut budget: u32` in `run_actions`, and call the real check (which reads the clock) when it reaches zero. The check at method entry stays, so deep recursion without loops is still caught.

### 2.4 Less operand-stack traffic

Each `push` / `pop` today loads `stack_pointer` from memory, bounds-checks it, moves 16 bytes and stores the pointer back. A binary op such as `Subtract` does two pops and a push: three pointer round trips and three bounds checks. LLVM can't keep the pointer in a register across arms, because every handler takes `&mut self`.

- **Update in place.** Add `StackFrame::pop2() -> (Value, Value)` and `replace_top(v)`, with one bounds check and one pointer update. `op_get_slot` already does this with `stack_top()` for exactly this reason (`activation.rs:1631`). Binary arithmetic and comparisons become one read of two slots, one write and `sp -= 1`.
- **Unchecked access in release builds.** Use `get_unchecked` with `debug_assert!`. The verifier guarantees stack depth, as the existing `// Verification guarantees` comments in `local_register` / `set_local_register` say. We tried this before and saw no gain on an M1. The M1's wide out-of-order core hides bounds checks that a 444 MHz Cortex-A9 does not, so measure again on the Vita, together with `pop2`.
- **Clear only the locals.** `Stack::get_stack_frame` (`stack.rs:50`) writes `Undefined` into all `num_locals + max_stack` slots on every call. The GC never scans the stack (`stack.rs:86` asserts it is empty during collection), and the verifier ensures operand slots are written before they are read. So only the locals past the arguments need clearing: fewer 16-byte stores on every call, which matters most for tiny methods such as getters and `b2Vec2` helpers.

### 2.5 Keep hot code small: inline fast paths, cold slow paths

The Cortex-A9 has a 32 KB L1 instruction cache. With fat LTO, `run_actions` inlines many `op_*` bodies, including their string, XML and `valueOf` branches, and grows large. The goal is that each arm holds only its common case, and everything else is one call away.

```rust
/// Binary numeric op: int and Number cases inline, everything else out of line.
macro_rules! num_binop {
    ($self:ident, |$a:ident, $b:ident| int: $int:expr, num: $num:expr, slow: $slow:path) => {{
        let ($a, $b) = $self.stack.pop2();
        let v: Value<'gc> = match ($a, $b) {
            (Value::Integer($a), Value::Integer($b)) => $int,
            (Value::Number($a), Value::Number($b)) => $num,
            _ => return $slow($self, $a, $b),
        };
        $self.stack.push(v);
        Ok(())
    }};
}

#[inline(always)]
fn op_subtract(&mut self) -> Result<(), Error<'gc>> {
    num_binop!(self, |a, b| int: (a - b).into(), num: (a - b).into(), slow: Self::subtract_slow)
}

#[cold]
#[inline(never)]
fn subtract_slow(&mut self, a: Value<'gc>, b: Value<'gc>) -> Result<(), Error<'gc>> {
    let b = b.coerce_to_number(self)?;
    let a = a.coerce_to_number(self)?;
    self.push_stack(a - b);
    Ok(())
}
```

- Use `#[inline(always)]` for push, pop, local, slot, arithmetic, comparison and branch handlers, and for `coerce_to_i32` / `coerce_to_number`, split so that their `String` / `Object` arms call a `#[cold]` function.
- Use `#[cold] #[inline(never)]` for `handle_err`, every error constructor (most `make_error_*` already are), string concatenation in `op_add`, XML, `valueOf` / `toString`, and `op_debug*`.
- Keep `avm_debug!` compiled out: check that the release build doesn't enable the `avm_debug` feature.
- Measure the size of `run_actions` in the Vita ELF (`arm-vita-eabi-nm --size-sort -C`) before and after. Watch for LLVM duplicating slow paths into many arms.
- Try `#[instruction_set(arm::a32)]` (stable Rust) on `run_actions` and the hottest handlers. The Vita target builds in Thumb-2 by default (`+thumb-mode`). A32 code is larger, but some sequences are faster on the A9. This is a measure-only experiment; keep it only if it wins.

### 2.6 Superinstructions, chosen from the histogram

Build these only once the op-pair histogram from section 0 shows the pair is common. Each goes in `postprocess_peephole` with the same guard: no jump target between the fused ops. Likely candidates for compiled AS3:

| Pattern | Fused op | Typical source |
| --- | --- | --- |
| `GetLocal { n }`, `GetSlot { s }` | `GetLocalSlot { local, slot }` | `this.x`, `p.x` |
| `GetLocal { n }`, `GetLocal { m }`, `SetSlot { s }` | `SetLocalSlot { obj, value, slot }` | `this.x = v` |
| `GetLocal`, `GetLocal`, `Add` / `Subtract` / `Multiply` / `LessThan` | `...Locals { a, b }` | `a + b` on locals |
| `IncLocalI { n }`, `Jump` | `IncLocalIJump { n, offset }` | `for (...; i++)` |
| `GetLocal { n }`, `PushByte` / `PushShort` / `PushInt`, `IfLt` | `IfLocalLtConst { n, value, offset }` | `i < 10` |

The `Op` has 12 bytes of payload to work with: two `u32` indices and a `u32` offset or an `i32` constant fit.

### 2.7 Cheaper calls

Box2D-style AS3 makes a very large number of small method calls. Per call, `function.rs:exec` and `init_from_method` (`activation.rs:308`) run the following:

- `get_stack_frame` clears the frame (see 2.4),
- `resolve_info` and `verify` (cheap after the first call, but still two checks),
- a `push_call` onto a `RefCell<Vec>`,
- `coerce_to_type` per argument.

`coerce_to_type` (`value.rs:1463`) checks up to six class identities in order before it reaches a common case.

- Precompute a `ParamKind { Any, Int, Uint, Number, Boolean, String, Object, Class(Class) }` in `ParamConfig` when the signature is resolved. Argument coercion becomes one `match`, with `Number` on a `Number` and `Int` on an `Integer` as no-ops.
- Handle `SetSlot` coercion the same way: a precomputed slot kind per class.
- Later, a direct path for `CallMethod` and `CallStatic` to a bytecode method: skip `FunctionArgs` and copy the arguments straight into the new frame's local slots. The caller's operand stack is contiguous with the new frame on the shared `Stack`.

### 2.8 Dispatch itself

A Rust `match` over 135 variants compiles to a jump table: a load of the tag, a range check, and one indirect branch shared by every op. On the A9 that shared branch predicts poorly. Stable Rust has no computed goto.

- Superinstructions (2.6) and fusion (2.2) reduce the number of dispatches, and are the cheapest way to cut dispatch cost.
- Threaded dispatch through guaranteed tail calls (`#![feature(explicit_tail_calls)]`, `become`) gives each handler its own indirect branch, the way computed goto does in C. We already build with nightly. This is a large refactor with open questions about the ARMv7 backend's `musttail` support when arguments spill to the stack, so it comes last and only if the earlier items leave dispatch at the top of the profile. The AOT path replaces dispatch completely for hot methods anyway.

### 2.9 AVM1 (AS1/AS2) note

The AVM1 interpreter (`patches/ruffle/core/src/avm1/activation.rs:412`) decodes SWF action bytes with `Reader::read_action` every time a script runs, including every iteration of every loop. A cache of pre-decoded actions per `SwfSlice` (decoded once, jump targets resolved into indices) is the AVM1 equivalent of what AVM2 already has. It helps AS2 games directly. It needs its own measurement pass with the same op histogram approach.

### 2.10 Summary

| # | Change | Where | Effort | Helps |
| --- | --- | --- | --- | --- |
| 2.1 | ToInt32 without `fmod` | `ecma_conversions.rs` | hours | all AS3, int-heavy code |
| 2.2 | Number `abstract_lt`, fused branches | `value.rs`, `op.rs`, `peephole.rs`, `activation.rs` | 1–2 days | every loop |
| 2.3 | Backward-only timeout, local budget | `peephole.rs`, `activation.rs` | 1 day | every branch |
| 2.4 | `pop2`/`replace_top`, unchecked access, locals-only clear | `stack.rs`, `activation.rs` | 1–2 days | every op, every call |
| 2.5 | Hot/cold split, A32 experiment | `activation.rs`, `value.rs` | 2–3 days | I-cache, everything |
| 2.6 | Superinstructions | `op.rs`, `peephole.rs`, `activation.rs` | per pattern | depends on histogram |
| 2.7 | `ParamKind`, direct calls | `method.rs`, `function.rs`, `activation.rs` | 2–4 days | call-heavy AS3 |
| 2.8 | Tail-call dispatch | `activation.rs` | weeks | only if still dispatch-bound |
| 2.9 | AVM1 pre-decoded actions | `avm1/` | 1 week | AS2 games |

Every item is a change to the vendored `patches/ruffle`. Keep each one a separate commit with its numbers, so it is easy to rebase onto a newer Ruffle or offer upstream.

---

## 3. The AOT C ABI

The complete header is [`emulator/aot/rv_api.h`](../emulator/aot/rv_api.h). A hand-written example of generated code is [`emulator/aot/example/example_module.c`](../emulator/aot/example/example_module.c). [`test_example.c`](../emulator/aot/example/test_example.c) runs it against a mock runtime:

```bash
cd emulator/aot/example
cc -std=c99 -Wall -Wextra -O2 test_example.c example_module.c -lm && ./a.out
```

The header compiles cleanly as C99, C11 and C++11 with clang, and with the VitaSDK's `arm-vita-eabi-gcc` 15.2. On the Vita `rv_value` is 16 bytes with 8-byte alignment.

### 3.1 Decisions

- **One C function per AS3 method**, `rv_method_fn`:
  ```c
  rv_status fn(rv_ctx *ctx, const rv_api *rt, const rv_env *env,
               const rv_value *args, uint32_t nargs, rv_value *ret);
  ```
  - `args[0]` is `this`. The runtime has already coerced the arguments and filled in the defaults, exactly as `init_from_method` does, so the C code starts from the same state as `run_actions`.
  - A method is compiled only if the translator supports every op in it. Otherwise it stays interpreted. This is always correct.
- **Status codes, not unwinding.**
  - `RV_OK`.
  - `RV_THROW`: a catchable AS3 exception is pending in the context.
  - `RV_FATAL`: timeout or internal error, propagated immediately.
  - `RV_DEOPT`: only before any side effect; the runtime interprets instead.

  `RV_TRY` / `RV_TRY_CATCH` wrap every call that can run ActionScript. A catch block calls `exc_matches(class)`, `exc_take`, and `scope_truncate` back to its saved depth, which is what `handle_err` does.
- **Own value layout.** Ruffle's `Value` is `repr(Rust)`, so `rv_value` is a separate, fixed C layout:
  - `tag`, which matches Ruffle's variants,
  - `aux`, the object kind,
  - an 8-byte union.

  The runtime converts at the boundary, which is a tag remap and a copy. `RV_INT` keeps Ruffle's 29-bit invariant (`rv_from_i32` switches to `RV_NUMBER` outside it). If profiling shows the conversion matters, Ruffle's `Value` can later be made `#[repr(C, u32)]` and asserted equal to `rv_value`.
- **Opaque handles, runtime-only heap writes.** Objects, strings, multinames, classes and methods are handles. All stores go through `rv_api`, so gc-arena's write barriers stay in Rust. Handles in C locals are safe, because Ruffle collects only between frames (`collect_debt`), never during script execution. C code must not keep handles in statics.
- **Inline what's safe, call out for the rest.** The header inlines the numeric fast paths that match Ruffle's (`rv_add`, `rv_lt`, `rv_to_f64`, `rv_to_i32`, `rv_truthy`, `rv_f64_to_i32`). Domain memory is inline too: `ctx->mem_base` and `mem_len` are kept current by the runtime, and `RV_MEM_CHECK` raises RangeError 1506. Everything else is one `rv_api` call.
- **One property call per operation.** `rv_rtname *` (NULL for a static multiname) covers the `*Static`, `*Fast` and `*Slow` op variants and all four multiname kinds.
- **Timeouts.** `RV_POLL` on backward branches decrements `ctx->poll_budget` and calls `rt->poll` when it reaches zero. This is the same scheme as 2.3.
- **Linking by ABC index.** The module lists what it needs as `(abc block, index)` pairs: strings, multinames, classes, methods, namespaces and scripts. The runtime resolves them at load exactly as the interpreter would, into `rv_env`. The module contains no runtime pointers and no NID imports.
- **Three levels of safety checks.**
  - Module load: `ruffle_rev`, `swf_hash` and each ABC block's hash must match, or the whole module is ignored.
  - Per method: `fingerprint` must match the hash of the runtime's own optimized `Op` stream and the class layouts it touched. The runtime computes it at the first call, the moment the optimizer runs today. On a mismatch, the method is interpreted.
  - `RV_DEOPT` for any extra entry guard a method needs.
- **Loading.** The prototype links generated modules into the VPK and looks them up by `swf_hash`. The final form is `game.rv.suprx` next to `game.swf`, loaded with `sceKernelLoadStartModule`. The `rv_handshake` goes in `argp`, and `module_start` (see `RV_DEFINE_SUPRX_ENTRY`) writes the module pointer back.
- **Versioning.** Major and minor version in every module, and `rv_api.size` for feature checks (`RV_API_HAS`). New entries are only ever appended.

### 3.2 What the runtime side needs (Rust)

- `extern "C"` implementations of every `rv_api` member, in a new `emulator/src/aot/` module. Each one converts `rv_value` to `Value`, calls the same Ruffle function the corresponding `op_*` handler calls, and converts back. On an `Err`, it stores the error in the `rv_ctx` private area and returns `RV_THROW` (for `Error::AvmError`) or `RV_FATAL`.
- A hook in `function.rs:exec`, `MethodKind::Bytecode` branch. After `init_from_method` (verification has run, the frame is set up), look up the method in the loaded module and compare fingerprints. On a match:
  1. build `rv_ctx` around the `Activation`,
  2. copy the locals `[0..=num_params]` (plus rest) into an `rv_value` array,
  3. call the C function instead of `run_actions`.

  On `RV_DEOPT`, fall through to `run_actions`.
- The capture tool (desktop only): after `optimizer::optimize`, dump each method's final `Op` stream, its exception table and the fingerprint, keyed by `(abc, method index)`.

### 3.3 Op-by-op mapping

**Inline C** means the translator emits plain C with no runtime call. **Inline C, then an `rv_api` entry** means an inline fast path with a call on the slow path.

**Constants and stack**

| Ops | Maps to |
| --- | --- |
| `PushDouble`, `PushInt`, `PushUint`, `PushShort`, `PushTrue`, `PushFalse`, `PushNull`, `PushUndefined` | Inline C: C constants (`rv_from_*` when boxed) |
| `PushString` | Inline C: `env->strings[i]` |
| `PushNamespace` | `push_namespace` |
| `Pop`, `Dup`, `Swap`, `Nop` | Inline C: C locals, no code |
| `GetLocal`, `SetLocal`, `StoreLocal`, `Kill` | Inline C: C locals |
| `IncLocal`, `DecLocal`, `IncLocalI`, `DecLocalI` | Inline C |

**Arithmetic and comparisons**

| Ops | Maps to |
| --- | --- |
| `AddI`, `SubtractI`, `MultiplyI`, `IncrementI`, `DecrementI`, `NegateI`, `BitAnd`, `BitOr`, `BitXor`, `BitNot`, `LShift`, `RShift`, `URShift` | Inline C, then `to_int32` / `to_uint32` for non-numbers (via `rv_to_i32`) |
| `Add` | Inline C, then `add` (via `rv_add`) |
| `Subtract`, `Multiply`, `Divide`, `Modulo`, `Negate`, `Increment`, `Decrement` | Inline C, then `to_number` (via `rv_to_f64`) |
| `LessThan`, `LessEquals`, `GreaterThan`, `GreaterEquals` | Inline C, then `lt` (via `rv_lt`) |
| `Equals` | Inline C for numeric and same-handle cases, then `eq` |
| `StrictEquals` | Inline C for tag and numeric cases, then `strict_eq` |
| `Not` | Inline C, then `to_boolean` for strings (via `rv_truthy`) |

**Conversions and type tests**

| Ops | Maps to |
| --- | --- |
| `CoerceB` | Inline C (`rv_truthy`) |
| `CoerceD`, `CoerceI`, `CoerceU` and their `*SwapPop` forms | Inline C, then `to_number` / `to_int32` / `to_uint32` |
| `CoerceA` | Inline C: nothing |
| `CoerceS` | `coerce_s` |
| `ConvertS` | `to_string` |
| `CoerceO`, `Coerce`, `CoerceSwapPop` | `coerce` |
| `ConvertO` | `convert_o` |
| `IsType`, `AsType` | `is_type`, `as_type` |
| `IsTypeLate`, `AsTypeLate`, `InstanceOf` | `is_type_late`, `as_type_late`, `instance_of` |
| `TypeOf` | `type_of` |
| `In` | `in` |

**Control flow**

| Ops | Maps to |
| --- | --- |
| `Jump`, `IfTrue`, `IfFalse`, `PopJump`, `LookupSwitch` | Inline C: `goto` / `switch`, `RV_POLL` on backward edges |
| `ReturnValue`, `ReturnVoid` | Inline C: coerce to the return type (inline for numeric types, else `coerce`), then `return RV_OK` |
| `Throw` | `throw_value` |

**Slots, properties and scope**

| Ops | Maps to |
| --- | --- |
| `GetSlot`, `SetSlot`, `SetSlotNoCoerce` | `get_slot`, `set_slot`, `set_slot_nc` (after an inline null check) |
| `SetGlobalSlot` | `set_global_slot` |
| `GetPropertyStatic`, `GetPropertyFast`, `GetPropertySlow` | `get_prop`, or `get_index` for a numeric name |
| `SetPropertyStatic`, `SetPropertyFast`, `SetPropertySlow` | `set_prop`, or `set_index` for a numeric name |
| `InitProperty`, `DeleteProperty` | `init_prop`, `delete_prop` |
| `GetSuper`, `SetSuper`, `GetDescendants` | `get_super`, `set_super`, `get_descendants` |
| `FindProperty`, `FindPropStrict`, `FindDef` | `find_prop`, `find_def` (`get_lex` for the `getlex` pattern) |
| `PushScope`, `PushWith`, `PopScope` | `push_scope`, `push_with`, `pop_scope` |
| `GetScopeObject`, `GetOuterScope`, `GetScriptGlobals` | `get_scope_object`, `get_outer_scope`, `get_script_globals` |

**Calls and construction**

| Ops | Maps to |
| --- | --- |
| `Call` | `call` |
| `CallProperty`, `CallPropLex`, `CallPropVoid` | `call_prop` with `RV_CALL_*` flags |
| `CallMethod` | `call_method` |
| `CallStatic`, `CallNative` | `call_static` (native functions are resolved through the method link) |
| `CallSuper` | `call_super` |
| `Construct`, `ConstructProp`, `ConstructSuper`, `ConstructSlot` | `construct`, `construct_prop`, `construct_super`, `construct_slot` |
| `ApplyType` | `apply_type` |
| `NewObject`, `NewArray`, `NewFunction`, `NewClass`, `NewActivation`, `NewCatch` | `new_object`, `new_array`, `new_function`, `new_class`, `new_activation`, `new_catch` |
| `HasNext`, `HasNext2`, `NextName`, `NextValue` | `has_next`, `has_next2`, `next_name`, `next_value` |

**Domain memory**

| Ops | Maps to |
| --- | --- |
| `Li8`, `Li16`, `Li32`, `Lf32`, `Lf64`, `Si8`, `Si16`, `Si32`, `Sf32`, `Sf64` | Inline C on `ctx->mem_base`, `RV_MEM_CHECK` |
| `Sxi1`, `Sxi8`, `Sxi16` | Inline C |

**E4X and debug**

| Ops | Maps to |
| --- | --- |
| `EscXAttr`, `EscXElem`, `CheckFilter`, `Dxns`, `DxnsLate` | `esc_xattr`, `esc_xelem`, `check_filter`, `dxns`, `dxns_late` |
| `Debug`, `DebugFile`, `DebugLine`, `Bkpt`, `BkptLine` | Inline C: no code |
| `Timestamp` | Not supported: the translator leaves methods that use it to the interpreter |

The same mapping is the checklist for the translator: a method is compilable exactly when every op in it is in this table.
