//! Scroll areas: wheel scrolling animated by a spring, touchpad scrolling
//! that follows the fingers with kinetic continuation after a flick, and
//! overlay scrollbars that appear while scrolling or hovered and can be
//! dragged (a click on the track pages toward it).

use std::hash::Hash;
use std::time::Instant;

use crate::anim::{Spring, SpringParams};
use crate::geom::{Rect, Vec2};
use crate::id::Id;
use crate::layout::{Align, Layout, Size, Sizing};
use crate::tree::{Node, Sense};
use crate::ui::Ui;

#[derive(Clone, Copy, PartialEq, Default)]
enum Motion {
    #[default]
    Rest,
    /// Easing toward the target (wheel, programmatic scrolls).
    Spring,
    /// Coasting after a touchpad flick.
    Kinetic,
}

pub(crate) struct ScrollState {
    pos: Vec2,
    target: Vec2,
    springs: [Spring; 2],
    velocity: Vec2,
    finger: (Option<Instant>, Vec2),
    motion: Motion,
    time: Option<Instant>,
    activity: Option<Instant>,
    /// Dragging a scrollbar thumb: axis, offset at the start.
    bar_drag: Option<(usize, f32)>,
}

impl Default for ScrollState {
    fn default() -> ScrollState {
        let p = SpringParams { stiffness: 420.0, damping: 1.0 };
        ScrollState {
            pos: Vec2::ZERO,
            target: Vec2::ZERO,
            springs: [Spring::new(0.0, p), Spring::new(0.0, p)],
            velocity: Vec2::ZERO,
            finger: (None, Vec2::ZERO),
            motion: Motion::Rest,
            time: None,
            activity: None,
            bar_drag: None,
        }
    }
}

impl ScrollState {
    /// Scroll so that `y0..y1` (content coordinates) is visible, animated.
    pub(crate) fn reveal_y(&mut self, y0: f32, y1: f32, viewport_h: f32) {
        let mut t = self.target.y;
        if y1 > t + viewport_h {
            t = y1 - viewport_h;
        }
        if y0 < t {
            t = y0;
        }
        self.scroll_to(Vec2::new(self.target.x, t));
    }

    pub(crate) fn scroll_to(&mut self, to: Vec2) {
        if to != self.target {
            self.target = to;
            self.motion = Motion::Spring;
        }
    }

    pub(crate) fn offset(&self) -> Vec2 {
        self.pos
    }
}

/// The visible part of a scroll area's content (content coordinates).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub rect: Rect,
    /// The size was known (false on the very first frame).
    pub known: bool,
}

#[must_use = "a ScrollArea does nothing until shown"]
pub struct ScrollArea {
    key: Option<Id>,
    exact: Option<Id>,
    x: bool,
    y: bool,
    layout: Layout,
    content: Layout,
}

impl ScrollArea {
    fn new(x: bool, y: bool) -> ScrollArea {
        ScrollArea { key: None, exact: None, x, y, layout: Layout::stack().size(Size::FILL, Size::FILL), content: Layout::column() }
    }

    /// Scrolls up and down; the content is as wide as the area.
    pub fn vertical() -> ScrollArea {
        ScrollArea::new(false, true)
    }

    /// Scrolls left and right.
    pub fn horizontal() -> ScrollArea {
        let mut s = ScrollArea::new(true, false);
        s.content = Layout::row();
        s
    }

    pub fn both() -> ScrollArea {
        ScrollArea::new(true, true)
    }

    pub fn id(mut self, key: impl Hash) -> ScrollArea {
        self.key = Some(Id(0).with(key));
        self
    }

    /// Use exactly this id (widgets that need the area's state).
    pub(crate) fn with_id(mut self, id: Id) -> ScrollArea {
        self.exact = Some(id);
        self
    }

    /// Layout of the content box (a column by default): padding, gap…
    pub fn content(mut self, l: Layout) -> ScrollArea {
        self.content = l;
        self
    }

