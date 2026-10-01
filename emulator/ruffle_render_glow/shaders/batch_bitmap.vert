#version 100

// RuffleVita: bitmap fills batched on the CPU. Positions are already in stage
// pixels and UVs already went through the fill's texture matrix; the colour
// transform, shared by the whole batch, is applied by bitmap.frag.

#ifdef GL_FRAGMENT_PRECISION_HIGH
    precision highp float;
#else
    precision mediump float;
#endif

uniform mat4 view_matrix;

attribute vec2 position;
attribute vec2 uv;

varying vec2 frag_uv;

void main() {
    frag_uv = uv;
    gl_Position = view_matrix * vec4(position, 0.0, 1.0);
}
