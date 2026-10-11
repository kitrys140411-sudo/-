//! GPU rendering: EGL on the Wayland surface, OpenGL ES 3 instanced quads.
//!
//! Everything is drawn into a persistent sRGB canvas texture (so blending
//! happens in linear light: gamma-correct text), and only the damaged part of
//! it is redrawn each frame; the canvas is then copied to the window buffer,
//! encoding back to sRGB.

pub mod atlas;
pub mod boxdraw;
pub mod ffi;
pub mod glyphs;

use std::ffi::{c_void, CString};

use self::atlas::Atlas;
use self::ffi::*;
use self::glyphs::GlyphKey;

/// sRGB (0–255) → linear (0–1).
pub fn lin(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// A color for the shaders: linear, premultiplied.
pub fn color(rgb: (u8, u8, u8), alpha: f32) -> [f32; 4] {
    [lin(rgb.0) * alpha, lin(rgb.1) * alpha, lin(rgb.2) * alpha, alpha]
}

pub struct Egl {
    dpy: EGLDisplay,
    ctx: EGLContext,
    surf: EGLSurface,
    swap_damage: Option<EglSwapWithDamage>,
}

impl Egl {
    /// # Safety
    /// `display` must be a live `wl_display*` and `window` a live
    /// `wl_egl_window*` for the lifetime of the returned object.
    pub unsafe fn new(display: *mut c_void, window: *const c_void) -> Result<Egl, String> {
        unsafe {
            let dpy = eglGetPlatformDisplay(EGL_PLATFORM_WAYLAND_KHR, display, std::ptr::null());
            if dpy.is_null() {
                return Err("eglGetPlatformDisplay failed".into());
            }
            let (mut maj, mut min) = (0, 0);
            if eglInitialize(dpy, &mut maj, &mut min) != EGL_TRUE {
                return Err(format!("eglInitialize failed ({:#x})", eglGetError()));
            }
            eglBindAPI(EGL_OPENGL_ES_API);
            let attrs = [
                EGL_SURFACE_TYPE,
                EGL_WINDOW_BIT,
                EGL_RENDERABLE_TYPE,
                EGL_OPENGL_ES3_BIT,
                EGL_RED_SIZE,
                8,
                EGL_GREEN_SIZE,
                8,
                EGL_BLUE_SIZE,
                8,
                EGL_ALPHA_SIZE,
                8,
                EGL_NONE,
            ];
            let mut config: EGLConfig = std::ptr::null_mut();
            let mut n = 0;
            if eglChooseConfig(dpy, attrs.as_ptr(), &mut config, 1, &mut n) != EGL_TRUE || n < 1 {
                return Err("no suitable EGL config (GLES 3, RGBA8)".into());
            }
            let ctx_attrs = [EGL_CONTEXT_MAJOR_VERSION, 3, EGL_CONTEXT_MINOR_VERSION, 0, EGL_NONE];
            let ctx = eglCreateContext(dpy, config, std::ptr::null_mut(), ctx_attrs.as_ptr());
            if ctx.is_null() {
                return Err(format!("eglCreateContext failed ({:#x})", eglGetError()));
            }
            let surf = eglCreatePlatformWindowSurface(dpy, config, window as *mut c_void, std::ptr::null());
            if surf.is_null() {
                return Err(format!("eglCreatePlatformWindowSurface failed ({:#x})", eglGetError()));
            }
            if eglMakeCurrent(dpy, surf, surf, ctx) != EGL_TRUE {
                return Err(format!("eglMakeCurrent failed ({:#x})", eglGetError()));
            }
            // Frames are paced by wl_surface.frame callbacks, never by blocking swaps.
            eglSwapInterval(dpy, 0);
            let exts = std::ffi::CStr::from_ptr(eglQueryString(dpy, EGL_EXTENSIONS)).to_string_lossy().into_owned();
            let swap_damage = [("EGL_KHR_swap_buffers_with_damage", "eglSwapBuffersWithDamageKHR"), ("EGL_EXT_swap_buffers_with_damage", "eglSwapBuffersWithDamageEXT")]
                .into_iter()
                .filter(|(ext, _)| exts.split(' ').any(|e| e == *ext))
                .find_map(|(_, n)| {
                    let c = CString::new(n).unwrap();
                    let p = eglGetProcAddress(c.as_ptr());
                    (!p.is_null()).then(|| std::mem::transmute::<*const c_void, EglSwapWithDamage>(p))
                });
            Ok(Egl { dpy, ctx, surf, swap_damage })
        }
    }

    pub fn renderer_name(&self) -> String {
        // SAFETY: the context is current.
        unsafe {
            let p = glGetString(GL_RENDERER);
            if p.is_null() { String::new() } else { std::ffi::CStr::from_ptr(p.cast()).to_string_lossy().into_owned() }
        }
    }

    /// Present; `damage` in buffer pixels with a top-left origin.
    pub fn swap(&self, damage: Option<(i32, i32, i32, i32)>, height: i32) -> bool {
        // SAFETY: valid display/surface; rect array of 4 ints per rect.
        unsafe {
            match (self.swap_damage, damage) {
                (Some(f), Some((x, y, w, h))) => {
                    let rect = [x, height - y - h, w, h];
                    f(self.dpy, self.surf, rect.as_ptr(), 1) == EGL_TRUE
                }
                _ => eglSwapBuffers(self.dpy, self.surf) == EGL_TRUE,
            }
        }
    }
}

impl Drop for Egl {
    fn drop(&mut self) {
        // SAFETY: tearing down what new() created.
        unsafe {
            eglMakeCurrent(self.dpy, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut());
            eglDestroySurface(self.dpy, self.surf);
            eglDestroyContext(self.dpy, self.ctx);
        }
    }
}

/// Rectangle instance: position/size in pixels (top-left origin), color
/// (linear, premultiplied), params: corner radius, kind, kind arguments.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct RectInst {
    pub rect: [f32; 4],
    pub color: [f32; 4],
    pub params: [f32; 4],
}

