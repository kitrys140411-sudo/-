//! GLSL ES 3.0 sources of the renderer's programs. All drawing happens in
//! linear light with premultiplied colors (the canvas is an sRGB texture), and
//! every quad is generated from `gl_VertexID` with one instance per shape.

pub const COMMON: &str = "#version 300 es\nprecision highp float;\n";

pub const RECT_VS: &str = r#"
layout(location = 0) in vec4 a_rect;
layout(location = 1) in vec4 a_color;
layout(location = 2) in vec4 a_params;
uniform vec2 u_viewport;
out vec2 v_local;
flat out vec2 v_size;
flat out vec4 v_color;
flat out vec4 v_params;
void main() {
    vec2 corner = vec2(float(gl_VertexID & 1), float((gl_VertexID >> 1) & 1));
    vec2 p = a_rect.xy + corner * a_rect.zw;
    v_local = corner * a_rect.zw;
    v_size = a_rect.zw;
    v_color = a_color;
    v_params = a_params;
    vec2 ndc = p / u_viewport * 2.0 - 1.0;
    gl_Position = vec4(ndc.x, -ndc.y, 0.0, 1.0);
}
"#;

/// Shape kinds (`params.y`, see `RectInst`): fill (rounded), undercurl,
/// dotted, dashed, square outline, fade, rounded border, soft shadow,
/// gradient, line segment, arc, area column.
pub const RECT_FS: &str = r#"
in vec2 v_local;
flat in vec2 v_size;
flat in vec4 v_color;
flat in vec4 v_params;
out vec4 frag;
float sdf_round(vec2 p, vec2 b, float r) {
    vec2 q = abs(p) - b + r;
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - r;
}
// Error function approximation (max error ~5e-4), for Gaussian edges.
float erf_(float x) {
    float s = sign(x), a = abs(x);
    float t = 1.0 + (0.278393 + (0.230389 + 0.078108 * (a * a)) * a) * a;
    t *= t;
    return s - s / (t * t);
}
float rounded_cov(float r) {
    if (r <= 0.0) return 1.0;
    float d = sdf_round(v_local - v_size * 0.5, v_size * 0.5, min(r, min(v_size.x, v_size.y) * 0.5));
    return clamp(0.5 - d, 0.0, 1.0);
}
void main() {
    float kind = v_params.y;
    float cov = 1.0;
    if (kind < 0.5) {
        cov = rounded_cov(v_params.x);
    } else if (kind < 1.5) {
        // Undercurl: a sine wave of thickness z and period w.
        float amp = max(v_size.y - v_params.z, 0.0) * 0.5;
        float y = v_size.y * 0.5 + amp * sin(v_local.x / v_params.w * 6.2831853);
        cov = clamp(v_params.z * 0.5 + 0.5 - abs(v_local.y - y), 0.0, 1.0);
    } else if (kind < 2.5) {
        cov = mod(v_local.x, v_params.w) < v_params.w * 0.5 ? 1.0 : 0.0;
    } else if (kind < 3.5) {
        cov = mod(v_local.x, v_params.w) < v_params.w * 0.65 ? 1.0 : 0.0;
    } else if (kind < 4.5) {
        // Outline of width z (hollow cursor).
        vec2 d = min(v_local, v_size - v_local);
        cov = min(d.x, d.y) < v_params.z ? 1.0 : 0.0;
    } else if (kind < 5.5) {
        // Fade from the color at the top to transparent at the bottom.
        cov = 1.0 - v_local.y / v_size.y;
    } else if (kind < 6.5) {
        // Rounded border of width z: the outer shape minus the inset one.
        float r = min(v_params.x, min(v_size.x, v_size.y) * 0.5);
        float d = sdf_round(v_local - v_size * 0.5, v_size * 0.5, r);
        cov = clamp(0.5 - d, 0.0, 1.0) - clamp(0.5 - (d + v_params.z), 0.0, 1.0);
    } else if (kind < 7.5) {
        // Soft shadow: a rounded box inset by w (= the blur reach) on every
        // side, blurred with a Gaussian of sigma z (erf of the SDF).
        vec2 box = max(v_size * 0.5 - v_params.w, vec2(0.5));
        float d = sdf_round(v_local - v_size * 0.5, box, min(v_params.x, min(box.x, box.y)));
        cov = 0.5 - 0.5 * erf_(d / (max(v_params.z, 0.1) * 1.41421356));
    } else if (kind < 8.5) {
        // Rounded fill whose alpha goes from 1 to z, vertically (w = 0) or
        // horizontally (w = 1).
        float t = v_params.w < 0.5 ? v_local.y / v_size.y : v_local.x / v_size.x;
        cov = rounded_cov(v_params.x) * mix(1.0, v_params.z, clamp(t, 0.0, 1.0));
    } else if (kind < 9.5) {
        // Line segment of thickness x with round caps between two corners of
        // the quad inset by z: top-left to bottom-right (w = 0) or
        // bottom-left to top-right (w = 1).
        float pad = v_params.z;
        vec2 a = v_params.w < 0.5 ? vec2(pad, pad) : vec2(pad, v_size.y - pad);
        vec2 b = v_params.w < 0.5 ? v_size - pad : vec2(v_size.x - pad, pad);
        vec2 ab = b - a;
        float h = clamp(dot(v_local - a, ab) / max(dot(ab, ab), 1e-6), 0.0, 1.0);
        float d = length(v_local - a - ab * h) - v_params.x * 0.5;
        cov = clamp(0.5 - d, 0.0, 1.0);
    } else if (kind > 10.5) {
        // Area column (one pixel wide): filled below the edge from local y
        // = x at the left side to z at the right side; the coverage of this
        // pixel row is the edge's average over four positions across the
        // column. The alpha fades from 1 at the top of the quad to w at its
        // bottom (graph fills).
        float c = 0.0;
        for (int i = 0; i < 4; i++) {
            float ey = mix(v_params.x, v_params.z, (float(i) + 0.5) * 0.25);
            c += clamp(v_local.y + 0.5 - ey, 0.0, 1.0);
        }
        cov = c * 0.25 * mix(1.0, v_params.w, clamp(v_local.y / max(v_size.y, 1.0), 0.0, 1.0));
    } else {
        // Arc of a ring of thickness x inside the quad, starting at angle z
        // (radians, clockwise from 12 o'clock) and sweeping w radians.
        vec2 c = v_size * 0.5;
        float rad = min(c.x, c.y) - v_params.x * 0.5;
        vec2 p = v_local - c;
        float ring = abs(length(p) - rad) - v_params.x * 0.5;
        float ang = atan(p.x, -p.y);
        float rel = mod(ang - v_params.z, 6.2831853);
        float sweep = min(v_params.w, 6.2831853);
        // Distance (in pixels) outside the angular range, for soft ends.
        float outside = rel <= sweep ? 0.0 : min(rel - sweep, 6.2831853 - rel) * rad;
        cov = clamp(0.5 - ring, 0.0, 1.0) * clamp(1.0 - outside, 0.0, 1.0);
    }
    frag = v_color * cov;
}
"#;

