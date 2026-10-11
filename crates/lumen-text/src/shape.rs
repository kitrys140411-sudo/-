//! Text shaping with HarfBuzz.

use harfbuzz_sys as hb;

use crate::font::Font;

/// One shaped glyph; positions in pixels, `cluster` is a byte offset into the
/// shaped text.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapedGlyph {
    pub id: u32,
    pub cluster: usize,
    pub x_advance: f32,
    pub x_offset: f32,
    pub y_offset: f32,
}

/// An OpenType feature setting, e.g. `Feature::new(b"liga", 0)`.
#[derive(Debug, Clone, Copy)]
pub struct Feature {
    pub tag: [u8; 4],
    pub value: u32,
}

impl Feature {
    pub const fn new(tag: &[u8; 4], value: u32) -> Feature {
        Feature { tag: *tag, value }
    }
}

/// A reusable HarfBuzz buffer.
pub struct Shaper {
    buf: *mut hb::hb_buffer_t,
}

impl Default for Shaper {
    fn default() -> Self {
        Self::new()
    }
}

impl Shaper {
    pub fn new() -> Shaper {
        // SAFETY: hb_buffer_create never returns NULL (it returns the empty buffer on OOM).
        Shaper { buf: unsafe { hb::hb_buffer_create() } }
    }

    /// Shape `text` (one run: one font, one script/direction guessed from the text).
    pub fn shape(&mut self, font: &Font, text: &str, features: &[Feature]) -> Vec<ShapedGlyph> {
        if text.is_empty() {
            return Vec::new();
        }
        let feats: Vec<hb::hb_feature_t> = features
            .iter()
            .map(|f| hb::hb_feature_t { tag: u32::from_be_bytes(f.tag), value: f.value, start: 0, end: u32::MAX })
            .collect();
        // SAFETY: the buffer is valid; text pointer/length describe valid UTF-8;
        // infos/positions are read before the buffer is touched again.
        unsafe {
            hb::hb_buffer_clear_contents(self.buf);
            hb::hb_buffer_set_cluster_level(self.buf, hb::HB_BUFFER_CLUSTER_LEVEL_MONOTONE_CHARACTERS);
            hb::hb_buffer_add_utf8(self.buf, text.as_ptr().cast(), text.len() as i32, 0, text.len() as i32);
            hb::hb_buffer_guess_segment_properties(self.buf);
            hb::hb_shape(font.hb, self.buf, feats.as_ptr(), feats.len() as u32);
            let mut n = 0u32;
            let infos = hb::hb_buffer_get_glyph_infos(self.buf, &mut n);
            let mut n2 = 0u32;
            let pos = hb::hb_buffer_get_glyph_positions(self.buf, &mut n2);
            if infos.is_null() || pos.is_null() || n == 0 {
                return Vec::new();
            }
            let infos = std::slice::from_raw_parts(infos, n as usize);
            let pos = std::slice::from_raw_parts(pos, n2.min(n) as usize);
            // HarfBuzz positions follow the FreeType size in 26.6; bitmap
            // strikes are scaled like their bitmaps.
            let s = font.bitmap_scale / 64.0;
            infos
                .iter()
                .zip(pos)
                .map(|(i, p)| ShapedGlyph {
                    id: i.codepoint,
                    cluster: i.cluster as usize,
                    x_advance: p.x_advance as f32 * s,
                    x_offset: p.x_offset as f32 * s,
                    y_offset: p.y_offset as f32 * s,
                })
                .collect()
        }
    }
}

impl Drop for Shaper {
    fn drop(&mut self) {
        // SAFETY: created by hb_buffer_create.
        unsafe { hb::hb_buffer_destroy(self.buf) };
    }
}
