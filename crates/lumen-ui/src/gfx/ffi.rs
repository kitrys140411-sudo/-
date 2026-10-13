//! Minimal EGL and OpenGL ES 3.0 bindings (only what the renderer uses),
//! linked against Mesa's libEGL and libGLESv2.

#![allow(non_snake_case, non_upper_case_globals, dead_code)]

use std::ffi::{c_char, c_void};

pub type EGLDisplay = *mut c_void;
pub type EGLConfig = *mut c_void;
pub type EGLContext = *mut c_void;
pub type EGLSurface = *mut c_void;
pub type EGLint = i32;
pub type EGLBoolean = u32;
pub type EGLenum = u32;
pub type EGLAttrib = isize;

pub const EGL_NONE: EGLint = 0x3038;
pub const EGL_TRUE: EGLBoolean = 1;
pub const EGL_RED_SIZE: EGLint = 0x3024;
pub const EGL_GREEN_SIZE: EGLint = 0x3023;
pub const EGL_BLUE_SIZE: EGLint = 0x3022;
pub const EGL_ALPHA_SIZE: EGLint = 0x3021;
pub const EGL_SURFACE_TYPE: EGLint = 0x3033;
pub const EGL_WINDOW_BIT: EGLint = 0x0004;
pub const EGL_RENDERABLE_TYPE: EGLint = 0x3040;
pub const EGL_OPENGL_ES3_BIT: EGLint = 0x0040;
pub const EGL_OPENGL_ES_API: EGLenum = 0x30A0;
pub const EGL_CONTEXT_MAJOR_VERSION: EGLint = 0x3098;
pub const EGL_CONTEXT_MINOR_VERSION: EGLint = 0x30FB;
pub const EGL_PLATFORM_WAYLAND_KHR: EGLenum = 0x31D8;
pub const EGL_EXTENSIONS: EGLint = 0x3055;
pub const EGL_VENDOR: EGLint = 0x3053;

pub type EglSwapWithDamage = unsafe extern "C" fn(EGLDisplay, EGLSurface, *const EGLint, EGLint) -> EGLBoolean;

#[link(name = "EGL")]
unsafe extern "C" {
    pub fn eglGetPlatformDisplay(platform: EGLenum, native_display: *mut c_void, attrib_list: *const EGLAttrib) -> EGLDisplay;
    pub fn eglInitialize(dpy: EGLDisplay, major: *mut EGLint, minor: *mut EGLint) -> EGLBoolean;
    pub fn eglTerminate(dpy: EGLDisplay) -> EGLBoolean;
    pub fn eglBindAPI(api: EGLenum) -> EGLBoolean;
    pub fn eglChooseConfig(dpy: EGLDisplay, attrib_list: *const EGLint, configs: *mut EGLConfig, config_size: EGLint, num_config: *mut EGLint) -> EGLBoolean;
    pub fn eglCreateContext(dpy: EGLDisplay, config: EGLConfig, share: EGLContext, attrib_list: *const EGLint) -> EGLContext;
    pub fn eglDestroyContext(dpy: EGLDisplay, ctx: EGLContext) -> EGLBoolean;
    pub fn eglCreatePlatformWindowSurface(dpy: EGLDisplay, config: EGLConfig, native_window: *mut c_void, attrib_list: *const EGLAttrib) -> EGLSurface;
    pub fn eglDestroySurface(dpy: EGLDisplay, surface: EGLSurface) -> EGLBoolean;
    pub fn eglMakeCurrent(dpy: EGLDisplay, draw: EGLSurface, read: EGLSurface, ctx: EGLContext) -> EGLBoolean;
    pub fn eglSwapBuffers(dpy: EGLDisplay, surface: EGLSurface) -> EGLBoolean;
    pub fn eglSwapInterval(dpy: EGLDisplay, interval: EGLint) -> EGLBoolean;
    pub fn eglGetError() -> EGLint;
    pub fn eglGetProcAddress(name: *const c_char) -> *const c_void;
    pub fn eglQueryString(dpy: EGLDisplay, name: EGLint) -> *const c_char;
}

