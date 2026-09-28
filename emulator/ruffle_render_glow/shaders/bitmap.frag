#version 100

#ifdef GL_FRAGMENT_PRECISION_HIGH
    precision highp float;
#else
    precision mediump float;
#endif

uniform mat4 view_matrix;
uniform mat4 world_matrix;
uniform vec4 mult_color;
uniform vec4 add_color;
uniform mat3 u_matrix;

uniform sampler2D u_texture;
// 1.0 for repeating bitmap fills. Wrapping is done here rather than with
// GL_REPEAT, which GLES2 leaves undefined (often black) for NPOT textures.
uniform float u_repeat;

varying vec2 frag_uv;

void main() {
    vec4 color = texture2D(u_texture, mix(frag_uv, fract(frag_uv), u_repeat));

    // Unmultiply alpha before apply color transform.
    if (color.a > 0.0) {
        color.rgb /= color.a;
        color = clamp(mult_color * color + add_color, 0.0, 1.0);
        float alpha = clamp(color.a, 0.0, 1.0);
        color = vec4(color.rgb * alpha, alpha);
    }

    gl_FragColor = color;
}
