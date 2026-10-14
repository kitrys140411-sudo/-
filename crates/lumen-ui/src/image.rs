//! Images for apps.
//!
//! [`Pixels`] are decoded RGBA pixels: plain data that background threads
//! produce ([`Pixels::load`] decodes PNG, JPEG, WebP, GIF, BMP and ICO files,
//! turned upright by their EXIF orientation; [`Pixels::thumbnail`] shrinks
//! them). An [`Image`] is the UI thread's handle to pixels for painting:
//! uploaded to a GPU texture the first time it is painted, freed when the
//! last handle is dropped.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::Path;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::gfx::{Renderer, TexRef};

/// File name extensions [`Pixels::load`] decodes (lower case).
pub const IMAGE_EXTENSIONS: [&str; 7] = ["png", "jpg", "jpeg", "gif", "webp", "bmp", "ico"];

/// Whether the file name has an extension [`Pixels::load`] decodes.
pub fn is_image_file(name: impl AsRef<Path>) -> bool {
    name.as_ref().extension().and_then(|e| e.to_str()).is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Decoded pixels: `width` × `height` straight-alpha sRGB RGBA, top row
/// first. Unlike [`Image`] they can be sent between threads, so decoding
/// and scaling happen off the UI thread:
///
/// ```no_run
/// # use lumen_ui::{Pixels, Image, Ui, Task};
/// # fn f(ui: &mut Ui, task: &mut Option<Task<Result<Pixels, String>>>, shown: &mut Option<Image>) {
/// // Start decoding once...
/// if task.is_none() {
///     *task = Some(ui.spawn(|| Pixels::load("/home/me/photo.jpg")));
/// }
/// // ...and show the picture when it is ready.
/// if let Some(Ok(p)) = task.as_mut().and_then(|t| t.take()) {
///     *shown = Some(Image::from_pixels(p));
/// }
/// # }
/// ```
#[derive(Clone, PartialEq, Eq)]
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl std::fmt::Debug for Pixels {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Pixels({}x{})", self.width, self.height)
    }
}

impl Pixels {
    /// Pixels from RGBA data (`width * height * 4` bytes).
    pub fn new(width: u32, height: u32, data: Vec<u8>) -> Pixels {
        assert_eq!(data.len(), width as usize * height as usize * 4, "RGBA data must be width*height*4 bytes");
        Pixels { width, height, data }
    }

    /// Decode an image file's contents (the format is detected from the
    /// data); JPEG and WebP photos are turned upright by their EXIF
    /// orientation, animated GIFs give their first frame.
    pub fn decode(bytes: &[u8]) -> Result<Pixels, String> {
        use ::image::ImageDecoder;
        let reader = ::image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().map_err(|e| e.to_string())?;
        if reader.format().is_none() {
            return Err("not an image file (or an unsupported format)".into());
        }
        let mut decoder = reader.into_decoder().map_err(|e| e.to_string())?;
        let orientation = decoder.orientation().unwrap_or(::image::metadata::Orientation::NoTransforms);
        let mut img = ::image::DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
        img.apply_orientation(orientation);
        let rgba = img.into_rgba8();
        let (w, h) = rgba.dimensions();
        Ok(Pixels::new(w, h, rgba.into_raw()))
    }

