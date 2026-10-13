//! Modal dialogs inside the window: a dimmed backdrop, a centered card that
//! springs in, keyboard focus kept inside (Tab cycles through the dialog),
//! Escape cancels. `ui.confirm` and `ui.prompt` cover the common cases.

use std::hash::Hash;

use crate::anim::SpringParams;
use crate::geom::Vec2;
use crate::id::Id;
use crate::input::Key;
use crate::layout::{Align, Justify, Layout, Size, Sizing};
use crate::text::Weight;
use crate::tree::{Anchor, Node, Sense};
use crate::ui::Ui;

use super::{Button, Frame, Label, TextInput};

#[derive(Default)]
struct DialogState {
    was_open: bool,
}

#[must_use = "a Dialog does nothing until shown"]
pub struct Dialog {
    key: Id,
    title: String,
    width: f32,
}

impl Dialog {
    pub fn new(key: impl Hash) -> Dialog {
        Dialog { key: Id(0).with(key), title: String::new(), width: 420.0 }
    }

    pub fn title(mut self, t: impl Into<String>) -> Dialog {
        self.title = t.into();
        self
    }

    pub fn width(mut self, w: f32) -> Dialog {
        self.width = w;
        self
    }

    /// Show the dialog while `*open`; `f` builds its body and buttons.
    /// Escape sets `*open = false`. Returns `f`'s result while open.
    pub fn show<R>(self, ui: &mut Ui, open: &mut bool, f: impl FnOnce(&mut Ui) -> R) -> Option<R> {
        let id = ui.make_id(("dialog", self.key));
        if !*open {
            if ui.state::<DialogState>(id).was_open {
                ui.state::<DialogState>(id).was_open = false;
                ui.forget(id.with("in"));
            }
            return None;
        }
        let first = !std::mem::replace(&mut ui.state::<DialogState>(id).was_open, true);
        let t = ui.theme().clone();
        let appear = ui.animate_from(id.with("in"), 0.0, 1.0, SpringParams::POPOVER);
        // Backdrop: dims the window and takes every pointer event below.
        let scrim_id = id.with("scrim");
        let mut scrim = Node::new(Layout::default().size(Size::FILL, Size::FILL));
        scrim.id = Some(scrim_id);
        scrim.sense = Sense { click: true, blocking: true, ..Sense::NONE };
        scrim.deco.bg = t.scrim.fade(appear.clamp(0.0, 1.0));
        ui.overlay(scrim_id, Anchor::Fill, false, scrim, |_| {});
        let mut card = Node::new(Layout::column().width(Size::Px(self.width)).padding(24.0).gap(16.0));
        card.id = Some(id);
        card.sense = Sense::BLOCK;
        card.kind = "dialog";
        card.label = Some(self.title.clone());
        card.deco.bg = t.surface.with_alpha(1.0);
        card.deco.radius = t.radius_large + 4.0;
        card.deco.border = t.border;
        card.deco.border_width = 1.0;
        card.deco.shadow = Some((48.0, 16.0, t.shadow.with_alpha(0.5)));
        card.opacity = appear.clamp(0.0, 1.0);
        card.paint_offset = Vec2::new(0.0, (1.0 - appear) * 16.0);
        let title = self.title;
        let (_, r) = ui.overlay(id, Anchor::Center, true, card, |ui| {
            if !title.is_empty() {
                Label::new(title).size(t.size_heading()).weight(Weight::SemiBold).show(ui);
            }
            f(ui)
        });
        if ui.key_pressed_in(id, Key::Escape) {
            *open = false;
            ui.log("dialog cancel");
        }
        if first {
            ui.focus_first_in(id);
        }
        Some(r)
    }
}

impl Ui {
    /// Escape pressed while the overlay `owner` has the keyboard.
    fn key_pressed_in(&mut self, owner: Id, key: Key) -> bool {
        let prev = self.swap_overlay(Some(owner));
        let r = self.key_pressed(key);
        self.swap_overlay(prev);
        r
    }

    /// A row of dialog buttons, right-aligned.
    pub fn dialog_buttons<R>(&mut self, f: impl FnOnce(&mut Ui) -> R) -> R {
        Frame::row().fill_width().justify(Justify::End).align(Align::Center).gap(8.0).padding((8.0, 0.0, 0.0, 0.0)).show(self, f)
    }

    /// A confirmation dialog. Returns `Some(true)` when confirmed,
    /// `Some(false)` when cancelled (both close it), `None` otherwise.
    pub fn confirm(&mut self, key: &str, open: &mut bool, title: &str, message: &str, confirm: &str, destructive: bool) -> Option<bool> {
        let mut result = None;
        let shown = Dialog::new(("confirm", key)).title(title).show(self, open, |ui| {
            Label::new(message).wrap().dim().fill_width().show(ui);
            ui.dialog_buttons(|ui| {
                if ui.button("Cancel").clicked {
                    result = Some(false);
                }
                let b = Button::new(confirm);
                let b = if destructive { b.destructive() } else { b.primary() };
                if b.show(ui).clicked {
                    result = Some(true);
                }
            });
        });
        if shown.is_some() && !*open {
            result = Some(false);
        }
        if result.is_some() {
            *open = false;
        }
        result
    }

    /// A dialog asking for a line of text (rename, new folder). Enter or
    /// the confirm button returns `Some(true)`; Cancel/Escape `Some(false)`.
    pub fn prompt(&mut self, key: &str, open: &mut bool, title: &str, text: &mut String, confirm: &str) -> Option<bool> {
        let mut result = None;
        let shown = Dialog::new(("prompt", key)).title(title).show(self, open, |ui| {
            if TextInput::new(format!("{key}-text"), text).show(ui).submitted && !text.trim().is_empty() {
                result = Some(true);
            }
            ui.dialog_buttons(|ui| {
                if ui.button("Cancel").clicked {
                    result = Some(false);
                }
                if Button::new(confirm).primary().enabled(!text.trim().is_empty()).show(ui).clicked {
                    result = Some(true);
                }
            });
        });
        if shown.is_some() && !*open {
            result = Some(false);
        }
        if result.is_some() {
            *open = false;
        }
        result
    }
}