    pub fn padding(mut self, p: impl Into<crate::geom::Insets>) -> ScrollArea {
        self.content.padding = p.into();
        self
    }

    pub fn gap(mut self, g: f32) -> ScrollArea {
        self.content.gap = g;
        self
    }

    pub fn show<R>(self, ui: &mut Ui, f: impl FnOnce(&mut Ui) -> R) -> R {
        self.show_viewport(ui, |ui, _| f(ui))
    }

    /// Like `show`, telling `f` which part of the content is visible (for
    /// building only what is on screen).
    pub fn show_viewport<R>(self, ui: &mut Ui, f: impl FnOnce(&mut Ui, Viewport) -> R) -> R {
        let id = match (self.exact, self.key) {
            (Some(id), _) => id,
            (None, Some(k)) => ui.make_id(k),
            (None, None) => ui.make_id("scroll"),
        };
        let (offset, viewport) = scroll_logic(ui, id, self.x, self.y);
        let mut outer = Node::new(self.layout);
        outer.id = Some(id);
        outer.sense = Sense::SCROLL;
        outer.kind = "scroll";
        outer.label = Some(format!("scroll-{:x}", id.0 & 0xffffff));
        if ui.test.enabled() {
            outer.value = Some(format!("{:.0}", offset.y));
        }
        let mut vp = Node::new(self.content.size(if self.x { Size::FILL } else { Size::FILL }, Size::FILL));
        vp.layout.scroll_x = self.x;
        vp.layout.scroll_y = self.y;
        if !self.x {
            vp.layout.align = if vp.layout.dir == crate::layout::Dir::Column && vp.layout.align == Align::Start { Align::Stretch } else { vp.layout.align };
        }
        vp.id = Some(id.with("vp"));
        vp.clip = true;
        vp.scroll_offset = offset;
        let (_, r) = ui.add_container(outer, |ui| {
            let (_, r) = ui.add_container(vp, |ui| f(ui, viewport));
            scrollbars(ui, id, self.x, self.y);
            r
        });
        r
    }
}

impl Sizing for ScrollArea {
    fn layout_mut(&mut self) -> &mut Layout {
        &mut self.layout
    }
}

