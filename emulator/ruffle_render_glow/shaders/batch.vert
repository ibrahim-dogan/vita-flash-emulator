#version 100

// RuffleVita: solid-colour shapes batched on the CPU. Positions are already in
// stage pixels and colours already have the colour transform applied and are
// premultiplied, so all that's left is the view transform.

#ifdef GL_FRAGMENT_PRECISION_HIGH
    precision highp float;
#else
    precision mediump float;
#endif

uniform mat4 view_matrix;

attribute vec2 position;
attribute vec4 color;
varying vec4 frag_color;

void main() {
    frag_color = color;
    gl_Position = view_matrix * vec4(position, 0.0, 1.0);
}
