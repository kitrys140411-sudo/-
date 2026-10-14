//! Pointer and keyboard state between frames: hit testing against the boxes
//! of the previous layout (what the user saw when acting), clicks with
//! multi-click counting, drags, scroll targets and keyboard focus.

use std::time::{Duration, Instant};

use crate::geom::{Rect, Vec2};
use crate::id::Id;
use crate::input::{Button, Event, KeyPress, Mods};
use crate::tree::Sense;

/// An interactive box of the last layout (clipped to its scroll areas), in
/// paint order.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Hit {
    pub id: Id,
    pub rect: Rect,
    pub layer: u32,
    pub sense: Sense,
    /// The nearest focusable box at or above this one: what a click focuses.
    pub focus: Option<Id>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Click {
    pub id: Id,
    pub button: Button,
    /// 1 single, 2 double, 3 triple.
    pub count: u32,
    pub pos: Vec2,
    pub mods: Mods,
}

#[derive(Clone, Debug)]
pub(crate) struct Scroll {
    pub target: Id,
    pub delta: Vec2,
    pub wheel: bool,
    pub kinetic_end: bool,
}

/// Distance the pointer must move while pressed before it is a drag.
const DRAG_THRESHOLD: f32 = 4.0;
const MULTI_CLICK: Duration = Duration::from_millis(400);

pub(crate) struct Input {
    pub pointer: Option<Vec2>,
    pub mods: Mods,
    pub hits: Vec<Hit>,
    pub hovered: Option<Id>,
    /// Pressed on this box with this button, still held.
    pub active: Option<(Id, Button)>,
    pub press_pos: Vec2,
    pub dragging: bool,
    // -- This pass ------------------------------------------------------------------
    pub clicks: Vec<Click>,
    pub presses: Vec<Click>,
    pub drag_delta: Vec2,
    pub drag_started: Option<Id>,
    pub drag_released: Option<Id>,
    pub scrolls: Vec<Scroll>,
    pub keys: Vec<(KeyPress, bool)>,
    pub pastes: Vec<(Id, String)>,
    // -- Focus ----------------------------------------------------------------------
    pub focus: Option<Id>,
    /// Focus moved by keyboard: draw the focus ring.
    pub focus_visible: bool,
    pub window_focused: bool,
    last_click: Option<(Id, Button, Instant, Vec2, u32)>,
}

impl Default for Input {
    fn default() -> Input {
        Input {
            pointer: None,
            mods: Mods::NONE,
            hits: Vec::new(),
            hovered: None,
            active: None,
            press_pos: Vec2::ZERO,
            dragging: false,
            clicks: Vec::new(),
            presses: Vec::new(),
            drag_delta: Vec2::ZERO,
            drag_started: None,
            drag_released: None,
            scrolls: Vec::new(),
            keys: Vec::new(),
            pastes: Vec::new(),
            focus: None,
            focus_visible: false,
            window_focused: true,
            last_click: None,
        }
    }
}

impl Input {
    /// Forget the previous pass's events.
    pub fn begin_pass(&mut self) {
        self.clicks.clear();
        self.presses.clear();
        self.drag_delta = Vec2::ZERO;
        self.drag_started = None;
        self.drag_released = None;
        self.scrolls.clear();
        self.keys.clear();
        self.pastes.clear();
    }

    /// The layer the pointer is in: the topmost layer with a box under it.
    fn top_layer_at(&self, p: Vec2) -> Option<u32> {
        self.hits.iter().rev().find(|h| h.sense.any() && h.rect.contains(p)).map(|h| h.layer)
    }

    fn find(&self, p: Vec2, want: impl Fn(&Sense) -> bool) -> Option<&Hit> {
        let layer = self.top_layer_at(p)?;
        self.hits.iter().rev().filter(|h| h.layer == layer).find(|h| want(&h.sense) && h.rect.contains(p))
    }

    /// The box the pointer is over at `p` (clickable or hoverable).
    pub fn hover_at(&self, p: Vec2) -> Option<&Hit> {
        self.find(p, |s| s.click || s.hover || s.drag)
    }

    pub fn scroll_target_at(&self, p: Vec2) -> Option<Id> {
        self.find(p, |s| s.scroll).map(|h| h.id)
    }

