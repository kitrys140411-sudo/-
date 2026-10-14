//! lumen-ui: the GUI toolkit of Lumen's own apps.

pub mod anim;
mod chrome;
pub mod color;
pub mod files;
pub mod geom;
pub mod gfx;
pub mod harness;
mod id;
pub mod icons;
pub mod image;
pub mod input;
mod interact;
pub mod layout;
pub mod paint;
pub mod task;
mod testing;
pub mod text;
pub mod theme;
mod tree;
#[cfg(test)]
mod widget_tests;
mod ui;
pub mod widgets;
mod window;

pub use anim::{Spring, SpringParams};
pub use chrome::HEADER_HEIGHT;
pub use color::Color;
pub use geom::{Insets, Rect, Vec2};
pub use harness::{Harness, WidgetInfo};
pub use icons::Icon;
pub use id::Id;
pub use crate::image::{is_image_file, Image, Pixels};
pub use input::{Key, KeyPress, Mods, Shortcut};
pub use layout::{Align, Justify, Layout, Size, Sizing};
pub use paint::Painter;
pub use task::{channel, Receiver, Sender, Task};
pub use text::{TextAlign, TextStyle, Weight, Wrap};
pub use theme::Theme;
pub use tree::{Anchor, Sense};
pub use ui::{CursorIcon, Response, Ui};
pub use widgets::*;
pub use window::{run, Waker};

/// Everything an app usually needs: `use lumen_ui::prelude::*;`.
pub mod prelude {
    pub use crate::files::{self, DirEntry, Place};
    pub use crate::widgets::*;
    pub use crate::{
        channel, run, Align, Anchor, App, Color, CursorIcon, Icon, Id, Image, Insets, Pixels, Justify, Key, KeyPress, Mods, Options, Painter, Rect,
        Receiver, Response, Sender, Sense, Shortcut, Size, Sizing, Task, TextAlign, TextStyle, Theme, Ui, Vec2, Waker, Weight, Wrap,
    };
}

/// An application: its state and how it looks.
pub trait App {
    /// Build the window's content for this frame (called whenever something
    /// may have changed: input, animations, timers, wake-ups).
    fn view(&mut self, ui: &mut Ui);

    /// The close button was pressed or the compositor asked the window to
    /// close; return false to keep it open (e.g. to ask about unsaved work).
    fn close_requested(&mut self) -> bool {
        true
    }
}

/// How the window starts.
#[derive(Clone, Debug)]
pub struct Options {
    /// Wayland app id: the desktop entry's name (`lumen-files`).
    pub app_id: String,
    pub title: String,
    /// Initial size in logical pixels.
    pub size: (u32, u32),
    pub min_size: (u32, u32),
}

impl Options {
    pub fn new(app_id: impl Into<String>) -> Options {
        let app_id = app_id.into();
        Options { title: app_id.clone(), app_id, size: (800, 600), min_size: (360, 240) }
    }

    pub fn title(mut self, t: impl Into<String>) -> Options {
        self.title = t.into();
        self
    }

    pub fn size(mut self, w: u32, h: u32) -> Options {
        self.size = (w, h);
        self
    }

    pub fn min_size(mut self, w: u32, h: u32) -> Options {
        self.min_size = (w, h);
        self
    }
}