pub const KIND_FILL: f32 = 0.0;
pub const KIND_CURL: f32 = 1.0;
pub const KIND_DOTTED: f32 = 2.0;
pub const KIND_DASHED: f32 = 3.0;
pub const KIND_OUTLINE: f32 = 4.0;
pub const KIND_FADE_DOWN: f32 = 5.0;

impl RectInst {
    pub fn fill(x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) -> RectInst {
        RectInst { rect: [x, y, w, h], color, params: [0.0, KIND_FILL, 0.0, 0.0] }
    }

    pub fn rounded(x: f32, y: f32, w: f32, h: f32, r: f32, color: [f32; 4]) -> RectInst {
        RectInst { rect: [x, y, w, h], color, params: [r, KIND_FILL, 0.0, 0.0] }
    }
}

/// Glyph instance: destination rectangle, atlas uv rectangle, color (linear
/// premultiplied; for color glyphs only its alpha is used), kind 0 = mask,
/// 1 = color.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GlyphInst {
    pub rect: [f32; 4],
    pub uv: [f32; 4],
    pub color: [f32; 4],
    pub kind: f32,
}

/// One drawing layer: its rectangles, then its glyphs.
#[derive(Default)]
pub struct Layer {
    pub rects: Vec<RectInst>,
    pub glyphs: Vec<GlyphInst>,
}

/// Backgrounds, text, decorations, overlays (drawn in this order).
pub const LAYERS: usize = 4;

#[derive(Default)]
pub struct Scene {
    pub layers: [Layer; LAYERS],
}

impl Scene {
    pub fn clear(&mut self) {
        for l in &mut self.layers {
            l.rects.clear();
            l.glyphs.clear();
        }
    }
}

const COMMON: &str = "#version 300 es\nprecision highp float;\n";

