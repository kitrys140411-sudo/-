//! Images for apps: RGBA pixels the app decoded (or drew), uploaded to a GPU
//! texture the first time they are painted and freed when the last handle is
//! dropped.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::gfx::{Renderer, TexRef};

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
        let id = renderer.create_texture(w, h, &pixels, d.mipmaps);
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
