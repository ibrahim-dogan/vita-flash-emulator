#version 100

#ifdef GL_FRAGMENT_PRECISION_HIGH
    precision highp float;
#else
    precision mediump float;
#endif

// The colour ramp is precomputed into a 256x1 texture on the CPU. Only
// float uniforms and no uniform arrays: vitaGL's GLSL translator (and the
// Vita GPU) handle those poorly, and a texture lookup beats a 15-way branch.
uniform vec4 mult_color;
uniform vec4 add_color;
uniform sampler2D u_texture;
uniform float u_gradient_type; // 0 linear, 1 radial, 2 focal
uniform float u_repeat_mode;   // 0 pad, 1 repeat, 2 reflect
uniform float u_focal_point;

varying vec2 frag_uv;

void main() {
    float t;
    if (u_gradient_type < 0.5) {
        t = frag_uv.x;
    } else if (u_gradient_type < 1.5) {
        t = length(frag_uv * 2.0 - 1.0);
    } else {
        vec2 uv = frag_uv * 2.0 - 1.0;
        vec2 d = vec2(u_focal_point, 0.0) - uv;
        float l = max(length(d), 0.00001);
        d /= l;
        t = l / (sqrt(max(1.0 - u_focal_point * u_focal_point * d.y * d.y, 0.0)) + u_focal_point * d.x);
    }

    if (u_repeat_mode < 0.5) {
        t = clamp(t, 0.0, 1.0);
    } else if (u_repeat_mode < 1.5) {
        t = fract(t);
    } else {
        t = 1.0 - abs(mod(t, 2.0) - 1.0);
    }

    // Sample texel centres of the 256-wide ramp.
    vec4 color = texture2D(u_texture, vec2(t * (255.0 / 256.0) + (0.5 / 256.0), 0.5));
    color = clamp(mult_color * color + add_color, 0.0, 1.0);
    gl_FragColor = vec4(color.rgb * color.a, color.a);
}
