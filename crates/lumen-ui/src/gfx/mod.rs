//! GPU rendering shared by Lumen's applications (the terminal and the
//! toolkit's widgets): EGL on the Wayland surface, OpenGL ES 3 instanced quads.
//!
//! Everything is drawn into a persistent sRGB canvas texture (so blending
//! happens in linear light: gamma-correct text), and only the damaged part of
//! it is redrawn each frame; the canvas is then copied to the window buffer,
//! encoding back to sRGB.
//!
//! Two ways to submit a frame: a [`Scene`] of fixed layers (rectangles, then
//! glyphs, per layer; the terminal), or an ordered [`DrawList`] that keeps
//! painter's order across rectangles, glyphs and images and carries a clip
//! rectangle per primitive (the toolkit).

pub mod atlas;
pub mod boxdraw;
pub mod ffi;
pub mod glyphs;
pub mod list;
mod shaders;

use std::collections::HashMap;
use std::ffi::{c_void, CString};

use self::atlas::Atlas;
use self::ffi::*;
use self::glyphs::GlyphKey;
use self::shaders::*;

pub use self::list::{DrawList, IRect, Item, Prim, TexRef};

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
#[derive(Clone, Copy, Debug, PartialEq)]
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
pub const KIND_BORDER: f32 = 6.0;
pub const KIND_SHADOW: f32 = 7.0;
pub const KIND_GRADIENT: f32 = 8.0;
pub const KIND_LINE: f32 = 9.0;
pub const KIND_ARC: f32 = 10.0;
pub const KIND_AREA: f32 = 11.0;

impl RectInst {
    pub fn fill(x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) -> RectInst {
        RectInst { rect: [x, y, w, h], color, params: [0.0, KIND_FILL, 0.0, 0.0] }
    }

    pub fn rounded(x: f32, y: f32, w: f32, h: f32, r: f32, color: [f32; 4]) -> RectInst {
        RectInst { rect: [x, y, w, h], color, params: [r, KIND_FILL, 0.0, 0.0] }
    }

    /// A rounded border of `width` drawn inside the rectangle.
    pub fn border(x: f32, y: f32, w: f32, h: f32, r: f32, width: f32, color: [f32; 4]) -> RectInst {
        RectInst { rect: [x, y, w, h], color, params: [r, KIND_BORDER, width, 0.0] }
    }

    /// The soft shadow of a rounded box (`x, y, w, h`), Gaussian with
    /// `sigma`; the instance covers the box grown by the blur's reach.
    pub fn shadow(x: f32, y: f32, w: f32, h: f32, r: f32, sigma: f32, color: [f32; 4]) -> RectInst {
        let reach = (sigma * 3.0).ceil();
        RectInst { rect: [x - reach, y - reach, w + 2.0 * reach, h + 2.0 * reach], color, params: [r, KIND_SHADOW, sigma, reach] }
    }

    /// A rounded fill whose alpha goes from 1 to `end` (vertically, or
    /// horizontally when `horizontal`).
    pub fn gradient(x: f32, y: f32, w: f32, h: f32, r: f32, end: f32, horizontal: bool, color: [f32; 4]) -> RectInst {
        RectInst { rect: [x, y, w, h], color, params: [r, KIND_GRADIENT, end, if horizontal { 1.0 } else { 0.0 }] }
    }

    /// A line from `a` to `b` of `width` with round caps.
    pub fn line(a: (f32, f32), b: (f32, f32), width: f32, color: [f32; 4]) -> RectInst {
        let pad = width / 2.0 + 1.0;
        let (x0, x1) = (a.0.min(b.0), a.0.max(b.0));
        let (y0, y1) = (a.1.min(b.1), a.1.max(b.1));
        // Which diagonal of the bounding box the segment is.
        let rising = (b.0 - a.0) * (b.1 - a.1) < 0.0;
        RectInst {
            rect: [x0 - pad, y0 - pad, x1 - x0 + 2.0 * pad, y1 - y0 + 2.0 * pad],
            color,
            params: [width, KIND_LINE, pad, if rising { 1.0 } else { 0.0 }],
        }
    }