pub type GLenum = u32;
pub type GLuint = u32;
pub type GLint = i32;
pub type GLsizei = i32;
pub type GLfloat = f32;
pub type GLboolean = u8;
pub type GLbitfield = u32;
pub type GLsizeiptr = isize;
pub type GLintptr = isize;

pub const GL_NO_ERROR: GLenum = 0;
pub const GL_TEXTURE_2D: GLenum = 0x0DE1;
pub const GL_TEXTURE0: GLenum = 0x84C0;
pub const GL_R8: GLint = 0x8229;
pub const GL_RED: GLenum = 0x1903;
pub const GL_RGBA: GLenum = 0x1908;
pub const GL_RGBA8: GLint = 0x8058;
pub const GL_SRGB8_ALPHA8: GLint = 0x8C43;
pub const GL_UNSIGNED_BYTE: GLenum = 0x1401;
pub const GL_FLOAT: GLenum = 0x1406;
pub const GL_TEXTURE_MIN_FILTER: GLenum = 0x2801;
pub const GL_TEXTURE_MAG_FILTER: GLenum = 0x2800;
pub const GL_TEXTURE_WRAP_S: GLenum = 0x2802;
pub const GL_TEXTURE_WRAP_T: GLenum = 0x2803;
pub const GL_LINEAR: GLint = 0x2601;
pub const GL_NEAREST: GLint = 0x2600;
pub const GL_LINEAR_MIPMAP_LINEAR: GLint = 0x2703;
pub const GL_CLAMP_TO_EDGE: GLint = 0x812F;
pub const GL_UNPACK_ALIGNMENT: GLenum = 0x0CF5;
pub const GL_PACK_ALIGNMENT: GLenum = 0x0D05;
pub const GL_FRAMEBUFFER: GLenum = 0x8D40;
pub const GL_READ_FRAMEBUFFER: GLenum = 0x8CA8;
pub const GL_COLOR_ATTACHMENT0: GLenum = 0x8CE0;
pub const GL_FRAMEBUFFER_COMPLETE: GLenum = 0x8CD5;
pub const GL_VERTEX_SHADER: GLenum = 0x8B31;
pub const GL_FRAGMENT_SHADER: GLenum = 0x8B30;
pub const GL_COMPILE_STATUS: GLenum = 0x8B81;
pub const GL_LINK_STATUS: GLenum = 0x8B82;
pub const GL_INFO_LOG_LENGTH: GLenum = 0x8B84;
pub const GL_ARRAY_BUFFER: GLenum = 0x8892;
pub const GL_STREAM_DRAW: GLenum = 0x88E0;
pub const GL_TRIANGLE_STRIP: GLenum = 0x0005;
pub const GL_TRIANGLES: GLenum = 0x0004;
pub const GL_BLEND: GLenum = 0x0BE2;
pub const GL_SCISSOR_TEST: GLenum = 0x0C11;
pub const GL_ONE: GLenum = 1;
pub const GL_ONE_MINUS_SRC_ALPHA: GLenum = 0x0303;
pub const GL_COLOR_BUFFER_BIT: GLbitfield = 0x4000;
pub const GL_MAX_TEXTURE_SIZE: GLenum = 0x0D33;
pub const GL_RENDERER: GLenum = 0x1F01;