    /// Re-evaluate hovering after a new layout (content moved under a still
    /// pointer). Returns true if it changed.
    pub fn refresh_hover(&mut self) -> bool {
        let h = self.pointer.and_then(|p| self.hover_at(p).map(|h| h.id));
        let changed = h != self.hovered;
        self.hovered = h;
        changed
    }

    pub fn handle(&mut self, ev: Event, now: Instant) {
        match ev {
            Event::PointerMove(p) => {
                if let (Some(_), Some(old)) = (self.active, self.pointer) {
                    if !self.dragging && (p - self.press_pos).length() > DRAG_THRESHOLD {
                        self.dragging = true;
                        self.drag_started = self.active.map(|a| a.0);
                        // The drag starts where the press was.
                        self.drag_delta = self.drag_delta + (p - self.press_pos);
                    } else if self.dragging {
                        self.drag_delta = self.drag_delta + (p - old);
                    }
                }
                self.pointer = Some(p);
                self.hovered = self.hover_at(p).map(|h| h.id);
            }
            Event::PointerLeave => {
                self.pointer = None;
                self.hovered = None;
            }
            Event::PointerButton { pos, button, pressed: true } => {
                self.pointer = Some(pos);
                let hit = self.hover_at(pos).copied();
                self.hovered = hit.map(|h| h.id);
                // A click focuses what it hits (or nothing). Presses on
                // backdrops (modal scrims, outside menus) and on overlay
                // parts that take no focus (menu items, toasts) leave the
                // focus where it was, so choosing "Paste" from an input's
                // menu keeps typing in that input.
                let keeps = hit.is_some_and(|h| h.sense.blocking || (h.layer > 0 && h.focus.is_none()));
                if button == Button::Left && !keeps {
                    self.focus = hit.and_then(|h| h.focus);
                    self.focus_visible = false;
                }
                if let Some(h) = hit {
                    self.active = Some((h.id, button));
                    self.press_pos = pos;
                    self.dragging = false;
                    self.presses.push(Click { id: h.id, button, count: 1, pos, mods: self.mods });
                } else {
                    self.active = None;
                }
            }
            Event::PointerButton { pos, button, pressed: false } => {
                self.pointer = Some(pos);
                self.hovered = self.hover_at(pos).map(|h| h.id);
                if let Some((id, b)) = self.active {
                    if b == button {
                        if self.dragging {
                            self.drag_released = Some(id);
                        } else if self.hovered == Some(id) {
                            let count = match self.last_click {
                                Some((lid, lb, t, lp, n)) if lid == id && lb == button && now.duration_since(t) < MULTI_CLICK && (lp - pos).length() < 6.0 => n % 3 + 1,
                                _ => 1,
                            };
                            self.last_click = Some((id, button, now, pos, count));
                            self.clicks.push(Click { id, button, count, pos, mods: self.mods });
                        }
                        self.active = None;
                        self.dragging = false;
                    }
                }
            }
            Event::Scroll { pos, delta, wheel, kinetic_end } => {
                self.pointer = Some(pos);
                if let Some(target) = self.scroll_target_at(pos) {
                    self.scrolls.push(Scroll { target, delta, wheel, kinetic_end });
                }
            }
            Event::Key(k) => {
                self.mods = k.mods;
                self.keys.push((k, false));
            }
            Event::Paste { target, text } => self.pastes.push((target, text)),
            Event::Focus(f) => {
                self.window_focused = f;
                if !f {
                    self.active = None;
                    self.dragging = false;
                    self.mods = Mods::NONE;
                }
            }
        }
    }

    pub fn click(&self, id: Id, button: Button) -> Option<Click> {
        self.clicks.iter().rev().find(|c| c.id == id && c.button == button).copied()
    }