    /// An arc of a ring of `thickness` inscribed in the square at (`cx`,
    /// `cy`) with outer radius `r`, from `start` radians (clockwise from 12
    /// o'clock) over `sweep` radians.
    pub fn arc(cx: f32, cy: f32, r: f32, thickness: f32, start: f32, sweep: f32, color: [f32; 4]) -> RectInst {
        RectInst { rect: [cx - r, cy - r, 2.0 * r, 2.0 * r], color, params: [thickness, KIND_ARC, start, sweep] }
    }

    /// One pixel column (`x`, from `top` down to `bottom`) of the area under
    /// a line: filled below the edge that goes from `y0` at the column's
    /// left side to `y1` at its right side (absolute y), with the alpha
    /// fading to `end` toward `bottom`. Graph fills are runs of these, so
    /// neighboring columns meet exactly at pixel boundaries (no seams).
    pub fn area_column(x: f32, top: f32, bottom: f32, y0: f32, y1: f32, end: f32, color: [f32; 4]) -> RectInst {
        RectInst { rect: [x, top, 1.0, (bottom - top).max(0.0)], color, params: [y0 - top, KIND_AREA, y1 - top, end] }
    }
}

/// Glyph instance: destination rectangle, atlas uv rectangle, color (linear
/// premultiplied; for color glyphs only its alpha is used), kind 0 = mask,
/// 1 = color.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlyphInst {
    pub rect: [f32; 4],
    pub uv: [f32; 4],
    pub color: [f32; 4],
    pub kind: f32,
}

/// Image instance: destination rectangle, texture uv rectangle, params:
/// corner radius, opacity.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageInst {
    pub rect: [f32; 4],
    pub uv: [f32; 4],
    pub params: [f32; 4],
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

/// An image texture owned by the renderer.
struct Texture {
    id: GLuint,
    size: (u32, u32),
}

/// What a batch of a draw list draws.
#[derive(Clone, Copy, PartialEq)]
enum BatchKind {
    Rect,
    Glyph,
    Image(u32),
}

/// A run of consecutive primitives of one kind with one clip: `start` and
/// `count` index that kind's instance array.
struct Batch {
    kind: BatchKind,
    clip: IRect,
    start: usize,
    count: usize,
}

pub struct Renderer {
    rect: Program,
    glyph: Program,
    image: Program,
    blit: Program,
    vao_rect: GLuint,
    vao_glyph: GLuint,
    vao_image: GLuint,
    vao_empty: GLuint,
    vbo_rect: GLuint,
    vbo_glyph: GLuint,
    vbo_image: GLuint,
    canvas_tex: GLuint,
    canvas_fbo: GLuint,
    pub canvas_size: (i32, i32),
    pub atlas: Atlas<GlyphKey>,
    textures: HashMap<u32, Texture>,
    next_texture: u32,
    // Scratch arrays for draw lists (kept to avoid reallocating every frame).
    rects: Vec<RectInst>,
    glyphs: Vec<GlyphInst>,
    images: Vec<ImageInst>,
    batches: Vec<Batch>,
}

unsafe fn attrib(index: u32, size: i32, stride: i32, offset: usize) {
    unsafe {
        glEnableVertexAttribArray(index);
        glVertexAttribPointer(index, size, GL_FLOAT, 0, stride, offset as *const c_void);
        glVertexAttribDivisor(index, 1);
    }
}

/// Point the bound VAO's instance attributes at instance `first` of the bound
/// buffer (GLES 3.0 has no base instance, so batches inside one uploaded
/// array are selected by attribute offsets).
unsafe fn rect_attribs(first: usize) {
    let rs = std::mem::size_of::<RectInst>();
    unsafe {
        attrib(0, 4, rs as i32, first * rs);
        attrib(1, 4, rs as i32, first * rs + 16);
        attrib(2, 4, rs as i32, first * rs + 32);
    }
}

unsafe fn glyph_attribs(first: usize) {
    let gs = std::mem::size_of::<GlyphInst>();
    unsafe {
        attrib(0, 4, gs as i32, first * gs);
        attrib(1, 4, gs as i32, first * gs + 16);
        attrib(2, 4, gs as i32, first * gs + 32);
        attrib(3, 1, gs as i32, first * gs + 48);
    }
}