#[link(name = "GLESv2")]
unsafe extern "C" {
    pub fn glGetError() -> GLenum;
    pub fn glGetString(name: GLenum) -> *const u8;
    pub fn glGetIntegerv(pname: GLenum, data: *mut GLint);
    pub fn glViewport(x: GLint, y: GLint, w: GLsizei, h: GLsizei);
    pub fn glScissor(x: GLint, y: GLint, w: GLsizei, h: GLsizei);
    pub fn glClearColor(r: GLfloat, g: GLfloat, b: GLfloat, a: GLfloat);
    pub fn glClear(mask: GLbitfield);
    pub fn glEnable(cap: GLenum);
    pub fn glDisable(cap: GLenum);
    pub fn glBlendFunc(s: GLenum, d: GLenum);
    pub fn glGenTextures(n: GLsizei, textures: *mut GLuint);
    pub fn glDeleteTextures(n: GLsizei, textures: *const GLuint);
    pub fn glBindTexture(target: GLenum, texture: GLuint);
    pub fn glActiveTexture(texture: GLenum);
    pub fn glTexImage2D(target: GLenum, level: GLint, internal: GLint, w: GLsizei, h: GLsizei, border: GLint, format: GLenum, ty: GLenum, data: *const c_void);
    pub fn glTexSubImage2D(target: GLenum, level: GLint, x: GLint, y: GLint, w: GLsizei, h: GLsizei, format: GLenum, ty: GLenum, data: *const c_void);
    pub fn glTexParameteri(target: GLenum, pname: GLenum, param: GLint);
    pub fn glPixelStorei(pname: GLenum, param: GLint);
    pub fn glGenerateMipmap(target: GLenum);
    pub fn glGenFramebuffers(n: GLsizei, fbs: *mut GLuint);
    pub fn glDeleteFramebuffers(n: GLsizei, fbs: *const GLuint);
    pub fn glBindFramebuffer(target: GLenum, fb: GLuint);
    pub fn glFramebufferTexture2D(target: GLenum, attachment: GLenum, textarget: GLenum, texture: GLuint, level: GLint);
    pub fn glCheckFramebufferStatus(target: GLenum) -> GLenum;
    pub fn glCreateShader(ty: GLenum) -> GLuint;
    pub fn glDeleteShader(s: GLuint);
    pub fn glShaderSource(s: GLuint, count: GLsizei, strings: *const *const c_char, lengths: *const GLint);
    pub fn glCompileShader(s: GLuint);
    pub fn glGetShaderiv(s: GLuint, pname: GLenum, out: *mut GLint);
    pub fn glGetShaderInfoLog(s: GLuint, max: GLsizei, len: *mut GLsizei, log: *mut c_char);
    pub fn glCreateProgram() -> GLuint;
    pub fn glAttachShader(p: GLuint, s: GLuint);
    pub fn glLinkProgram(p: GLuint);
    pub fn glGetProgramiv(p: GLuint, pname: GLenum, out: *mut GLint);
    pub fn glGetProgramInfoLog(p: GLuint, max: GLsizei, len: *mut GLsizei, log: *mut c_char);
    pub fn glUseProgram(p: GLuint);
    pub fn glGetUniformLocation(p: GLuint, name: *const c_char) -> GLint;
    pub fn glUniform1i(loc: GLint, v: GLint);
    pub fn glUniform1f(loc: GLint, v: GLfloat);
    pub fn glUniform2f(loc: GLint, a: GLfloat, b: GLfloat);
    pub fn glUniform4f(loc: GLint, a: GLfloat, b: GLfloat, c: GLfloat, d: GLfloat);
    pub fn glGenBuffers(n: GLsizei, b: *mut GLuint);
    pub fn glDeleteBuffers(n: GLsizei, b: *const GLuint);
    pub fn glBindBuffer(target: GLenum, b: GLuint);
    pub fn glBufferData(target: GLenum, size: GLsizeiptr, data: *const c_void, usage: GLenum);
    pub fn glGenVertexArrays(n: GLsizei, a: *mut GLuint);
    pub fn glBindVertexArray(a: GLuint);
    pub fn glEnableVertexAttribArray(i: GLuint);
    pub fn glVertexAttribPointer(i: GLuint, size: GLint, ty: GLenum, normalized: GLboolean, stride: GLsizei, ptr: *const c_void);
    pub fn glVertexAttribDivisor(i: GLuint, divisor: GLuint);
    pub fn glDrawArrays(mode: GLenum, first: GLint, count: GLsizei);
    pub fn glDrawArraysInstanced(mode: GLenum, first: GLint, count: GLsizei, instances: GLsizei);
    pub fn glReadPixels(x: GLint, y: GLint, w: GLsizei, h: GLsizei, format: GLenum, ty: GLenum, data: *mut c_void);
    pub fn glFinish();
    pub fn glFlush();
}