/// Apply this frame's scroll input and animation; returns the offset and
/// the visible content rectangle.
fn scroll_logic(ui: &mut Ui, id: Id, ax: bool, ay: bool) -> (Vec2, Viewport) {
    let vp_id = id.with("vp");
    let view = ui.prev_rect(vp_id);
    let content = ui.prev_content(vp_id).unwrap_or(Vec2::ZERO);
    let size = view.map(|r| r.size()).unwrap_or(ui.window_size());
    let max = Vec2::new(if ax { (content.x - size.x).max(0.0) } else { 0.0 }, if ay { (content.y - size.y).max(0.0) } else { 0.0 });
    let events = ui.take_scrolls(id);
    let now = ui.now();
    let speed = ui.theme().anim_speed;
    let st = ui.state::<ScrollState>(id);
    for (mut d, wheel, kinetic_end) in events {
        if !ax {
            d.x = 0.0;
        }
        if !ay {
            // Horizontal-only areas scroll with the vertical wheel too.
            d = Vec2::new(d.x + d.y, 0.0);
        }
        st.activity = Some(now);
        if kinetic_end {
            let (t, v) = st.finger;
            let recent = t.is_some_and(|t| now.duration_since(t).as_secs_f32() < 0.08);
            if recent && v.length() > 60.0 {
                st.velocity = Vec2::new(v.x.clamp(-6000.0, 6000.0), v.y.clamp(-6000.0, 6000.0));
                st.motion = Motion::Kinetic;
            }
        } else if wheel {
            st.target = st.target + d;
            st.motion = Motion::Spring;
        } else {
            // Touchpad: follow the fingers exactly and track their speed.
            st.target = st.target + d;
            st.target = Vec2::new(st.target.x.clamp(0.0, max.x), st.target.y.clamp(0.0, max.y));
            st.pos = st.target;
            st.springs[0].snap(st.pos.x);
            st.springs[1].snap(st.pos.y);
            st.motion = Motion::Rest;
            let dt = st.finger.0.map(|t| now.duration_since(t).as_secs_f32()).unwrap_or(0.016).clamp(0.004, 0.1);
            let v = d * (1.0 / dt);
            st.finger = (Some(now), if dt > 0.09 { v } else { st.finger.1 * 0.6 + v * 0.4 });
        }
    }
    st.target = Vec2::new(st.target.x.clamp(0.0, max.x), st.target.y.clamp(0.0, max.y));
    let dt = st.time.map(|t| now.saturating_duration_since(t).as_secs_f32()).unwrap_or(0.0).min(0.1);
    st.time = Some(now);
    let mut moving = false;
    match st.motion {
        Motion::Rest => {
            st.pos = Vec2::new(st.pos.x.clamp(0.0, max.x), st.pos.y.clamp(0.0, max.y));
            st.target = st.pos;
        }
        Motion::Spring => {
            if speed <= 0.0 {
                st.pos = st.target;
            } else {
                for (axis, s) in st.springs.iter_mut().enumerate() {
                    let target = if axis == 0 { st.target.x } else { st.target.y };
                    s.value = if axis == 0 { st.pos.x } else { st.pos.y };
                    s.set_target(target);
                    s.step(dt, 0.05);
                }
                st.pos = Vec2::new(st.springs[0].value, st.springs[1].value);
            }
            moving = st.pos != st.target;
            if !moving {
                st.motion = Motion::Rest;
            }
        }
        Motion::Kinetic => {
            st.pos = st.pos + st.velocity * dt;
            st.velocity = st.velocity * (-3.2 * dt).exp();
            let clamped = Vec2::new(st.pos.x.clamp(0.0, max.x), st.pos.y.clamp(0.0, max.y));
            if clamped != st.pos {
                st.pos = clamped;
                st.velocity = Vec2::ZERO;
            }
            st.target = st.pos;
            moving = st.velocity.length() > 15.0;
            if !moving {
                st.motion = Motion::Rest;
            }
            st.springs[0].snap(st.pos.x);
            st.springs[1].snap(st.pos.y);
        }
    }
    if moving {
        st.activity = Some(now);
    }
    let pos = st.pos;
    let activity = st.activity;
    if moving {
        ui.request_repaint();
    }
    // Keep the bars while they fade out.
    if let Some(a) = activity {
        let since = now.duration_since(a).as_secs_f32();
        if since < 1.6 {
            ui.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }
    if view.is_none() {
        // The first layout decides the real viewport: build again with it.
        ui.relayout = true;
    }
    (pos, Viewport { rect: Rect::new(pos.x, pos.y, size.x, size.y), known: view.is_some() })
}

/// Overlay scrollbars for the area `id`.
fn scrollbars(ui: &mut Ui, id: Id, ax: bool, ay: bool) {
    let vp_id = id.with("vp");
    let (Some(view), Some(content)) = (ui.prev_rect(vp_id), ui.prev_content(vp_id)) else { return };
    let t = ui.theme().clone();
    let now = ui.now();
    for (axis, on) in [(1usize, ay), (0usize, ax)] {
        let (len, total) = if axis == 1 { (view.h, content.y) } else { (view.w, content.x) };
        if !on || total <= len + 0.5 {
            continue;
        }
        let bar_id = id.with(("bar", axis));
        let r = ui.interact(bar_id);
        let (activity, offset, dragging) = {
            let st = ui.state::<ScrollState>(id);
            (st.activity, st.pos, st.bar_drag)
        };
        let off = if axis == 1 { offset.y } else { offset.x };
        let track = r.rect;
        let track_len = if axis == 1 { track.h } else { track.w };
        let thumb_len = (track_len * len / total).max(32.0).min(track_len);
        let max_off = total - len;
        // Dragging the thumb, or paging toward a click on the track.
        if let Some((p, _)) = ui.press_pos(bar_id) {
            let along = if axis == 1 { p.y - track.y } else { p.x - track.x };
            let thumb_start = (track_len - thumb_len) * off / max_off.max(1.0);
            let st = ui.state::<ScrollState>(id);
            if along >= thumb_start && along <= thumb_start + thumb_len {
                st.bar_drag = Some((axis, off));
            } else {
                let page = len * 0.9 * if along < thumb_start { -1.0 } else { 1.0 };
                let mut to = st.target;
                if axis == 1 {
                    to.y = (to.y + page).clamp(0.0, max_off);
                } else {
                    to.x = (to.x + page).clamp(0.0, max_off);
                }
                st.scroll_to(to);
            }
        }
        if r.dragging {
            let st = ui.state::<ScrollState>(id);
            if let Some((a, start)) = st.bar_drag.filter(|d| d.0 == axis) {
                let d = if a == 1 { r.drag_delta.y } else { r.drag_delta.x };
                // drag_delta is per frame: accumulate into the start offset.
                let new = (start + d * max_off / (track_len - thumb_len).max(1.0)).clamp(0.0, max_off);
                st.bar_drag = Some((a, new));
                if a == 1 {
                    st.pos.y = new;
                } else {
                    st.pos.x = new;
                }
                st.target = st.pos;
                st.springs[0].snap(st.pos.x);
                st.springs[1].snap(st.pos.y);
                st.motion = Motion::Rest;
                st.activity = Some(now);
            }
        }
        if r.drag_released || (!r.dragging && dragging.is_some() && ui.press_pos(bar_id).is_none() && !r.pressed) {
            ui.state::<ScrollState>(id).bar_drag = None;
        }
        let recent = activity.is_some_and(|a| now.duration_since(a).as_secs_f32() < 1.0);
        let visible = recent || r.hovered || r.dragging || r.pressed;
        let alpha = ui.animate(bar_id.with("a"), if visible { 1.0 } else { 0.0 }, SpringParams::GENTLE);
        let wide = ui.animate_bool(bar_id.with("w"), r.hovered || r.dragging);
        // The bar's box is narrow so it takes little from the content
        // under it; hovering it reveals the thumb.
        let layout = if axis == 1 {
            Layout::default().size(Size::Px(12.0), Size::FILL).align_self(Align::End)
        } else {
            Layout::default().size(Size::FILL, Size::Px(12.0))
        };
        let mut n = Node::new(layout);
        n.id = Some(bar_id);
        n.sense = Sense::DRAG;
        let pos_frac = off / max_off.max(1.0);
        n.paint = Some(Box::new(move |p, r| {
            if alpha <= 0.01 {
                return;
            }
            let thick = 4.0 + 3.0 * wide;
            let tl = if axis == 1 { r.h } else { r.w };
            let tlen = (tl * len / total).max(32.0).min(tl);
            let start = (tl - tlen) * pos_frac;
            let thumb = if axis == 1 {
                Rect::new(r.right() - thick - 4.0, r.y + start + 2.0, thick, tlen - 4.0)
            } else {
                Rect::new(r.x + start + 2.0, r.bottom() - thick - 4.0, tlen - 4.0, thick)
            };
            p.rounded(thumb, thick / 2.0, t.text.with_alpha((0.32 + 0.18 * wide) * alpha));
        }));
        if axis == 0 {
            // Horizontal bars sit at the bottom of the stack.
            n.layout.align_self = Some(Align::Start);
            let mut wrap = Node::new(Layout::column().size(Size::FILL, Size::FILL).justify(crate::layout::Justify::End));
            wrap.id = None;
            ui.add_container(wrap, |ui| {
                ui.add_node(n);
            });
        } else {
            ui.add_node(n);
        }
    }
}

impl Ui {
    /// A vertical scroll area filling the available space.
    pub fn scroll<R>(&mut self, f: impl FnOnce(&mut Ui) -> R) -> R {
        ScrollArea::vertical().show(self, f)
    }

    pub(crate) fn scroll_state(&mut self, id: Id) -> &mut ScrollState {
        self.state::<ScrollState>(id)
    }
}