    /// Read and decode an image file (see [`Pixels::decode`]).
    pub fn load(path: impl AsRef<Path>) -> Result<Pixels, String> {
        let path = path.as_ref();
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Pixels::decode(&bytes).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Width and height of an image file without decoding its pixels (the
    /// stored size, before any EXIF rotation).
    pub fn dimensions(path: impl AsRef<Path>) -> Result<(u32, u32), String> {
        let path = path.as_ref();
        let reader = ::image::ImageReader::open(path).and_then(|r| r.with_guessed_format()).map_err(|e| format!("{}: {e}", path.display()))?;
        reader.into_dimensions().map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Decode a file and shrink it to fit `max` × `max` (file manager
    /// thumbnails).
    pub fn load_thumbnail(path: impl AsRef<Path>, max: u32) -> Result<Pixels, String> {
        Ok(Pixels::load(path)?.thumbnail(max, max))
    }

    /// A copy shrunk (never enlarged) to fit `max_w` × `max_h`, keeping the
    /// aspect ratio. Each output pixel averages the source pixels it covers
    /// (with premultiplied alpha), so detail does not alias.
    pub fn thumbnail(&self, max_w: u32, max_h: u32) -> Pixels {
        let k = (max_w as f64 / self.width.max(1) as f64).min(max_h as f64 / self.height.max(1) as f64);
        if k >= 1.0 {
            return self.clone();
        }
        let w = ((self.width as f64 * k).round() as u32).max(1);
        let h = ((self.height as f64 * k).round() as u32).max(1);
        self.resized_down(w, h)
    }

    /// Area-averaging downscale to exactly `w` × `h` (both at most the
    /// current size).
    fn resized_down(&self, w: u32, h: u32) -> Pixels {
        let (sw, sh) = (self.width as usize, self.height as usize);
        let (w, h) = (w.min(self.width).max(1) as usize, h.min(self.height).max(1) as usize);
        // Source spans of each output column/row: [start, end) in source
        // pixels as fractions, so every source pixel contributes exactly once.
        let spans = |src: usize, dst: usize| -> Vec<(usize, usize, f32, f32)> {
            let scale = src as f64 / dst as f64;
            (0..dst)
                .map(|i| {
                    let (a, b) = (i as f64 * scale, (i + 1) as f64 * scale);
                    let (i0, i1) = (a.floor() as usize, (b.ceil() as usize).min(src));
                    let first = 1.0 - (a - i0 as f64);
                    let last = 1.0 - (i1 as f64 - b);
                    (i0, i1, first as f32, last as f32)
                })
                .collect()
        };
        let weight = |j: usize, (i0, i1, first, last): (usize, usize, f32, f32)| -> f32 {
            let mut wt = 1.0;
            if j == i0 {
                wt *= first;
            }
            if j + 1 == i1 {
                wt *= last;
            }
            wt
        };
        let xs = spans(sw, w);
        let ys = spans(sh, h);
        // Horizontal pass into premultiplied float rows, then vertical.
        let mut tmp = vec![0f32; w * sh * 4];
        for y in 0..sh {
            let row = &self.data[y * sw * 4..(y + 1) * sw * 4];
            for (x, &span) in xs.iter().enumerate() {
                let mut acc = [0f32; 4];
                let mut total = 0.0;
                for sx in span.0..span.1 {
                    let wt = weight(sx, span);
                    let p = &row[sx * 4..sx * 4 + 4];
                    let a = p[3] as f32 / 255.0;
                    acc[0] += p[0] as f32 * a * wt;
                    acc[1] += p[1] as f32 * a * wt;
                    acc[2] += p[2] as f32 * a * wt;
                    acc[3] += a * wt;
                    total += wt;
                }
                let o = (y * w + x) * 4;
                for c in 0..4 {
                    tmp[o + c] = acc[c] / total.max(1e-6);
                }
            }
        }
        let mut out = vec![0u8; w * h * 4];
        for (y, &span) in ys.iter().enumerate() {
            for x in 0..w {
                let mut acc = [0f32; 4];
                let mut total = 0.0;
                for sy in span.0..span.1 {
                    let wt = weight(sy, span);
                    let o = (sy * w + x) * 4;
                    for c in 0..4 {
                        acc[c] += tmp[o + c] * wt;
                    }
                    total += wt;
                }
                let a = acc[3] / total.max(1e-6);
                let o = (y * w + x) * 4;
                if a > 0.0 {
                    for c in 0..3 {
                        out[o + c] = (acc[c] / total.max(1e-6) / a).round().clamp(0.0, 255.0) as u8;
                    }
                }
                out[o + 3] = (a * 255.0).round().clamp(0.0, 255.0) as u8;
            }
        }
        Pixels::new(w as u32, h as u32, out)
    }

    /// Turned a quarter clockwise (`quarters` = 1), half (2) or three
    /// quarters (3).
    pub fn rotated(&self, quarters: u32) -> Pixels {
        let (w, h) = (self.width as usize, self.height as usize);
        let q = quarters % 4;
        if q == 0 {
            return self.clone();
        }
        let (nw, nh) = if q == 2 { (w, h) } else { (h, w) };
        let mut out = vec![0u8; self.data.len()];
        for y in 0..h {
            for x in 0..w {
                let (nx, ny) = match q {
                    1 => (h - 1 - y, x),
                    2 => (w - 1 - x, h - 1 - y),
                    _ => (y, w - 1 - x),
                };
                let (s, d) = ((y * w + x) * 4, (ny * nw + nx) * 4);
                out[d..d + 4].copy_from_slice(&self.data[s..s + 4]);
            }
        }
        Pixels::new(nw as u32, nh as u32, out)
    }

    /// Mirrored left to right.
    pub fn flipped(&self) -> Pixels {
        let w = self.width as usize;
        let mut out = self.data.clone();
        for row in out.chunks_exact_mut(w * 4) {
            for x in 0..w / 2 {
                let (a, b) = (x * 4, (w - 1 - x) * 4);
                for c in 0..4 {
                    row.swap(a + c, b + c);
                }
            }
        }
        Pixels::new(self.width, self.height, out)
    }
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

struct Data {
    id: u64,
    size: Cell<(u32, u32)>,
    /// Pixels waiting for upload (taken by the renderer).
    pixels: RefCell<Option<Vec<u8>>>,
    version: Cell<u32>,
    mipmaps: bool,
}

/// A cheap-to-clone handle to an image: straight-alpha sRGB RGBA rows, top
/// row first (what PNG/JPEG decoders produce).
#[derive(Clone)]
pub struct Image(Rc<Data>);

impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (w, h) = self.size();
        write!(f, "Image#{}({w}x{h})", self.0.id)
    }
}

impl PartialEq for Image {
    fn eq(&self, o: &Image) -> bool {
        Rc::ptr_eq(&self.0, &o.0)
    }
}

impl Image {
    /// An image showing decoded pixels (mipmapped, see [`Image::from_rgba`]).
    pub fn from_pixels(p: Pixels) -> Image {
        Image::new(p.width, p.height, p.data, true)
    }

    /// Decode an image file on the calling thread (small files; use
    /// [`Pixels::load`] on a worker thread for photos).
    pub fn load(path: impl AsRef<Path>) -> Result<Image, String> {
        Pixels::load(path).map(Image::from_pixels)
    }

    /// An image from `width` × `height` RGBA pixels. Minified smoothly
    /// (mipmapped), which suits photos and thumbnails.
    pub fn from_rgba(width: u32, height: u32, pixels: Vec<u8>) -> Image {
        Image::new(width, height, pixels, true)
    }

    /// Like `from_rgba` without mipmaps (for images always drawn at or above
    /// their size, e.g. charts redrawn often: cheaper uploads).
    pub fn from_rgba_unfiltered(width: u32, height: u32, pixels: Vec<u8>) -> Image {
        Image::new(width, height, pixels, false)
    }

    fn new(width: u32, height: u32, pixels: Vec<u8>, mipmaps: bool) -> Image {
        assert_eq!(pixels.len(), (width * height * 4) as usize, "RGBA data must be width*height*4 bytes");
        Image(Rc::new(Data {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            size: Cell::new((width, height)),
            pixels: RefCell::new(Some(pixels)),
            version: Cell::new(1),
            mipmaps,
        }))
    }

    pub fn size(&self) -> (u32, u32) {
        self.0.size.get()
    }

    pub fn width(&self) -> u32 {
        self.size().0
    }

    pub fn height(&self) -> u32 {
        self.size().1
    }

    /// Replace the pixels (and possibly the size); every place showing the
    /// image updates.
    pub fn update(&self, width: u32, height: u32, pixels: Vec<u8>) {
        assert_eq!(pixels.len(), (width * height * 4) as usize, "RGBA data must be width*height*4 bytes");
        self.0.size.set((width, height));
        *self.0.pixels.borrow_mut() = Some(pixels);
        self.0.version.set(self.0.version.get() + 1);
    }
}

/// Textures of the images painted so far.
#[derive(Default)]
pub(crate) struct ImageCache {
    map: HashMap<u64, (Weak<Data>, TexRef)>,
}

impl ImageCache {
    /// The texture showing `img`, uploading new pixels if needed.
    pub fn texture(&mut self, renderer: &mut Renderer, img: &Image) -> Option<TexRef> {
        let d = &img.0;
        let version = d.version.get();
        if let Some((_, t)) = self.map.get(&d.id) {
            if t.version == version {
                return Some(*t);
            }
        }
        let pixels = d.pixels.borrow_mut().take()?;
        if let Some((_, old)) = self.map.remove(&d.id) {
            renderer.delete_texture(old.id);
        }
        let (w, h) = d.size.get();
        // Larger than the GPU takes (a 50-megapixel photo): upload a copy
        // shrunk to fit; it is drawn over the same rectangle.
        let max = renderer.max_texture_size;
        let id = if w > max || h > max {
            let small = Pixels { width: w, height: h, data: pixels }.thumbnail(max, max);
            renderer.create_texture(small.width, small.height, &small.data, d.mipmaps)
        } else {
            renderer.create_texture(w, h, &pixels, d.mipmaps)
        };
        let t = TexRef { id, version };
        self.map.insert(d.id, (Rc::downgrade(&img.0), t));
        Some(t)
    }

    /// Free textures of images nobody holds any more.
    pub fn collect(&mut self, renderer: &mut Renderer) {
        self.map.retain(|_, (weak, t)| {
            let alive = weak.strong_count() > 0;
            if !alive {
                renderer.delete_texture(t.id);
            }
            alive
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
        use ::image::ImageEncoder;
        let mut out = Vec::new();
        ::image::codecs::png::PngEncoder::new(&mut out).write_image(rgba, w, h, ::image::ExtendedColorType::Rgba8).unwrap();
        out
    }

    #[test]
    fn decodes_png_and_rejects_garbage() {
        let px = [255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 0, 10, 20, 30, 40];
        let p = Pixels::decode(&png(2, 2, &px)).unwrap();
        assert_eq!((p.width, p.height), (2, 2));
        assert_eq!(p.data, px);
        assert!(Pixels::decode(b"hello, not an image").is_err());
        assert!(is_image_file("Photo.JPG") && is_image_file("a/b.webp") && !is_image_file("notes.txt"));
    }

    #[test]
    fn loads_files_and_reads_dimensions() {
        let dir = std::env::temp_dir().join(format!("lumen-ui-img-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.png");
        std::fs::write(&f, png(3, 1, &[9; 12])).unwrap();
        assert_eq!(Pixels::dimensions(&f).unwrap(), (3, 1));
        assert_eq!(Pixels::load(&f).unwrap().data, vec![9; 12]);
        let e = Pixels::load(dir.join("missing.png")).unwrap_err();
        assert!(e.contains("missing.png"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn thumbnails_average_and_keep_the_aspect() {
        // A 4x2 image: left half white opaque, right half fully transparent red.
        let mut data = Vec::new();
        for _ in 0..2 {
            for x in 0..4 {
                data.extend_from_slice(if x < 2 { &[255, 255, 255, 255] } else { &[255, 0, 0, 0] });
            }
        }
        let p = Pixels::new(4, 2, data);
        let t = p.thumbnail(2, 2);
        assert_eq!((t.width, t.height), (2, 1));
        assert_eq!(&t.data[0..4], &[255, 255, 255, 255]);
        assert_eq!(t.data[7], 0, "transparent stays transparent");
        // Transparent pixels do not darken their opaque neighbours.
        let one = p.thumbnail(1, 1);
        assert_eq!(&one.data, &[255, 255, 255, 128]);
        // Odd ratios: 3 -> 2 covers every source pixel once.
        let g = Pixels::new(3, 1, vec![0, 0, 0, 255, 90, 90, 90, 255, 180, 180, 180, 255]).thumbnail(2, 1);
        assert_eq!((g.data[0], g.data[4]), (30, 150));
        assert_eq!(p.thumbnail(100, 100), p, "never enlarged");
    }

    #[test]
    fn rotation_and_flip() {
        // 2x1: A B  -> rotated a quarter clockwise: A over B (1x2).
        let a = [1, 1, 1, 255];
        let b = [2, 2, 2, 255];
        let p = Pixels::new(2, 1, [a, b].concat());
        let r = p.rotated(1);
        assert_eq!((r.width, r.height), (1, 2));
        assert_eq!(r.data, [a, b].concat());
        assert_eq!(p.rotated(2).data, [b, a].concat());
        let r3 = p.rotated(3);
        assert_eq!(r3.data, [b, a].concat());
        assert_eq!(p.rotated(4), p);
        assert_eq!(p.flipped().data, [b, a].concat());
    }
}
