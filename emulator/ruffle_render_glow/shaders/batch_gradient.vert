#version 100

// RuffleVita: gradient fills batched on the CPU. Positions are in stage
// pixels, `uv` is already in gradient space, and `params` carries what used
// to be uniforms: the ramp's row in the atlas, the gradient type, the repeat
// mode and the focal point. They're the same for every vertex of a shape.

#ifdef GL_FRAGMENT_PRECISION_HIGH
    precision highp float;
#else
    precision mediump float;
#endif

uniform mat4 view_matrix;

attribute vec2 position;
attribute vec2 uv;
attribute vec4 params;
// Solid-colour shapes that join a gradient batch (type 3): the colour,
// already transformed and premultiplied.
attribute vec4 color;

varying vec2 frag_uv;
varying vec4 frag_params;
varying vec4 frag_color;

void main() {
    frag_uv = uv;
    frag_params = params;
    frag_color = color;
    gl_Position = view_matrix * vec4(position, 0.0, 1.0);
}