const RECT_VS: &str = r#"
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

const RECT_FS: &str = r#"
in vec2 v_local;
flat in vec2 v_size;
flat in vec4 v_color;
flat in vec4 v_params;
out vec4 frag;
float sdf_round(vec2 p, vec2 b, float r) {
    vec2 q = abs(p) - b + r;
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - r;
}
void main() {
    float kind = v_params.y;
    float cov = 1.0;
    if (kind < 0.5) {
        if (v_params.x > 0.0) {
            float d = sdf_round(v_local - v_size * 0.5, v_size * 0.5, v_params.x);
            cov = clamp(0.5 - d, 0.0, 1.0);
        }
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
    } else {
        // Fade from the color at the top to transparent at the bottom.
        cov = 1.0 - v_local.y / v_size.y;
    }
    frag = v_color * cov;
}
"#;

const GLYPH_VS: &str = r#"
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

const GLYPH_FS: &str = r#"
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

const BLIT_VS: &str = r#"
void main() {
    vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
    gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}
"#;

const BLIT_FS: &str = r#"
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

struct Program {
    id: GLuint,
    viewport: GLint,
}

unsafe fn compile(kind: GLenum, src: &str) -> Result<GLuint, String> {
    unsafe {
        let s = glCreateShader(kind);
        let full = CString::new(format!("{COMMON}{src}")).unwrap();
        let ptr = full.as_ptr();
        glShaderSource(s, 1, &ptr, std::ptr::null());
        glCompileShader(s);
        let mut ok = 0;
        glGetShaderiv(s, GL_COMPILE_STATUS, &mut ok);
        if ok == 0 {
            let mut log = vec![0u8; 4096];
            let mut n = 0;
            glGetShaderInfoLog(s, log.len() as i32, &mut n, log.as_mut_ptr().cast());
            return Err(format!("shader: {}", String::from_utf8_lossy(&log[..n.max(0) as usize])));
        }
        Ok(s)
    }
}

unsafe fn program(vs: &str, fs: &str) -> Result<Program, String> {
    unsafe {
        let p = glCreateProgram();
        let (v, f) = (compile(GL_VERTEX_SHADER, vs)?, compile(GL_FRAGMENT_SHADER, fs)?);
        glAttachShader(p, v);
        glAttachShader(p, f);
        glLinkProgram(p);
        glDeleteShader(v);
        glDeleteShader(f);
        let mut ok = 0;
        glGetProgramiv(p, GL_LINK_STATUS, &mut ok);
        if ok == 0 {
            let mut log = vec![0u8; 4096];
            let mut n = 0;
            glGetProgramInfoLog(p, log.len() as i32, &mut n, log.as_mut_ptr().cast());
            return Err(format!("link: {}", String::from_utf8_lossy(&log[..n.max(0) as usize])));
        }
        let name = CString::new("u_viewport").unwrap();
        Ok(Program { id: p, viewport: glGetUniformLocation(p, name.as_ptr()) })
    }
}

unsafe fn uniform(p: GLuint, name: &str) -> GLint {
    let c = CString::new(name).unwrap();
    unsafe { glGetUniformLocation(p, c.as_ptr()) }
}

pub struct Renderer {
    rect: Program,
    glyph: Program,
    blit: Program,
    vao_rect: GLuint,
    vao_glyph: GLuint,
    vao_empty: GLuint,
    vbo_rect: GLuint,
    vbo_glyph: GLuint,
    canvas_tex: GLuint,
    canvas_fbo: GLuint,
    pub canvas_size: (i32, i32),
    pub atlas: Atlas<GlyphKey>,
}

unsafe fn attrib(index: u32, size: i32, stride: i32, offset: usize) {
    unsafe {
        glEnableVertexAttribArray(index);
        glVertexAttribPointer(index, size, GL_FLOAT, 0, stride, offset as *const c_void);
        glVertexAttribDivisor(index, 1);
    }
}