unsafe fn image_attribs(first: usize) {
    let is = std::mem::size_of::<ImageInst>();
    unsafe {
        attrib(0, 4, is as i32, first * is);
        attrib(1, 4, is as i32, first * is + 16);
        attrib(2, 4, is as i32, first * is + 32);
    }
}

/// Upload `data` into the bound array buffer.
unsafe fn upload<T>(data: &[T]) {
    unsafe { glBufferData(GL_ARRAY_BUFFER, std::mem::size_of_val(data) as isize, data.as_ptr().cast(), GL_STREAM_DRAW) };
}

impl Renderer {
    pub fn new() -> Result<Renderer, String> {
        // SAFETY: GL calls on the current context; buffers and arrays are ours.
        unsafe {
            let rect = program(RECT_VS, RECT_FS)?;
            let glyph = program(GLYPH_VS, GLYPH_FS)?;
            let image = program(IMAGE_VS, IMAGE_FS)?;
            let blit = program(BLIT_VS, BLIT_FS)?;
            let mut vaos = [0u32; 4];
            glGenVertexArrays(4, vaos.as_mut_ptr());
            let mut vbos = [0u32; 3];
            glGenBuffers(3, vbos.as_mut_ptr());
            glBindVertexArray(vaos[0]);
            glBindBuffer(GL_ARRAY_BUFFER, vbos[0]);
            rect_attribs(0);
            glBindVertexArray(vaos[1]);
            glBindBuffer(GL_ARRAY_BUFFER, vbos[1]);
            glyph_attribs(0);
            glBindVertexArray(vaos[2]);
            glBindBuffer(GL_ARRAY_BUFFER, vbos[2]);
            image_attribs(0);
            glBindVertexArray(0);
            glUseProgram(glyph.id);
            glUniform1i(uniform(glyph.id, "u_mask"), 0);
            glUniform1i(uniform(glyph.id, "u_rgba"), 1);
            glUseProgram(image.id);
            glUniform1i(uniform(image.id, "u_tex"), 0);
            glUseProgram(blit.id);
            glUniform1i(uniform(blit.id, "u_canvas"), 0);
            let mut fbo = 0;
            glGenFramebuffers(1, &mut fbo);
            let mut tex = 0;
            glGenTextures(1, &mut tex);
            Ok(Renderer {
                rect,
                glyph,
                image,
                blit,
                vao_rect: vaos[0],
                vao_glyph: vaos[1],
                vao_image: vaos[2],
                vao_empty: vaos[3],
                vbo_rect: vbos[0],
                vbo_glyph: vbos[1],
                vbo_image: vbos[2],
                canvas_tex: tex,
                canvas_fbo: fbo,
                canvas_size: (0, 0),
                atlas: Atlas::new(),
                textures: HashMap::new(),
                next_texture: 1,
                rects: Vec::new(),
                glyphs: Vec::new(),
                images: Vec::new(),
                batches: Vec::new(),
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
                eprintln!("lumen-ui: canvas framebuffer incomplete ({status:#x})");
            }
            glBindFramebuffer(GL_FRAMEBUFFER, 0);
        }
        self.canvas_size = (w, h);
        true
    }

    /// Bind the canvas for drawing inside `clip` (top-left origin).
    unsafe fn begin_canvas(&self) {
        let (w, h) = self.canvas_size;
        unsafe {
            glBindFramebuffer(GL_FRAMEBUFFER, self.canvas_fbo);
            glViewport(0, 0, w, h);
            glEnable(GL_SCISSOR_TEST);
            glEnable(GL_BLEND);
            glBlendFunc(GL_ONE, GL_ONE_MINUS_SRC_ALPHA);
        }
    }

    unsafe fn scissor(&self, r: IRect) {
        // The canvas is drawn upside down (y-down projection into a
        // bottom-up framebuffer), so scissor in flipped coordinates.
        unsafe { glScissor(r.x, self.canvas_size.1 - r.y - r.h, r.w, r.h) };
    }

    /// Copy the canvas to the window's back buffer (whose old contents are
    /// unknown, so always all of it).
    unsafe fn present_canvas(&self) {
        let (w, h) = self.canvas_size;
        unsafe {
            glDisable(GL_BLEND);
            glDisable(GL_SCISSOR_TEST);
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

    unsafe fn bind_atlas(&self) {
        unsafe {
            glActiveTexture(GL_TEXTURE0);
            glBindTexture(GL_TEXTURE_2D, self.atlas.mask.id);
            glActiveTexture(GL_TEXTURE0 + 1);
            glBindTexture(GL_TEXTURE_2D, self.atlas.color.as_ref().map(|t| t.id).unwrap_or(self.atlas.mask.id));
            glActiveTexture(GL_TEXTURE0);
        }
    }

    /// Draw `scene` into the canvas inside `clip` (top-left origin), then
    /// copy the canvas to the window's back buffer.
    pub fn draw(&mut self, scene: &Scene, clip: Option<(i32, i32, i32, i32)>) {
        let (w, h) = self.canvas_size;
        // SAFETY: GL calls with our objects; instance data outlives the calls
        // (glBufferData copies).
        unsafe {
            if let Some((x, y, cw, ch)) = clip {
                self.begin_canvas();
                self.scissor(IRect::new(x, y, cw, ch));
                for layer in &scene.layers {
                    if !layer.rects.is_empty() {
                        glUseProgram(self.rect.id);
                        glUniform2f(self.rect.viewport, w as f32, h as f32);
                        glBindVertexArray(self.vao_rect);
                        glBindBuffer(GL_ARRAY_BUFFER, self.vbo_rect);
                        upload(&layer.rects);
                        glDrawArraysInstanced(GL_TRIANGLE_STRIP, 0, 4, layer.rects.len() as i32);
                    }
                    if !layer.glyphs.is_empty() {
                        glUseProgram(self.glyph.id);
                        glUniform2f(self.glyph.viewport, w as f32, h as f32);
                        self.bind_atlas();
                        glBindVertexArray(self.vao_glyph);
                        glBindBuffer(GL_ARRAY_BUFFER, self.vbo_glyph);
                        upload(&layer.glyphs);
                        glDrawArraysInstanced(GL_TRIANGLE_STRIP, 0, 4, layer.glyphs.len() as i32);
                    }
                }
            }
            self.present_canvas();
        }
    }

    /// Redraw the `damage` part of the canvas from `list` (cleared to
    /// transparent first), then copy the canvas to the window's back buffer.
    /// Image primitives whose texture does not exist are skipped.
    pub fn draw_list(&mut self, list: &DrawList, damage: Option<IRect>) {
        let (w, h) = self.canvas_size;
        let Some(damage) = damage.map(|d| d.intersect(&IRect::new(0, 0, w, h))).filter(|d| !d.is_empty()) else {
            // SAFETY: only the blit.
            unsafe { self.present_canvas() };
            return;
        };
        self.rects.clear();
        self.glyphs.clear();
        self.images.clear();
        self.batches.clear();
        for it in &list.items {
            let clip = it.clip.intersect(&damage);
            if clip.is_empty() {
                continue;
            }
            let (kind, start) = match it.prim {
                Prim::Rect(r) => {
                    self.rects.push(r);
                    (BatchKind::Rect, self.rects.len() - 1)
                }
                Prim::Glyph(g) => {
                    self.glyphs.push(g);
                    (BatchKind::Glyph, self.glyphs.len() - 1)
                }
                Prim::Image(t, i) => {
                    if !self.textures.contains_key(&t.id) {
                        continue;
                    }
                    self.images.push(i);
                    (BatchKind::Image(t.id), self.images.len() - 1)
                }
            };
            match self.batches.last_mut() {
                Some(b) if b.kind == kind && b.clip == clip && b.start + b.count == start => b.count += 1,
                _ => self.batches.push(Batch { kind, clip, start, count: 1 }),
            }
        }
        // SAFETY: GL calls with our objects; the instance arrays outlive the
        // calls (glBufferData copies them).
        unsafe {
            self.begin_canvas();
            self.scissor(damage);
            glClearColor(0.0, 0.0, 0.0, 0.0);
            glClear(GL_COLOR_BUFFER_BIT);
            glBindVertexArray(self.vao_rect);
            glBindBuffer(GL_ARRAY_BUFFER, self.vbo_rect);
            upload(&self.rects);
            glBindVertexArray(self.vao_glyph);
            glBindBuffer(GL_ARRAY_BUFFER, self.vbo_glyph);
            upload(&self.glyphs);
            glBindVertexArray(self.vao_image);
            glBindBuffer(GL_ARRAY_BUFFER, self.vbo_image);
            upload(&self.images);
            for p in [&self.rect, &self.glyph, &self.image] {
                glUseProgram(p.id);
                glUniform2f(p.viewport, w as f32, h as f32);
            }
            let mut current: Option<BatchKind> = None;
            for b in &self.batches {
                self.scissor(b.clip);
                match b.kind {
                    BatchKind::Rect => {
                        if current != Some(b.kind) {
                            glUseProgram(self.rect.id);
                            glBindVertexArray(self.vao_rect);
                            glBindBuffer(GL_ARRAY_BUFFER, self.vbo_rect);
                        }
                        rect_attribs(b.start);
                    }
                    BatchKind::Glyph => {
                        if current != Some(b.kind) {
                            glUseProgram(self.glyph.id);
                            self.bind_atlas();
                            glBindVertexArray(self.vao_glyph);
                            glBindBuffer(GL_ARRAY_BUFFER, self.vbo_glyph);
                        }
                        glyph_attribs(b.start);
                    }
                    BatchKind::Image(tex) => {
                        if !matches!(current, Some(BatchKind::Image(_))) {
                            glUseProgram(self.image.id);
                            glBindVertexArray(self.vao_image);
                            glBindBuffer(GL_ARRAY_BUFFER, self.vbo_image);
                        }
                        glActiveTexture(GL_TEXTURE0);
                        glBindTexture(GL_TEXTURE_2D, self.textures[&tex].id);
                        image_attribs(b.start);
                    }
                }
                current = Some(b.kind);
                glDrawArraysInstanced(GL_TRIANGLE_STRIP, 0, 4, b.count as i32);
            }
            // Leave the VAOs pointing at the start (the Scene path relies on it).
            glBindVertexArray(self.vao_rect);
            rect_attribs(0);
            glBindVertexArray(self.vao_glyph);
            glyph_attribs(0);
            glBindVertexArray(self.vao_image);
            image_attribs(0);
            self.present_canvas();
        }
    }

    /// Create an image texture from straight-alpha sRGB RGBA rows (top row
    /// first); `mipmaps` gives smooth minification (thumbnails, zoomed-out
    /// photos). Returns its id for [`TexRef`].
    pub fn create_texture(&mut self, width: u32, height: u32, rgba: &[u8], mipmaps: bool) -> u32 {
        assert!(rgba.len() >= (width * height * 4) as usize, "texture data too short");
        let mut id = 0;
        // SAFETY: data holds width*height RGBA texels.
        unsafe {
            glGenTextures(1, &mut id);
            glBindTexture(GL_TEXTURE_2D, id);
            glPixelStorei(GL_UNPACK_ALIGNMENT, 1);
            glTexImage2D(GL_TEXTURE_2D, 0, GL_SRGB8_ALPHA8, width as i32, height as i32, 0, GL_RGBA, GL_UNSIGNED_BYTE, rgba.as_ptr().cast());
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
            if mipmaps {
                glGenerateMipmap(GL_TEXTURE_2D);
                glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR_MIPMAP_LINEAR);
            } else {
                glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
            }
        }
        let handle = self.next_texture;
        self.next_texture += 1;
        self.textures.insert(handle, Texture { id, size: (width, height) });
        handle
    }

    pub fn texture_size(&self, handle: u32) -> Option<(u32, u32)> {
        self.textures.get(&handle).map(|t| t.size)
    }

    pub fn delete_texture(&mut self, handle: u32) {
        if let Some(t) = self.textures.remove(&handle) {
            // SAFETY: deleting our texture.
            unsafe { glDeleteTextures(1, &t.id) };
        }
    }

    pub fn texture_count(&self) -> usize {
        self.textures.len()
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

impl Drop for Renderer {
    fn drop(&mut self) {
        // SAFETY: deleting our objects on the (still current) context.
        unsafe {
            for t in self.textures.values() {
                glDeleteTextures(1, &t.id);
            }
            glDeleteTextures(1, &self.canvas_tex);
            glDeleteFramebuffers(1, &self.canvas_fbo);
            let vbos = [self.vbo_rect, self.vbo_glyph, self.vbo_image];
            glDeleteBuffers(3, vbos.as_ptr());
        }
    }
}
