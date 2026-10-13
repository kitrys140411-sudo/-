//! The widgets. Most have a shorthand on [`crate::Ui`] (`ui.button("Save")`)
//! and a builder for options (`Button::new("Save").primary().show(ui)`).

mod button;
mod containers;
mod dialog;
pub mod edit;
mod feedback;
mod label;
mod list;
mod media;
mod menu;
mod nav;
mod range;
mod scroll;
mod text_input;
mod toggle;

pub use button::{Button, ButtonStyle, Widget, CONTROL_HEIGHT};
pub use containers::Frame;
pub use dialog::Dialog;
pub use feedback::Severity;
pub use label::Label;
pub use list::{Item, ListResponse, ListView, Selection};
pub use media::{fit_image, Canvas, Fit, ImageView};
pub use menu::{MenuItem, MenuUi};
pub use range::{ProgressBar, Slider, Spinner};
pub use scroll::{ScrollArea, Viewport};
pub use text_input::TextInput;
