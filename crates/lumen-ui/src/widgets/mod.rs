//! The widgets. Most have a shorthand on [`crate::Ui`] (`ui.button("Save")`)
//! and a builder for options (`Button::new("Save").primary().show(ui)`).

mod button;
mod containers;
mod dialog;
pub mod edit;
mod feedback;
mod file_chooser;
mod graph;
mod label;
mod list;
mod media;
mod menu;
mod nav;
mod range;
mod rows;
mod scroll;
mod text_doc;
pub(crate) mod text_input;
mod toggle;

pub use button::{Button, ButtonStyle, Widget, CONTROL_HEIGHT};
pub use containers::Frame;
pub use dialog::Dialog;
pub use feedback::Severity;
pub use file_chooser::{ChooserMode, ChooserResult, FileChooser, FileFilter};
pub use graph::{graph_palette, LineGraph};
pub use label::Label;
pub use list::{Item, ListResponse, ListView, Selection};
pub use media::{fit_image, Canvas, Fit, ImageView};
pub use menu::{MenuItem, MenuUi};
pub use range::{ProgressBar, Slider, Spinner};
pub use rows::ListRow;
pub use scroll::{ScrollArea, Viewport};
pub use text_doc::TextDoc;
pub use text_input::TextInput;
