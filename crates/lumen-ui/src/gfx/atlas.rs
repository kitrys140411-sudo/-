//! Glyph atlases: one coverage texture (R8) for outlines and box drawing,
//! one color texture (sRGB RGBA) for emoji. Shelf packing; when a texture is
//! full it is cleared and the caller re-uploads what the next frame needs.

use std::collections::HashMap;
use std::hash::Hash;

use super::ffi::*;

/// Where a glyph is in the atlas, and how to place it relative to the pen.
#[derive(Clone, Copy, Debug)]
pub struct Entry {
    /// Normalized texture rectangle (u0, v0, u1, v1).
    pub uv: [f32; 4],
    pub width: u32,
    pub height: u32,
    /// Bitmap offset from the pen position: right, and up from the baseline.
    pub left: f32,
    pub top: f32,
    pub color: bool,
}

pub struct Texture {
    pub id: GLuint,
    pub size: u32,
    color: bool,
    /// Shelves: (y, height, next x).
    shelves: Vec<(u32, u32, u32)>,
    next_y: u32,
}

impl Texture {
    fn new(size: u32, color: bool) -> Texture {
        let mut id = 0;
        // SAFETY: plain GL calls on the current context.
        unsafe {
            glGenTextures(1, &mut id);
            glBindTexture(GL_TEXTURE_2D, id);
            let (internal, format) = if color { (GL_SRGB8_ALPHA8, GL_RGBA) } else { (GL_R8, GL_RED) };
            glTexImage2D(GL_TEXTURE_2D, 0, internal, size as i32, size as i32, 0, format, GL_UNSIGNED_BYTE, std::ptr::null());
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
            glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
        }
        Texture { id, size, color, shelves: Vec::new(), next_y: 0 }
    }

    /// Space for a `w`×`h` bitmap (1 px gutter so linear sampling never
    /// bleeds between neighbors).
    fn alloc(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        let (w, h) = (w + 1, h + 1);
        if w > self.size || h > self.size {
            return None;
        }
        // Best fitting shelf: tallest waste at most 30%.
        let mut best: Option<usize> = None;
        for (i, &(_, sh, sx)) in self.shelves.iter().enumerate() {
            if sh >= h && sh <= h + h * 3 / 10 + 2 && sx + w <= self.size && best.is_none_or(|b| self.shelves[b].1 > sh) {
                best = Some(i);
            }
        }
        if let Some(i) = best {
            let (y, _, x) = self.shelves[i];
            self.shelves[i].2 += w;
            return Some((x, y));
        }
        if self.next_y + h > self.size {
            return None;
        }
        let y = self.next_y;
        self.shelves.push((y, h, w));
        self.next_y += h;
        Some((0, y))
    }

    fn upload(&self, x: u32, y: u32, w: u32, h: u32, data: &[u8]) {
        let format = if self.color { GL_RGBA } else { GL_RED };
        // SAFETY: data holds w*h texels of the texture's format.
        unsafe {
            glBindTexture(GL_TEXTURE_2D, self.id);
            glPixelStorei(GL_UNPACK_ALIGNMENT, 1);
            glTexSubImage2D(GL_TEXTURE_2D, 0, x as i32, y as i32, w as i32, h as i32, format, GL_UNSIGNED_BYTE, data.as_ptr().cast());
        }
    }

    fn reset(&mut self) {
        self.shelves.clear();
        self.next_y = 0;
    }
}

impl Drop for Texture {
    fn drop(&mut self) {
        // SAFETY: deleting our texture.
        unsafe { glDeleteTextures(1, &self.id) };
    }
}

/// Bitmap to place: coverage (1 byte/px) or premultiplied RGBA.
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
    pub left: f32,
    pub top: f32,
    pub color: bool,
}

pub struct Atlas<K: Hash + Eq + Clone> {
    pub mask: Texture,
    pub color: Option<Texture>,
    entries: HashMap<K, Option<Entry>>,
    size: u32,
    /// Set when a texture was reset: everything drawn must be re-resolved.
    pub generation: u64,
    /// Bitmaps uploaded so far (a steady frame of known glyphs uploads
    /// nothing; tests and perf logs watch this).
    pub uploads: u64,
}

impl<K: Hash + Eq + Clone> Atlas<K> {
    pub fn new() -> Atlas<K> {
        let mut max = 0;
        // SAFETY: query on the current context.
        unsafe { glGetIntegerv(GL_MAX_TEXTURE_SIZE, &mut max) };
        let size = (max as u32).clamp(512, 2048);
        Atlas { mask: Texture::new(size, false), color: None, entries: HashMap::new(), size, generation: 0, uploads: 0 }
    }

    /// The entry for `key`, rasterizing with `make` on a miss. None for
    /// glyphs without pixels (spaces).
    pub fn get(&mut self, key: &K, make: impl FnOnce() -> Option<Raster>) -> Option<Entry> {
        if let Some(e) = self.entries.get(key) {
            return *e;
        }
        let entry = make().and_then(|b| self.insert(&b));
        self.entries.insert(key.clone(), entry);
        entry
    }

    fn insert(&mut self, b: &Raster) -> Option<Entry> {
        let data = &b.data;
        if b.width == 0 || b.height == 0 {
            return None;
        }
        let size = self.size;
        let tex = if b.color {
            self.color.get_or_insert_with(|| Texture::new(size, true))
        } else {
            &mut self.mask
        };
        let pos = match tex.alloc(b.width, b.height) {
            Some(p) => p,
            None => {
                // Full: start over (the caller's frame re-requests its glyphs).
                tex.reset();
                self.entries.retain(|_, e| e.is_some_and(|e| e.color != b.color));
                self.generation += 1;
                let tex = if b.color { self.color.as_mut().unwrap() } else { &mut self.mask };
                tex.alloc(b.width, b.height)?
            }
        };
        let tex = if b.color { self.color.as_ref().unwrap() } else { &self.mask };
        tex.upload(pos.0, pos.1, b.width, b.height, data);
        self.uploads += 1;
        let s = tex.size as f32;
        Some(Entry {
            uv: [pos.0 as f32 / s, pos.1 as f32 / s, (pos.0 + b.width) as f32 / s, (pos.1 + b.height) as f32 / s],
            width: b.width,
            height: b.height,
            left: b.left,
            top: b.top,
            color: b.color,
        })
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.mask.reset();
        if let Some(c) = &mut self.color {
            c.reset();
        }
        self.generation += 1;
    }
}