pub const GLYPH_VS: &str = r#"
layout(location = 0) in vec4 a_rect;
layout(location = 1) in vec4 a_uv;
layout(location = 2) in vec4 a_color;
layout(location = 3) in float a_kind;
uniform vec2 u_viewport;
out vec2 v_uv;
flat out vec4 v_color;
flat out float v_kind;
void main() {
    vec2 corner = vec2(float(gl_VertexID & 1), float((gl_VertexID >> 1) & 1));
    vec2 p = a_rect.xy + corner * a_rect.zw;
    v_uv = mix(a_uv.xy, a_uv.zw, corner);
    v_color = a_color;
    v_kind = a_kind;
    vec2 ndc = p / u_viewport * 2.0 - 1.0;
    gl_Position = vec4(ndc.x, -ndc.y, 0.0, 1.0);
}
"#;

pub const GLYPH_FS: &str = r#"
in vec2 v_uv;
flat in vec4 v_color;
flat in float v_kind;
uniform sampler2D u_mask;
uniform sampler2D u_rgba;
out vec4 frag;
void main() {
    if (v_kind < 0.5) {
        float a = texture(u_mask, v_uv).r;
        // Blending in linear light makes light text on dark look heavier and
        // dark text on light thinner; compensate a little by luminance.
        vec3 c = v_color.rgb / max(v_color.a, 1e-4);
        float lum = dot(c, vec3(0.2126, 0.7152, 0.0722));
        a = pow(a, mix(0.82, 1.22, sqrt(lum)));
        frag = v_color * a;
    } else {
        frag = texture(u_rgba, v_uv) * v_color.a;
    }
}
"#;

pub const IMAGE_VS: &str = r#"
layout(location = 0) in vec4 a_rect;
layout(location = 1) in vec4 a_uv;
layout(location = 2) in vec4 a_params;
uniform vec2 u_viewport;
out vec2 v_uv;
out vec2 v_local;
flat out vec2 v_size;
flat out vec4 v_params;
void main() {
    vec2 corner = vec2(float(gl_VertexID & 1), float((gl_VertexID >> 1) & 1));
    vec2 p = a_rect.xy + corner * a_rect.zw;
    v_uv = mix(a_uv.xy, a_uv.zw, corner);
    v_local = corner * a_rect.zw;
    v_size = a_rect.zw;
    v_params = a_params;
    vec2 ndc = p / u_viewport * 2.0 - 1.0;
    gl_Position = vec4(ndc.x, -ndc.y, 0.0, 1.0);
}
"#;

/// Images are straight-alpha sRGB textures (sampled as linear): premultiply,
/// apply the opacity (`params.y`) and the rounded clip (`params.x`).
pub const IMAGE_FS: &str = r#"
in vec2 v_uv;
in vec2 v_local;
flat in vec2 v_size;
flat in vec4 v_params;
uniform sampler2D u_tex;
out vec4 frag;
float sdf_round(vec2 p, vec2 b, float r) {
    vec2 q = abs(p) - b + r;
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - r;
}
void main() {
    vec4 c = texture(u_tex, v_uv);
    float cov = 1.0;
    if (v_params.x > 0.0) {
        float d = sdf_round(v_local - v_size * 0.5, v_size * 0.5, min(v_params.x, min(v_size.x, v_size.y) * 0.5));
        cov = clamp(0.5 - d, 0.0, 1.0);
    }
    frag = vec4(c.rgb * c.a, c.a) * (v_params.y * cov);
}
"#;

pub const BLIT_VS: &str = r#"
void main() {
    vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
    gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}
"#;

pub const BLIT_FS: &str = r#"
uniform sampler2D u_canvas;
out vec4 frag;
vec3 to_srgb(vec3 c) {
    return mix(c * 12.92, 1.055 * pow(c, vec3(1.0 / 2.4)) - 0.055, step(0.0031308, c));
}
void main() {
    vec4 c = texelFetch(u_canvas, ivec2(gl_FragCoord.xy), 0);
    frag = vec4(to_srgb(c.rgb), 1.0);
}
"#;
