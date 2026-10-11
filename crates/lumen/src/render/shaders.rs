//! GLSL programs used on top of smithay's GLES renderer.
//!
//! Pixel shaders get `v_coords` (0..1 across the element), `size` (element size
//! in physical px) and `alpha`; all their parameters are in physical pixels. Texture shaders get `v_coords` (texture
//! coordinates) and `tex`. All sizes below are passed so that antialiasing is
//! computed in physical pixels (crisp at fractional scales).

use smithay::backend::renderer::gles::{
    GlesError, GlesPixelProgram, GlesRenderer, GlesTexProgram, UniformName, UniformType,
};

const COMMON: &str = r#"
#ifdef GL_FRAGMENT_PRECISION_HIGH
precision highp float;
#else
precision mediump float;
#endif

// Signed distance from p (relative to the box center) to a rounded box.
float sdf_round_box(vec2 p, vec2 half_size, float r) {
    r = min(r, min(half_size.x, half_size.y));
    vec2 q = abs(p) - half_size + vec2(r);
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - r;
}

// Abramowitz & Stegun 7.1.26, |error| < 1.5e-7.
float erf_approx(float x) {
    float s = sign(x);
    float a = abs(x);
    float t = 1.0 / (1.0 + 0.3275911 * a);
    float y = 1.0 - (((((1.061405429 * t - 1.453152027) * t) + 1.421413741) * t - 0.284496736) * t + 0.254829592) * t * exp(-a * a);
    return s * y;
}

// Tiny ordered noise to break up banding in smooth gradients.
float dither(vec2 frag) {
    return (fract(sin(dot(frag, vec2(12.9898, 78.233))) * 43758.5453) - 0.5) / 255.0;
}
"#;

const PIXEL_HEADER: &str = r#"
uniform float alpha;
#if defined(DEBUG_FLAGS)
uniform float tint;
#endif
uniform vec2 size;
varying vec2 v_coords;
"#;

const TEX_HEADER: &str = r#"
//_DEFINES_
#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif
"#;

const TEX_SAMPLER: &str = r#"
#if defined(EXTERNAL)
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif
uniform float alpha;
varying vec2 v_coords;
#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

vec4 sample_tex(vec2 uv) {
    vec4 c = texture2D(tex, uv);
#if defined(NO_ALPHA)
    c = vec4(c.rgb, 1.0);
#endif
    return c;
}
"#;

/// Rounded rectangle with optional vertical gradient and inner border. The box
/// is positioned with sub-pixel precision inside the (pixel-aligned) element so
/// shapes glide smoothly during animations.
const RECT: &str = r#"
uniform vec4 fill;        // premultiplied, top
uniform vec4 fill2;       // premultiplied, bottom
uniform vec4 border;      // premultiplied
uniform float border_width;
uniform float radius;
uniform vec4 box;         // x, y, w, h within the element (physical px)

void main() {
    vec2 px = v_coords * size;
    vec2 half_size = box.zw * 0.5;
    float d = sdf_round_box(px - box.xy - half_size, half_size, radius);
    float coverage = clamp(0.5 - d, 0.0, 1.0);
    vec4 c = mix(fill, fill2, clamp((px.y - box.y) / max(box.w, 1.0), 0.0, 1.0));
    if (border_width > 0.0) {
        float inner = clamp(0.5 - (d + border_width), 0.0, 1.0);
        c = mix(border, c, inner);
    }
    gl_FragColor = c * coverage * alpha;
}
"#;

/// Soft shadow of a rounded box: Gaussian edge falloff from the signed
/// distance (erf), cut out under the box so translucent windows stay clean.
const SHADOW: &str = r#"
uniform vec4 color;       // premultiplied
uniform vec4 box;         // x, y, w, h within the element (physical px)
uniform float radius;
uniform float sigma;
uniform vec4 cutout;      // box to keep clear (the window itself)

void main() {
    vec2 p = v_coords * size;
    float d = sdf_round_box(p - box.xy - box.zw * 0.5, box.zw * 0.5, radius);
    float a = 0.5 - 0.5 * erf_approx(d / (sigma * 1.41421356));
    float dc = sdf_round_box(p - cutout.xy - cutout.zw * 0.5, cutout.zw * 0.5, radius);
    a *= clamp(dc + 0.5, 0.0, 1.0);
    gl_FragColor = color * a * alpha;
}
"#;

