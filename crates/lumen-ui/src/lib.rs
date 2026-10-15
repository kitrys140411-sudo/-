//! lumen-ui: the GUI toolkit of Lumen's own apps (Files, Text Editor,
//! Settings, Calculator, System Monitor, Image Viewer…).
//!
//! An app is a struct implementing [`App`]: its `view` describes the whole
//! window every time something may have changed (input, an animation, a
//! timer, a background thread's result), and the toolkit lays it out with a
//! small flexbox ([`layout`]), paints it on the GPU (EGL/GLES 3, the same
//! renderer as `lumen-term`) and presents only the part that changed. When
//! nothing changes the window renders nothing at all, so idle apps cost no
//! CPU. Widgets are calls on [`Ui`] that return a [`Response`] (clicked,
//! changed, focused…); their small private state (scroll positions, cursor
//! blink, undo history) is kept by the toolkit under ids derived from their
//! names, while the app's state stays in the app struct.
//!
//! The window has client-side decorations matching the desktop (a header bar
//! with the title, app buttons via [`Ui::header_start`] / [`Ui::header_end`],
//! maximize and close), the desktop's theme (`~/.config/lumen/theme.toml`,
//! see [`Theme`]), Inter and JetBrains Mono text shaped by HarfBuzz,
//! [`Icon`]s drawn from vector paths, clipboard and primary selection, Tab
//! focus traversal with a focus ring, and fractional scaling.
//!
//! A complete small app, a task list that loads its file on a background
//! thread:
//!
//! ```no_run
//! use lumen_ui::prelude::*;
//!
//! #[derive(Default)]
//! struct Tasks {
//!     items: Vec<(String, bool)>,
//!     draft: String,
//!     selection: Selection,
//!     confirm_clear: bool,
//!     /// Reading the task file, until it is done.
//!     loading: Option<Task<std::io::Result<String>>>,
//!     loaded: bool,
//! }
//!
//! impl App for Tasks {
//!     fn view(&mut self, ui: &mut Ui) {
//!         // Slow work runs on a thread; its result wakes the window.
//!         if !self.loaded && self.loading.is_none() {
//!             self.loading = Some(ui.spawn(|| std::fs::read_to_string("tasks.txt")));
//!         }
//!         if let Some(text) = self.loading.as_mut().and_then(|t| t.take()) {
//!             let text = text.unwrap_or_default();
//!             self.items = text.lines().map(|l| (l.to_string(), false)).collect();
//!             self.loading = None;
//!             self.loaded = true;
//!         }
//!
//!         let open = self.items.iter().filter(|(_, done)| !done).count();
//!         ui.title(format!("Tasks — {open} open"));
//!         ui.header_end(|ui| {
//!             if ui.tool_button(Icon::Trash, "Remove Finished Tasks").clicked {
//!                 self.confirm_clear = true;
//!             }
//!         });
//!         // App shortcuts; a focused text input keeps its editing keys.
//!         if ui.shortcut(Shortcut::ctrl('q')) {
//!             ui.close_window();
//!         }
//!
//!         Frame::column().fill_width().fill_height().padding(16.0).gap(8.0).show(ui, |ui| {
//!             Frame::row().fill_width().gap(8.0).show(ui, |ui| {
//!                 let input = TextInput::new("new task", &mut self.draft).placeholder("New task").fill_width().show(ui);
//!                 let add = Button::new("Add").icon(Icon::Plus).primary().show(ui).clicked;
//!                 if (add || input.submitted) && !self.draft.trim().is_empty() {
//!                     self.items.push((std::mem::take(&mut self.draft), false));
//!                 }
//!             });
//!             if self.loading.is_some() {
//!                 ui.spinner();
//!             }
//!             // Only the rows on screen are built, however long the list.
//!             let items = &mut self.items;
//!             ListView::new("tasks", items.len(), 40.0).show(ui, &mut self.selection, |ui, item| {
//!                 let (text, done) = &mut items[item.index];
//!                 ui.checkbox(text, done);
//!             });
//!         });
//!
//!         if let Some(true) = ui.confirm("clear", &mut self.confirm_clear, "Remove finished tasks?",
//!             "They are removed from the list for good.", "Remove", true)
//!         {
//!             self.items.retain(|(_, done)| !done);
//!             self.selection.clear();
//!             ui.toast("Finished tasks removed");
//!         }
//!     }
//! }
//!
//! fn main() {
//!     let options = Options::new("lumen-tasks").title("Tasks").size(480, 560);
//!     if let Err(e) = run(options, Tasks::default()) {
//!         eprintln!("lumen-tasks: {e}");
//!         std::process::exit(1);
//!     }
//! }
//! ```
//!
//! Where to look next: [`widgets`] (every widget, most with a shorthand on
//! `Ui` and a builder for options), [`layout`] (sizes, rows, columns,
//! stacks), [`task`] (threads), [`FileChooser`], [`Painter`] (custom
//! drawing), [`Harness`] (unit-testing an app without a window) and the
//! crate's README, which also covers driving an app in the headless
//! compositor. `lumen-ui-gallery` shows every widget.

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
