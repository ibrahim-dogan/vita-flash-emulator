#version 100

#ifdef GL_FRAGMENT_PRECISION_HIGH
    precision highp float;
#else
    precision mediump float;
#endif

// gradient.frag, with the per-gradient values coming from the vertices
// (frag_params = atlas row v, type, repeat mode, focal point) so that
// consecutive gradient fills can share one draw call.
uniform vec4 mult_color;
uniform vec4 add_color;
uniform sampler2D u_texture;

varying vec2 frag_uv;
varying vec4 frag_params;
varying vec4 frag_color;

void main() {
    float gradient_type = frag_params.y; // 0 linear, 1 radial, 2 focal, 3 solid
    if (gradient_type > 2.5) {
        gl_FragColor = frag_color;
        return;
    }
    float repeat_mode = frag_params.z;   // 0 pad, 1 repeat, 2 reflect
    float focal_point = frag_params.w;

    float t;
    if (gradient_type < 0.5) {
        t = frag_uv.x;
    } else if (gradient_type < 1.5) {
        t = length(frag_uv * 2.0 - 1.0);
    } else {
        vec2 uv = frag_uv * 2.0 - 1.0;
        vec2 d = vec2(focal_point, 0.0) - uv;
        float l = max(length(d), 0.00001);
        d /= l;
        t = l / (sqrt(max(1.0 - focal_point * focal_point * d.y * d.y, 0.0)) + focal_point * d.x);
    }

    if (repeat_mode < 0.5) {
        t = clamp(t, 0.0, 1.0);
    } else if (repeat_mode < 1.5) {
        t = fract(t);
    } else {
        t = 1.0 - abs(mod(t, 2.0) - 1.0);
    }

    // Sample texel centres of the 256-wide ramp in its atlas row.
    vec4 color = texture2D(u_texture, vec2(t * (255.0 / 256.0) + (0.5 / 256.0), frag_params.x));
    color = clamp(mult_color * color + add_color, 0.0, 1.0);
    gl_FragColor = vec4(color.rgb * color.a, color.a);
}