impl Renderer {
    pub fn new() -> Result<Renderer, String> {
        // SAFETY: GL calls on the current context; buffers and arrays are ours.
        unsafe {
            let rect = program(RECT_VS, RECT_FS)?;
            let glyph = program(GLYPH_VS, GLYPH_FS)?;
            let blit = program(BLIT_VS, BLIT_FS)?;
            let mut vaos = [0u32; 3];
            glGenVertexArrays(3, vaos.as_mut_ptr());
            let mut vbos = [0u32; 2];
            glGenBuffers(2, vbos.as_mut_ptr());
            glBindVertexArray(vaos[0]);
            glBindBuffer(GL_ARRAY_BUFFER, vbos[0]);
            let rs = std::mem::size_of::<RectInst>() as i32;
            attrib(0, 4, rs, 0);
            attrib(1, 4, rs, 16);
            attrib(2, 4, rs, 32);
            glBindVertexArray(vaos[1]);
            glBindBuffer(GL_ARRAY_BUFFER, vbos[1]);
            let gs = std::mem::size_of::<GlyphInst>() as i32;
            attrib(0, 4, gs, 0);
            attrib(1, 4, gs, 16);
            attrib(2, 4, gs, 32);
            attrib(3, 1, gs, 48);
            glBindVertexArray(0);
            glUseProgram(glyph.id);
            glUniform1i(uniform(glyph.id, "u_mask"), 0);
            glUniform1i(uniform(glyph.id, "u_rgba"), 1);
            glUseProgram(blit.id);
            glUniform1i(uniform(blit.id, "u_canvas"), 0);
            let mut fbo = 0;
            glGenFramebuffers(1, &mut fbo);
            let mut tex = 0;
            glGenTextures(1, &mut tex);
            Ok(Renderer {
                rect,
                glyph,
                blit,
                vao_rect: vaos[0],
                vao_glyph: vaos[1],
                vao_empty: vaos[2],
                vbo_rect: vbos[0],
                vbo_glyph: vbos[1],
                canvas_tex: tex,
                canvas_fbo: fbo,
                canvas_size: (0, 0),
                atlas: Atlas::new(),
            })
        }
    }