/// Procedural wallpaper: vertical gradient with two soft colored glows.
const WALLPAPER: &str = r#"
uniform vec4 top;
uniform vec4 bottom;
uniform vec4 glow_a;
uniform vec4 glow_b;

float blob(vec2 p, vec2 c, vec2 r) {
    vec2 d = (p - c) / r;
    return exp(-dot(d, d));
}

void main() {
    vec2 uv = v_coords;
    float aspect = size.x / max(size.y, 1.0);
    vec2 p = vec2(uv.x * aspect, uv.y);
    vec3 c = mix(top.rgb, bottom.rgb, smoothstep(0.0, 1.0, uv.y));
    c += glow_a.rgb * 0.42 * blob(p, vec2(aspect * 0.28, 0.30), vec2(0.55 * aspect, 0.45));
    c += glow_b.rgb * 0.30 * blob(p, vec2(aspect * 0.82, 0.62), vec2(0.45 * aspect, 0.40));
    c += glow_a.rgb * 0.12 * blob(p, vec2(aspect * 0.55, 1.05), vec2(0.70 * aspect, 0.30));
    c += vec3(dither(gl_FragCoord.xy));
    gl_FragColor = vec4(c, 1.0) * alpha;
}
"#;

/// Window surfaces: clip to the window geometry with rounded corners.
/// `input_to_geo` maps texture coordinates to physical pixels relative to the
/// window geometry's top-left corner (built from the inverse of the renderer's
/// texture matrix, so viewport crops, buffer transforms and y-flips are exact).
const ROUNDED_SURFACE: &str = r#"
uniform vec2 geo_size;    // physical px
uniform float radius;     // physical px
uniform mat3 input_to_geo;

void main() {
    vec3 g = input_to_geo * vec3(v_coords, 1.0);
    vec2 half_size = geo_size * 0.5;
    float d = sdf_round_box(g.xy - half_size, half_size, radius);
    float coverage = clamp(0.5 - d, 0.0, 1.0);
    vec4 color = sample_tex(v_coords) * alpha * coverage;
#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif
    gl_FragColor = color;
}
"#;

/// Frosted glass: a blurred texture, saturated, tinted and clipped to a
/// rounded rectangle, with a hairline highlight at the top edge.
const GLASS: &str = r#"
uniform vec2 px_size;     // physical size of the element
uniform float radius;     // physical px
uniform vec4 tint_color;  // premultiplied
uniform float saturation;
uniform float highlight;

void main() {
    vec2 px = v_coords * px_size;
    vec2 half_size = px_size * 0.5;
    float d = sdf_round_box(px - half_size, half_size, radius);
    float coverage = clamp(0.5 - d, 0.0, 1.0);
    vec3 c = sample_tex(v_coords).rgb;
    float luma = dot(c, vec3(0.2126, 0.7152, 0.0722));
    c = mix(vec3(luma), c, saturation);
    c = c * (1.0 - tint_color.a) + tint_color.rgb;
    c += vec3(dither(gl_FragCoord.xy));
    // Inner hairline along the top edge.
    float edge = clamp(1.0 - abs(d + 1.0), 0.0, 1.0) * smoothstep(0.6, 0.0, v_coords.y);
    c += vec3(highlight * edge);
    gl_FragColor = vec4(c, 1.0) * coverage * alpha;
}
"#;

/// Dual-Kawase blur, downsample pass (one sample at center, four diagonal).
const KAWASE_DOWN: &str = r#"
uniform vec2 half_pixel;
uniform float offset;

void main() {
    vec2 uv = v_coords;
    vec2 o = half_pixel * offset;
    vec4 sum = sample_tex(uv) * 4.0;
    sum += sample_tex(uv - o);
    sum += sample_tex(uv + o);
    sum += sample_tex(uv + vec2(o.x, -o.y));
    sum += sample_tex(uv - vec2(o.x, -o.y));
    gl_FragColor = sum / 8.0;
}
"#;