    pub fn press(&self, id: Id, button: Button) -> Option<Click> {
        self.presses.iter().rev().find(|c| c.id == id && c.button == button).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(id: u64, x: f32, y: f32, w: f32, h: f32, layer: u32, sense: Sense) -> Hit {
        Hit { id: Id(id), rect: Rect::new(x, y, w, h), layer, sense, focus: None }
    }

    fn input() -> Input {
        let mut i = Input::default();
        i.hits = vec![
            hit(1, 0.0, 0.0, 400.0, 400.0, 0, Sense::SCROLL),
            hit(2, 10.0, 10.0, 100.0, 30.0, 0, Sense::CLICK),
            hit(3, 50.0, 20.0, 20.0, 10.0, 0, Sense::CLICK),
        ];
        i
    }

    fn press(i: &mut Input, x: f32, y: f32, down: bool, t: Instant) {
        i.handle(Event::PointerButton { pos: Vec2::new(x, y), button: Button::Left, pressed: down }, t);
    }

    #[test]
    fn topmost_box_wins() {
        let i = input();
        assert_eq!(i.hover_at(Vec2::new(55.0, 25.0)).map(|h| h.id), Some(Id(3)));
        assert_eq!(i.hover_at(Vec2::new(15.0, 15.0)).map(|h| h.id), Some(Id(2)));
        assert_eq!(i.hover_at(Vec2::new(200.0, 200.0)).map(|h| h.id), None);
        assert_eq!(i.scroll_target_at(Vec2::new(15.0, 15.0)), Some(Id(1)));
    }

    #[test]
    fn click_needs_press_and_release_on_the_same_box() {
        let t = Instant::now();
        let mut i = input();
        press(&mut i, 15.0, 15.0, true, t);
        press(&mut i, 16.0, 16.0, false, t);
        assert_eq!(i.clicks.len(), 1);
        assert_eq!(i.clicks[0].id, Id(2));
        i.begin_pass();
        // Released elsewhere: no click.
        press(&mut i, 15.0, 15.0, true, t);
        i.handle(Event::PointerMove(Vec2::new(300.0, 300.0)), t);
        press(&mut i, 300.0, 300.0, false, t);
        assert!(i.clicks.is_empty());
    }

    #[test]
    fn double_and_triple_clicks() {
        let t = Instant::now();
        let mut i = input();
        for n in 1..=4 {
            i.begin_pass();
            press(&mut i, 15.0, 15.0, true, t);
            press(&mut i, 15.0, 15.0, false, t + Duration::from_millis(n * 100));
            assert_eq!(i.clicks[0].count as u64, (n - 1) % 3 + 1);
        }
        i.begin_pass();
        press(&mut i, 15.0, 15.0, true, t + Duration::from_secs(5));
        press(&mut i, 15.0, 15.0, false, t + Duration::from_secs(5));
        assert_eq!(i.clicks[0].count, 1, "too late for a double click");
    }

    #[test]
    fn drags_are_not_clicks() {
        let t = Instant::now();
        let mut i = input();
        press(&mut i, 15.0, 15.0, true, t);
        i.handle(Event::PointerMove(Vec2::new(18.0, 15.0)), t);
        assert!(!i.dragging, "below the threshold");
        i.handle(Event::PointerMove(Vec2::new(40.0, 15.0)), t);
        assert!(i.dragging);
        assert_eq!(i.drag_started, Some(Id(2)));
        assert_eq!(i.drag_delta, Vec2::new(25.0, 0.0));
        press(&mut i, 40.0, 15.0, false, t);
        assert!(i.clicks.is_empty());
        assert_eq!(i.drag_released, Some(Id(2)));
    }

    #[test]
    fn upper_layers_block_lower_ones() {
        let mut i = input();
        i.hits.push(hit(9, 0.0, 0.0, 400.0, 400.0, 1, Sense::BLOCK));
        i.hits.push(hit(10, 100.0, 100.0, 50.0, 50.0, 1, Sense::CLICK));
        assert_eq!(i.hover_at(Vec2::new(15.0, 15.0)).map(|h| h.id), None, "covered by the modal layer");
        assert_eq!(i.hover_at(Vec2::new(120.0, 120.0)).map(|h| h.id), Some(Id(10)));
        assert_eq!(i.scroll_target_at(Vec2::new(15.0, 15.0)), None);
    }

    #[test]
    fn clicking_focuses_the_hit_focus_target() {
        let t = Instant::now();
        let mut i = input();
        i.hits[1].focus = Some(Id(2));
        i.focus_visible = true;
        press(&mut i, 15.0, 15.0, true, t);
        assert_eq!(i.focus, Some(Id(2)));
        assert!(!i.focus_visible);
        press(&mut i, 15.0, 15.0, false, t);
        press(&mut i, 300.0, 300.0, true, t);
        assert_eq!(i.focus, None, "clicking empty space clears focus");
    }
}