    /// Make the canvas `w`×`h`; returns true if it was (re)allocated and so
    /// holds nothing yet.
    pub fn ensure_canvas(&mut self, w: i32, h: i32) -> bool {
        if self.canvas_size == (w, h) {
            return false;
        }
        // SAFETY: (re)allocating our texture and attaching it to our FBO.
        unsafe {
            glBindTexture(GL_TEXTURE_2D, self.canvas_tex);
            glTexImage2D(GL_TEXTURE_2D, 0, GL_SRGB8_ALPHA8, w, h, 0, GL_RGBA, GL_UNSIGNED_BYTE, std::ptr::null());
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_NEAREST);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_NEAREST);
            glBindFramebuffer(GL_FRAMEBUFFER, self.canvas_fbo);
            glFramebufferTexture2D(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, self.canvas_tex, 0);
            let status = glCheckFramebufferStatus(GL_FRAMEBUFFER);
            if status != GL_FRAMEBUFFER_COMPLETE {
                eprintln!("lumen-term: canvas framebuffer incomplete ({status:#x})");
            }
            glBindFramebuffer(GL_FRAMEBUFFER, 0);
        }
        self.canvas_size = (w, h);
        true
    }

    /// Draw `scene` into the canvas inside `clip` (top-left origin), then
    /// copy the canvas to the window's back buffer.
    pub fn draw(&mut self, scene: &Scene, clip: Option<(i32, i32, i32, i32)>) {
        let (w, h) = self.canvas_size;
        // SAFETY: GL calls with our objects; instance data outlives the calls
        // (glBufferData copies).
        unsafe {
            glBindFramebuffer(GL_FRAMEBUFFER, self.canvas_fbo);
            glViewport(0, 0, w, h);
            if let Some((x, y, cw, ch)) = clip {
                glEnable(GL_SCISSOR_TEST);
                // The canvas is drawn upside down (y-down projection into a
                // bottom-up framebuffer), so scissor in flipped coordinates.
                glScissor(x, h - y - ch, cw, ch);
                glEnable(GL_BLEND);
                glBlendFunc(GL_ONE, GL_ONE_MINUS_SRC_ALPHA);
                for layer in &scene.layers {
                    if !layer.rects.is_empty() {
                        glUseProgram(self.rect.id);
                        glUniform2f(self.rect.viewport, w as f32, h as f32);
                        glBindVertexArray(self.vao_rect);
                        glBindBuffer(GL_ARRAY_BUFFER, self.vbo_rect);
                        glBufferData(
                            GL_ARRAY_BUFFER,
                            std::mem::size_of_val(layer.rects.as_slice()) as isize,
                            layer.rects.as_ptr().cast(),
                            GL_STREAM_DRAW,
                        );
                        glDrawArraysInstanced(GL_TRIANGLE_STRIP, 0, 4, layer.rects.len() as i32);
                    }
                    if !layer.glyphs.is_empty() {
                        glUseProgram(self.glyph.id);
                        glUniform2f(self.glyph.viewport, w as f32, h as f32);
                        glActiveTexture(GL_TEXTURE0);
                        glBindTexture(GL_TEXTURE_2D, self.atlas.mask.id);
                        glActiveTexture(GL_TEXTURE0 + 1);
                        glBindTexture(GL_TEXTURE_2D, self.atlas.color.as_ref().map(|t| t.id).unwrap_or(self.atlas.mask.id));
                        glActiveTexture(GL_TEXTURE0);
                        glBindVertexArray(self.vao_glyph);
                        glBindBuffer(GL_ARRAY_BUFFER, self.vbo_glyph);
                        glBufferData(
                            GL_ARRAY_BUFFER,
                            std::mem::size_of_val(layer.glyphs.as_slice()) as isize,
                            layer.glyphs.as_ptr().cast(),
                            GL_STREAM_DRAW,
                        );
                        glDrawArraysInstanced(GL_TRIANGLE_STRIP, 0, 4, layer.glyphs.len() as i32);
                    }
                }
                glDisable(GL_BLEND);
                glDisable(GL_SCISSOR_TEST);
            }
            // Copy to the window (the back buffer's old contents are unknown).
            glBindFramebuffer(GL_FRAMEBUFFER, 0);
            glViewport(0, 0, w, h);
            glUseProgram(self.blit.id);
            glActiveTexture(GL_TEXTURE0);
            glBindTexture(GL_TEXTURE_2D, self.canvas_tex);
            glBindVertexArray(self.vao_empty);
            glDrawArrays(GL_TRIANGLES, 0, 3);
            glBindVertexArray(0);
        }
    }

    /// Read the window back buffer as RGBA rows, top row first (screenshots
    /// for tests). Call after `draw`, before swapping.
    pub fn read_pixels(&self) -> Vec<u8> {
        let (w, h) = self.canvas_size;
        let mut px = vec![0u8; (w * h * 4) as usize];
        // SAFETY: buffer of w*h*4 bytes for an RGBA/UNSIGNED_BYTE read.
        unsafe {
            glBindFramebuffer(GL_FRAMEBUFFER, 0);
            glPixelStorei(GL_PACK_ALIGNMENT, 1);
            glReadPixels(0, 0, w, h, GL_RGBA, GL_UNSIGNED_BYTE, px.as_mut_ptr().cast());
        }
        let row = (w * 4) as usize;
        let mut out = vec![0u8; px.len()];
        for y in 0..h as usize {
            out[y * row..(y + 1) * row].copy_from_slice(&px[(h as usize - 1 - y) * row..(h as usize - y) * row]);
        }
        out
    }
}