/// Dual-Kawase blur, upsample pass (eight samples on a diamond).
const KAWASE_UP: &str = r#"
uniform vec2 half_pixel;
uniform float offset;

void main() {
    vec2 uv = v_coords;
    vec2 o = half_pixel * offset;
    vec4 sum = sample_tex(uv + vec2(-o.x * 2.0, 0.0));
    sum += sample_tex(uv + vec2(-o.x, o.y)) * 2.0;
    sum += sample_tex(uv + vec2(0.0, o.y * 2.0));
    sum += sample_tex(uv + vec2(o.x, o.y)) * 2.0;
    sum += sample_tex(uv + vec2(o.x * 2.0, 0.0));
    sum += sample_tex(uv + vec2(o.x, -o.y)) * 2.0;
    sum += sample_tex(uv + vec2(0.0, -o.y * 2.0));
    sum += sample_tex(uv + vec2(-o.x, -o.y)) * 2.0;
    gl_FragColor = sum / 12.0;
}
"#;

/// Plain texture clipped to a rounded rectangle (thumbnails, icons, previews).
const ROUNDED_TEXTURE: &str = r#"
uniform vec2 px_size;
uniform float radius;

void main() {
    vec2 px = v_coords * px_size;
    vec2 half_size = px_size * 0.5;
    float d = sdf_round_box(px - half_size, half_size, radius);
    gl_FragColor = sample_tex(v_coords) * alpha * clamp(0.5 - d, 0.0, 1.0);
}
"#;

pub struct Shaders {
    pub rect: GlesPixelProgram,
    pub shadow: GlesPixelProgram,
    pub wallpaper: GlesPixelProgram,
    pub rounded_surface: GlesTexProgram,
    #[allow(dead_code)] // glass panels of the shell UI (milestone 5)
    pub glass: GlesTexProgram,
    pub kawase_down: GlesTexProgram,
    pub kawase_up: GlesTexProgram,
    pub rounded_texture: GlesTexProgram,
}

fn u(name: &'static str, ty: UniformType) -> UniformName<'static> {
    UniformName::new(name, ty)
}

fn pixel(r: &mut GlesRenderer, body: &str, uniforms: &[UniformName<'_>]) -> Result<GlesPixelProgram, GlesError> {
    r.compile_custom_pixel_shader(format!("{COMMON}{PIXEL_HEADER}{body}"), uniforms)
}

fn texture(r: &mut GlesRenderer, body: &str, uniforms: &[UniformName<'_>]) -> Result<GlesTexProgram, GlesError> {
    // The `//_DEFINES_` line must come first so extension directives precede code.
    r.compile_custom_texture_shader(format!("#version 100\n{TEX_HEADER}{COMMON}{TEX_SAMPLER}{body}"), uniforms)
}

impl Shaders {
    pub fn compile(r: &mut GlesRenderer) -> Result<Shaders, GlesError> {
        use UniformType::*;
        Ok(Shaders {
            rect: pixel(
                r,
                RECT,
                &[u("fill", _4f), u("fill2", _4f), u("border", _4f), u("border_width", _1f), u("radius", _1f), u("box", _4f)],
            )?,
            shadow: pixel(r, SHADOW, &[u("color", _4f), u("box", _4f), u("radius", _1f), u("sigma", _1f), u("cutout", _4f)])?,
            wallpaper: pixel(r, WALLPAPER, &[u("top", _4f), u("bottom", _4f), u("glow_a", _4f), u("glow_b", _4f)])?,
            rounded_surface: texture(r, ROUNDED_SURFACE, &[u("geo_size", _2f), u("radius", _1f), u("input_to_geo", Matrix3x3)])?,
            glass: texture(
                r,
                GLASS,
                &[u("px_size", _2f), u("radius", _1f), u("tint_color", _4f), u("saturation", _1f), u("highlight", _1f)],
            )?,
            kawase_down: texture(r, KAWASE_DOWN, &[u("half_pixel", _2f), u("offset", _1f)])?,
            kawase_up: texture(r, KAWASE_UP, &[u("half_pixel", _2f), u("offset", _1f)])?,
            rounded_texture: texture(r, ROUNDED_TEXTURE, &[u("px_size", _2f), u("radius", _1f)])?,
        })
    }
}
