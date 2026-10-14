//! [`Ui`]: what an app's `view` builds the window with, every frame.
//!
//! The view creates boxes (rows, columns, widgets); after it returns, the
//! toolkit lays them out and paints them. Widget calls return a
//! [`Response`] computed from the boxes as they were laid out in the previous
//! frame — i.e. what the user saw when they clicked — so interactions are
//! exact without a second pass.

use std::any::Any;
use std::collections::HashMap;
use std::hash::Hash;
use std::time::{Duration, Instant};

use crate::anim::{Spring, SpringParams};
use crate::geom::{Rect, Vec2};
use crate::id::Id;
use crate::input::{Button, Event, Key, KeyPress, Mods, Shortcut};
use crate::interact::{Hit, Input};
use crate::layout::{Dir, Layout, Size};
use crate::paint::Painter;
use crate::testing::TestLog;
use crate::text::Fonts;
use crate::theme::Theme;
use crate::tree::{Anchor, Node, Root, Sense, Tree, Visual, NONE};

/// What happened to a widget this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Response {
    pub id: Id,
    /// Where the widget was in the last frame (logical window coordinates).
    pub rect: Rect,
    pub hovered: bool,
    /// The primary button is held down on it (and the pointer is over it).
    pub pressed: bool,
    /// Activated: primary click, or Enter/Space while focused (buttons).
    pub clicked: bool,
    pub double_clicked: bool,
    /// Right click (or the Menu key while focused): open a context menu.
    pub secondary_clicked: bool,
    pub middle_clicked: bool,
    pub drag_started: bool,
    pub dragging: bool,
    pub drag_released: bool,
    /// Pointer movement during a drag this frame.
    pub drag_delta: Vec2,
    pub focused: bool,
    /// Value widgets: the value changed this frame.
    pub changed: bool,
    /// Text inputs: Enter was pressed.
    pub submitted: bool,
    /// Where the click (or press) happened, and the modifiers then.
    pub pointer: Option<Vec2>,
    pub mods: Mods,
}

impl Response {
    /// Merge another response into this one (a widget made of parts).
    pub fn union(mut self, o: Response) -> Response {
        self.hovered |= o.hovered;
        self.pressed |= o.pressed;
        self.clicked |= o.clicked;
        self.double_clicked |= o.double_clicked;
        self.secondary_clicked |= o.secondary_clicked;
        self.middle_clicked |= o.middle_clicked;
        self.changed |= o.changed;
        self.submitted |= o.submitted;
        self.focused |= o.focused;
        self.pointer = self.pointer.or(o.pointer);
        self
    }
}

/// Pointer shapes a widget can ask for while hovered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CursorIcon {
    #[default]
    Default,
    Pointer,
    Text,
    Grab,
    Grabbing,
    Move,
    ResizeEw,
    ResizeNs,
    ResizeNwse,
    ResizeNesw,
    NotAllowed,
    Progress,
}

/// Requests from the frame to the window runtime.
#[derive(Default)]
pub(crate) struct Requests {
    pub title: Option<String>,
    pub close: bool,
    pub toggle_maximize: bool,
    pub start_move: bool,
    pub window_menu: Option<Vec2>,
    pub fullscreen: Option<bool>,
    pub copy: Option<(String, bool)>,
    pub paste: Option<(Id, bool)>,
}

/// Window state the runtime keeps up to date.
#[derive(Clone, Copy, Debug)]
pub(crate) struct WindowState {
    pub size: Vec2,
    pub scale: f32,
    pub maximized: bool,
    pub fullscreen: bool,
    /// Tiled or maximized: no resize borders.
    pub tiled: bool,
    pub decorated: bool,
}

struct Slot {
    value: Box<dyn Any>,
    used: u64,
}

struct AnimSlot {
    spring: Spring,
    time: Instant,
    used: u64,
}

/// Header bar slots of the current pass.
#[derive(Clone, Copy)]
pub(crate) struct Chrome {
    pub header: u32,
    pub start: u32,
    pub end: u32,
    pub title: u32,
    pub content: u32,
}

pub struct Ui {
    pub(crate) theme: Theme,
    pub(crate) fonts: Fonts,
    pub(crate) input: Input,
    pub(crate) tree: Tree,
    pub(crate) window: WindowState,
    pub(crate) out: Requests,
    pub(crate) test: TestLog,
    pub(crate) chrome: Chrome,
    pub(crate) title: String,
    pub(crate) cursor: CursorIcon,
    memory: HashMap<Id, Slot>,
    anims: HashMap<Id, AnimSlot>,
    parent: u32,
    scope: Id,
    counts: HashMap<(Id, u64), u32>,
    /// Overlay being built (None for the window content).
    overlay: Option<Id>,
    layer: u32,
    /// The topmost overlay that takes the keyboard (menus, dialogs) as of
    /// the previous pass; keys go only to it.
    key_owner: Option<Id>,
    key_owners: Vec<Id>,
    focus_order: Vec<(Id, Option<Id>)>,
    /// Focus the first focusable widget of this overlay after the pass.
    focus_first: Option<Id>,
    prev_focus_order: Vec<(Id, Option<Id>)>,
    prev_rects: HashMap<Id, Rect>,
    /// Content extents of boxes with ids (scroll areas).
    prev_content: HashMap<Id, Vec2>,
    /// A focused text input was built in this pass (in this overlay; and
    /// whether it is multi-line), and in the previous one: app shortcuts
    /// leave its editing keys alone.
    text_owner: Option<(Option<Id>, bool)>,
    prev_text_owner: Option<(Option<Id>, bool)>,
    pub(crate) now: Instant,
    pub(crate) frame: u64,
    pub(crate) animating: bool,
    pub(crate) repaint_at: Option<Instant>,
    /// A rebuild is needed before painting (a widget saw that its guess of
    /// the layout was wrong, e.g. a virtual list's viewport).
    pub(crate) relayout: bool,
    pub(crate) waker: crate::Waker,
    /// Rows built so far in the current [`Ui::group`] (None outside one).
    pub(crate) group_rows: Option<usize>,
}

impl Ui {
    pub(crate) fn new(theme: Theme, fonts: Fonts, waker: crate::Waker) -> Ui {
        Ui {
            theme,
            fonts,
            input: Input::default(),
            tree: Tree::default(),
            window: WindowState { size: Vec2::new(800.0, 600.0), scale: 1.0, maximized: false, fullscreen: false, tiled: false, decorated: true },
            out: Requests::default(),
            test: TestLog::from_env(),
            chrome: Chrome { header: NONE, start: NONE, end: NONE, title: NONE, content: NONE },
            title: String::new(),
            cursor: CursorIcon::Default,
            memory: HashMap::new(),
            anims: HashMap::new(),
            parent: NONE,
            scope: Id::ROOT,
            counts: HashMap::new(),
            overlay: None,
            layer: 0,
            key_owner: None,
            key_owners: Vec::new(),
            focus_order: Vec::new(),
            focus_first: None,
            prev_focus_order: Vec::new(),
            prev_rects: HashMap::new(),
            prev_content: HashMap::new(),
            text_owner: None,
            prev_text_owner: None,
            now: Instant::now(),
            frame: 0,
            animating: false,
            repaint_at: None,
            relayout: false,
            waker,
            group_rows: None,
        }
    }

    // -- Frame passes (driven by the runtime) ---------------------------------------------

    /// Build passes for one frame: until every queued event was handled (a
    /// click ends a pass so the next event sees its effect) and the layout
    /// is stable. Returns true when the window should close.
    pub(crate) fn run_frame(&mut self, app: &mut dyn crate::App, events: &mut std::collections::VecDeque<Event>, now: Instant) -> bool {
        let mut first = true;
        let mut passes = 0;
        let mut close = false;
        loop {
            let mut batch = Vec::new();
            while let Some(e) = events.pop_front() {
                let barrier = e.is_barrier();
                batch.push(e);
                if barrier {
                    break;
                }
            }
            // Events change state the view shows (and the boxes the next
            // events hit): after a pass with events, build once more clean.
            let rounds = if batch.is_empty() { 1 } else { 2 };
            for r in 0..rounds {
                let evs = if r == 0 { std::mem::take(&mut batch) } else { Vec::new() };
                self.begin_pass(evs, now, first);
                crate::chrome::build(self, app);
                self.end_pass();
                first = false;
                passes += 1;
                // The app may keep the window open (e.g. to ask first).
                if std::mem::take(&mut self.out.close) && app.close_requested() {
                    close = true;
                }
            }
            if passes >= 24 {
                break;
            }
            if events.is_empty() {
                if !self.relayout {
                    break;
                }
                if passes >= 4 {
                    break;
                }
            }
        }
        close
    }

    /// Start a build pass handling `events`.
    pub(crate) fn begin_pass(&mut self, events: Vec<Event>, now: Instant, first: bool) {
        if first {
            self.now = now;
            self.frame += 1;
            self.animating = false;
            self.repaint_at = None;
            self.fonts.next_frame();
        }
        self.relayout = false;
        self.input.begin_pass();
        for e in events {
            self.input.handle(e, now);
        }
        self.tree.clear();
        self.counts.clear();
        self.parent = NONE;
        self.scope = Id::ROOT;
        self.overlay = None;
        self.layer = 0;
        self.key_owners.clear();
        self.focus_order.clear();
        self.cursor = CursorIcon::Default;
        self.group_rows = None;
    }

    /// Lay out what was built, record hit boxes and handle Tab focus moves.
    pub(crate) fn end_pass(&mut self) {
        let scale = self.window.scale;
        self.tree.layout(self.window.size, &mut self.fonts, scale);
        crate::chrome::place_title(self);
        self.record_hits();
        // Tab / Shift+Tab move focus when no widget used the key.
        let tab = self.input.keys.iter_mut().find(|(k, used)| !*used && k.key == Key::Tab && !k.mods.command());
        if let Some((k, used)) = tab {
            *used = true;
            let back = k.mods.shift;
            self.move_focus(back);
        }
        if let Some(owner) = self.focus_first.take() {
            // Unless the content focused one of its widgets itself (a file
            // chooser focuses its list), the first focusable one.
            let inside = |f: Id| self.focus_order.iter().any(|(id, o)| *id == f && *o == Some(owner));
            if !self.input.focus.is_some_and(inside) {
                if let Some((id, _)) = self.focus_order.iter().find(|(_, o)| *o == Some(owner)) {
                    self.input.focus = Some(*id);
                    self.animating = true;
                }
            }
        }
        // Focus on a widget that no longer exists is dropped.
        if let Some(f) = self.input.focus {
            if !self.focus_order.iter().any(|(id, _)| *id == f) && !self.prev_rects.contains_key(&f) {
                self.input.focus = None;
            }
        }
        if self.input.refresh_hover() {
            self.animating = true;
        }
        self.key_owner = self.key_owners.last().copied();
        self.prev_focus_order = std::mem::take(&mut self.focus_order);
        self.prev_text_owner = self.text_owner.take();
        self.gc();
    }

    fn move_focus(&mut self, back: bool) {
        let owner = self.key_owners.last().copied();
        let order: Vec<Id> = self.focus_order.iter().filter(|(_, o)| owner.is_none() || *o == owner).map(|(id, _)| *id).collect();
        if order.is_empty() {
            return;
        }
        let cur = self.input.focus.and_then(|f| order.iter().position(|&x| x == f));
        let next = match (cur, back) {
            (None, false) => 0,
            (None, true) => order.len() - 1,
            (Some(i), false) => (i + 1) % order.len(),
            (Some(i), true) => (i + order.len() - 1) % order.len(),
        };
        self.input.focus = Some(order[next]);
        self.input.focus_visible = true;
        self.animating = true;
        self.test.event(&format!("focus {}", self.label_of(order[next])));
    }

    fn label_of(&self, id: Id) -> String {
        self.tree.nodes.iter().find(|n| n.id == Some(id)).and_then(|n| n.label.clone()).unwrap_or_else(|| format!("{:x}", id.0))
    }

    /// Hit boxes and rectangles of this layout, in paint order.
    fn record_hits(&mut self) {
        let mut hits = Vec::new();
        self.prev_rects.clear();
        self.prev_content.clear();
        let roots: Vec<(u32, u32)> = self.tree.roots.iter().map(|r| (r.node, r.layer)).collect();
        let full = Rect::new(0.0, 0.0, self.window.size.x, self.window.size.y);
        for (node, layer) in roots {
            self.hits_of(node, full, layer, None, &mut hits);
        }
        self.input.hits = hits;
    }

    fn hits_of(&mut self, i: u32, clip: Rect, layer: u32, focus: Option<Id>, out: &mut Vec<Hit>) {
        let n = &self.tree.nodes[i as usize];
        let rect = n.rect;
        let focus = if n.focusable { n.id } else { focus };
        if let Some(id) = n.id {
            self.prev_rects.insert(id, rect);
            if n.layout.scroll_x || n.layout.scroll_y {
                self.prev_content.insert(id, n.content);
            }
            if n.sense.any() {
                let r = rect.intersect(&clip);
                if !r.is_empty() {
                    out.push(Hit { id, rect: r, layer, sense: n.sense, focus });
                }
            }
        }
        let clip = if n.clip { clip.intersect(&rect) } else { clip };
        let kids: Vec<u32> = self.tree.children(i).collect();
        for c in kids {
            self.hits_of(c, clip, layer, focus, out);
        }
    }

    /// Drop state of widgets that have not been shown for a long time.
    fn gc(&mut self) {
        if self.frame % 600 != 0 {
            return;
        }
        let frame = self.frame;
        self.memory.retain(|_, s| s.used + 3000 > frame);
        self.anims.retain(|_, s| s.used + 600 > frame);
    }

    /// Paint the laid-out tree (consumes its paint closures).
    pub(crate) fn paint(&mut self, p: &mut Painter) {
        let roots: Vec<u32> = self.tree.roots.iter().map(|r| r.node).collect();
        for r in roots {
            self.paint_node(r, p);
        }
    }

    fn paint_node(&mut self, i: u32, p: &mut Painter) {
        let focus = self.input.focus;
        let focus_visible = self.input.focus_visible;
        let ring = self.theme.focus_ring;
        let n = &mut self.tree.nodes[i as usize];
        let r = n.rect;
        if n.opacity <= 0.0 {
            return;
        }
        let faded = n.opacity < 1.0;
        if faded {
            p.push_opacity(n.opacity);
        }
        let shifted = n.paint_offset != Vec2::ZERO;
        if shifted {
            p.push_offset(n.paint_offset);
        }
        let d = n.deco;
        if let Some((blur, dy, c)) = d.shadow {
            p.shadow(r, d.radius, blur, dy, c);
        }
        p.rounded(r, d.radius, d.bg);
        if d.border_width > 0.0 {
            p.border(r, d.radius, d.border_width, d.border);
        }
        let inner = r.inset(n.layout.padding);
        match &n.visual {
            Visual::None => {}
            Visual::Text { color, align } => {
                if let Some(t) = &n.text {
                    p.text(t, inner.min(), *color, *align, Some(inner.w));
                }
            }
            Visual::Icon { icon, size, color } => p.icon(*icon, inner, *size, *color),
        }
        if let Some(f) = n.paint.take() {
            f(p, r);
        }
        let clip = n.clip;
        let paint_over = n.paint_over.take();
        let focused = n.focusable && n.id.is_some() && n.id == focus && focus_visible;
        if clip {
            p.push_clip(r);
        }
        let kids: Vec<u32> = self.tree.children(i).collect();
        for c in kids {
            self.paint_node(c, p);
        }
        if clip {
            p.pop_clip();
        }
        if let Some(f) = paint_over {
            f(p, r);
        }
        if focused {
            let rad = self.tree.nodes[i as usize].deco.radius;
            p.border(r.expand(2.0), rad + 2.0, 2.0, ring);
        }
        if shifted {
            p.pop_offset();
        }
        if faded {
            p.pop_opacity();
        }
    }

    // -- Building boxes -----------------------------------------------------------------

    /// Add a box as a child of the current container; returns its index.
    pub(crate) fn add_node(&mut self, node: Node) -> u32 {
        let focusable = node.focusable;
        let id = node.id;
        let i = self.tree.add(self.parent, node);
        if let (true, Some(id)) = (focusable, id) {
            self.focus_order.push((id, self.overlay));
        }
        i
    }

    /// Add a container and build its children with `f`.
    pub(crate) fn add_container<R>(&mut self, node: Node, f: impl FnOnce(&mut Ui) -> R) -> (u32, R) {
        let i = self.add_node(node);
        let prev = std::mem::replace(&mut self.parent, i);
        let r = f(self);
        self.parent = prev;
        (i, r)
    }

    pub(crate) fn node_mut(&mut self, i: u32) -> &mut Node {
        &mut self.tree.nodes[i as usize]
    }

    /// The direction of the container children are added to.
    pub(crate) fn parent_dir(&self) -> Dir {
        if self.parent == NONE { Dir::Column } else { self.tree.nodes[self.parent as usize].layout.dir }
    }

    /// Default size of a container created in the current one: in a column
    /// (or stack) it spans the width, in a row it is as wide as its content.
    pub(crate) fn default_container_width(&self) -> Size {
        match self.parent_dir() {
            Dir::Row => Size::Fit,
            Dir::Column | Dir::Stack => Size::FILL,
        }
    }

    /// Build an overlay (popover, menu, dialog, tooltip, toast) drawn above
    /// the window content. `takes_keys` routes keyboard input to it (and Tab
    /// focus stays inside).
    pub(crate) fn overlay<R>(&mut self, id: Id, anchor: Anchor, takes_keys: bool, node: Node, f: impl FnOnce(&mut Ui) -> R) -> (u32, R) {
        let i = self.tree.add(NONE, node);
        let layer = self.tree.roots.len() as u32;
        self.tree.roots.push(Root { node: i, anchor: Some(anchor), layer });
        if takes_keys {
            self.key_owners.push(id);
        }
        let saved = (self.parent, self.overlay, self.layer);
        self.parent = i;
        self.overlay = Some(id);
        self.layer = layer;
        let r = f(self);
        (self.parent, self.overlay, self.layer) = saved;
        (i, r)
    }

    /// Start the content root (runtime).
    pub(crate) fn begin_root(&mut self, node: Node) -> u32 {
        let i = self.tree.add(NONE, node);
        self.tree.roots.push(Root { node: i, anchor: None, layer: 0 });
        self.parent = i;
        i
    }

    pub(crate) fn set_parent(&mut self, i: u32) -> u32 {
        std::mem::replace(&mut self.parent, i)
    }

    // -- Ids and state ------------------------------------------------------------------

    /// A new id for `key` in the current scope; the n-th use of the same
    /// key in a scope gets a distinct id (stable while the order is).
    pub fn make_id(&mut self, key: impl Hash) -> Id {
        let k = Id(0).with(&key).0;
        let n = self.counts.entry((self.scope, k)).or_insert(0);
        let id = self.scope.with((k, *n));
        *n += 1;
        id
    }

    /// The id of a popup menu: scoped like widgets but not counted, so
    /// opening and showing it may happen at different places in `view`.
    pub(crate) fn popup_menu_id(&self, key: impl Hash) -> Id {
        self.scope.with(("popup-menu", key))
    }

    /// Build `f` with ids scoped under `key` (use in loops over items so
    /// each item's widgets keep their state when others are added or removed).
    pub fn push_id<R>(&mut self, key: impl Hash, f: impl FnOnce(&mut Ui) -> R) -> R {
        let scope = self.scope.with(key);
        let prev = std::mem::replace(&mut self.scope, scope);
        let r = f(self);
        self.scope = prev;
        r
    }

    /// Persistent state of type `T` for `id` (created with `T::default()`).
    /// Kept while the widget is shown, and for a while after.
    pub fn state<T: Default + 'static>(&mut self, id: Id) -> &mut T {
        let frame = self.frame;
        let slot = self.memory.entry(id).or_insert_with(|| Slot { value: Box::new(T::default()), used: frame });
        slot.used = frame;
        if !slot.value.is::<T>() {
            slot.value = Box::new(T::default());
        }
        slot.value.downcast_mut::<T>().expect("checked type")
    }

    /// Forget the state of `id` (e.g. reset a text input's undo history).
    pub fn forget(&mut self, id: Id) {
        self.memory.remove(&id);
    }

    /// Animate toward `target` with a spring; returns the current value.
    /// The first call for an id starts at the target (no animation).
    pub fn animate(&mut self, id: Id, target: f32, params: SpringParams) -> f32 {
        self.animate_from(id, target, target, params)
    }

    /// Like `animate`, starting at `from` the first time (appear animations).
    pub fn animate_from(&mut self, id: Id, from: f32, target: f32, params: SpringParams) -> f32 {
        let speed = self.theme.anim_speed;
        let (now, frame) = (self.now, self.frame);
        let slot = self.anims.entry(id).or_insert_with(|| AnimSlot { spring: Spring::new(from, params), time: now, used: frame });
        slot.used = frame;
        if speed <= 0.0 {
            slot.spring.snap(target);
            return target;
        }
        slot.spring.params = SpringParams { stiffness: params.stiffness * speed * speed, damping: params.damping };
        let dt = now.saturating_duration_since(slot.time).as_secs_f32();
        slot.time = now;
        slot.spring.set_target(target);
        slot.spring.step(dt, 0.002);
        if !slot.spring.is_settled() {
            self.animating = true;
        }
        slot.spring.value
    }

    /// 0 → 1 as `on` becomes true (hover and press feedback).
    pub fn animate_bool(&mut self, id: Id, on: bool) -> f32 {
        self.animate(id, if on { 1.0 } else { 0.0 }, SpringParams::FAST)
    }

    // -- Interaction --------------------------------------------------------------------

    /// The response of the box `id` (created with a matching sense).
    pub fn interact(&mut self, id: Id) -> Response {
        let inp = &self.input;
        let rect = self.prev_rects.get(&id).copied().unwrap_or_default();
        let hovered = inp.hovered == Some(id);
        let click = inp.click(id, Button::Left);
        let active = inp.active.map(|a| a.0) == Some(id);
        // The movement before a release in the same frame still counts.
        let moving = (active && inp.dragging) || inp.drag_released == Some(id);
        let mut r = Response {
            id,
            rect,
            hovered,
            pressed: active && hovered && inp.active.is_some_and(|a| a.1 == Button::Left),
            clicked: click.is_some(),
            double_clicked: click.is_some_and(|c| c.count == 2),
            secondary_clicked: inp.click(id, Button::Right).is_some(),
            middle_clicked: inp.click(id, Button::Middle).is_some(),
            drag_started: inp.drag_started == Some(id),
            dragging: active && inp.dragging,
            drag_released: inp.drag_released == Some(id),
            drag_delta: if moving { inp.drag_delta } else { Vec2::ZERO },
            focused: inp.focus == Some(id),
            changed: false,
            submitted: false,
            pointer: click.map(|c| c.pos).or_else(|| inp.click(id, Button::Right).map(|c| c.pos)),
            mods: click.map(|c| c.mods).unwrap_or(inp.mods),
        };
        if r.focused && self.keys_allowed() {
            if self.take_key(Key::Menu, Mods::NONE) || self.take_key(Key::F(10), Mods::SHIFT) {
                r.secondary_clicked = true;
                r.pointer = Some(rect.min() + Vec2::new(8.0, rect.h));
            }
        }
        r
    }

    /// Where the pointer pressed on `id` this frame (sliders, text cursors).
    pub fn press_pos(&self, id: Id) -> Option<(Vec2, u32)> {
        self.input.press(id, Button::Left).map(|c| (c.pos, c.count))
    }

    /// Pointer position (logical window coordinates).
    pub fn pointer(&self) -> Option<Vec2> {
        self.input.pointer
    }

    /// Modifier keys currently held.
    pub fn mods(&self) -> Mods {
        self.input.mods
    }

    /// The rectangle `id` had in the last layout.
    pub fn prev_rect(&self, id: Id) -> Option<Rect> {
        self.prev_rects.get(&id).copied()
    }

    /// The content size of the scrolling box `id` in the last layout.
    pub(crate) fn prev_content(&self, id: Id) -> Option<Vec2> {
        self.prev_content.get(&id).copied()
    }

    pub fn is_focused(&self, id: Id) -> bool {
        self.input.focus == Some(id)
    }

    /// Give keyboard focus to `id` (e.g. a search field when it appears).
    pub fn request_focus(&mut self, id: Id) {
        if self.input.focus != Some(id) {
            self.input.focus = Some(id);
            self.animating = true;
        }
    }

    pub fn clear_focus(&mut self) {
        self.input.focus = None;
    }

    /// The focus ring is shown (focus moved by keyboard).
    pub fn focus_visible(&self) -> bool {
        self.input.focus_visible
    }

    /// Whether keyboard input reaches what is being built (false under an
    /// open menu or dialog).
    pub fn keys_allowed(&self) -> bool {
        self.key_owner.is_none() || self.key_owner == self.overlay
    }

    /// Key presses not yet used by a widget (for the current scope's layer).
    pub fn keys(&self) -> Vec<KeyPress> {
        if !self.keys_allowed() {
            return Vec::new();
        }
        self.input.keys.iter().filter(|(_, used)| !*used).map(|(k, _)| k.clone()).collect()
    }

    /// Mark the first unused press of `key` with exactly `mods` as used and
    /// return true if there was one.
    pub fn take_key(&mut self, key: Key, mods: Mods) -> bool {
        if !self.keys_allowed() {
            return false;
        }
        for (k, used) in self.input.keys.iter_mut() {
            if !*used && k.key == key && k.mods == mods {
                *used = true;
                return true;
            }
        }
        false
    }

    /// `key` was pressed without modifiers (and not used by a widget).
    pub fn key_pressed(&mut self, key: Key) -> bool {
        self.take_key(key, Mods::NONE)
    }

    /// An app keyboard shortcut was pressed, e.g.
    /// `if ui.shortcut(Shortcut::ctrl('s')) { self.save() }`.
    ///
    /// While a text input has the keyboard, the keys it edits with (Ctrl+A,
    /// Ctrl+C, Delete, arrows, plain characters…) are left to it, wherever
    /// in `view` the shortcut is checked; other shortcuts (Ctrl+S, F5,
    /// Escape) still work.
    pub fn shortcut(&mut self, s: Shortcut) -> bool {
        if let Some((owner, multiline)) = self.prev_text_owner {
            if owner == self.overlay && crate::widgets::text_input::is_editing_key(s.key, s.mods, multiline) {
                return false;
            }
        }
        self.take_key(s.key, s.mods)
    }

    /// A focused text input is being built (it gets the editing keys; a
    /// multi-line one also Enter, Tab and vertical moves).
    pub(crate) fn mark_text_focus(&mut self, multiline: bool) {
        self.text_owner = Some((self.overlay, multiline));
    }

    /// Use all unused key presses with `f` (text input); `f` returns whether
    /// it used the key.
    pub(crate) fn consume_keys(&mut self, mut f: impl FnMut(&mut Ui, &KeyPress) -> bool) {
        if !self.keys_allowed() {
            return;
        }
        let mut i = 0;
        while i < self.input.keys.len() {
            if !self.input.keys[i].1 {
                let k = self.input.keys[i].0.clone();
                if f(self, &k) {
                    self.input.keys[i].1 = true;
                }
            }
            i += 1;
        }
    }

    /// Scroll deltas aimed at `id` this frame: (delta, wheel, kinetic end).
    pub(crate) fn take_scrolls(&mut self, id: Id) -> Vec<(Vec2, bool, bool)> {
        let mut out = Vec::new();
        self.input.scrolls.retain(|s| {
            if s.target == id {
                out.push((s.delta, s.wheel, s.kinetic_end));
                false
            } else {
                true
            }
        });
        out
    }

    /// Scrolling (wheel or touchpad, logical px; positive y scrolls toward
    /// the end) aimed at the box `id` this frame, which needs a
    /// [`Sense`](crate::Sense) with `scroll` (a zoomable canvas). Taking it
    /// leaves nothing for the next call.
    pub fn scroll_delta(&mut self, id: Id) -> Vec2 {
        self.take_scrolls(id).into_iter().fold(Vec2::ZERO, |a, (d, _, _)| a + d)
    }

    pub(crate) fn take_paste(&mut self, id: Id) -> Option<String> {
        let i = self.input.pastes.iter().position(|(t, _)| *t == id)?;
        Some(self.input.pastes.remove(i).1)
    }

    /// Show `icon` as the pointer while it is over what is being built.
    pub fn set_cursor(&mut self, icon: CursorIcon) {
        self.cursor = icon;
    }

    // -- Window and app services --------------------------------------------------------

    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    /// Replace the theme (recolor everything from the next frame).
    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
        self.animating = true;
    }

    /// The fonts and text layout cache (for measuring text in custom widgets).
    pub fn fonts(&mut self) -> &mut Fonts {
        &mut self.fonts
    }

    /// Window size in logical pixels.
    pub fn window_size(&self) -> Vec2 {
        self.window.size
    }

    /// Device pixels per logical pixel.
    pub fn scale(&self) -> f32 {
        self.window.scale
    }

    pub fn is_maximized(&self) -> bool {
        self.window.maximized
    }

    /// The window has keyboard focus.
    pub fn window_focused(&self) -> bool {
        self.input.window_focused
    }

    /// The window title (header bar and task switcher).
    pub fn title(&mut self, title: impl Into<String>) {
        let t = title.into();
        if t != self.title {
            self.out.title = Some(t.clone());
            self.title = t;
        }
    }

    pub fn close_window(&mut self) {
        self.out.close = true;
    }

    pub fn toggle_maximize(&mut self) {
        self.out.toggle_maximize = true;
    }

    pub fn set_fullscreen(&mut self, on: bool) {
        self.out.fullscreen = Some(on);
    }

    pub fn is_fullscreen(&self) -> bool {
        self.window.fullscreen
    }

    /// Put text on the clipboard.
    pub fn copy_text(&mut self, text: impl Into<String>) {
        self.out.copy = Some((text.into(), false));
    }

    /// Set the primary selection (middle-click paste).
    pub fn set_primary_selection(&mut self, text: impl Into<String>) {
        self.out.copy = Some((text.into(), true));
    }

    /// Ask for the clipboard's text; it arrives in a later frame for `id`
    /// (see [`Ui::pasted`]).
    pub fn request_paste(&mut self, id: Id, primary: bool) {
        self.out.paste = Some((id, primary));
    }

    /// Clipboard text that arrived for `id`.
    pub fn pasted(&mut self, id: Id) -> Option<String> {
        self.take_paste(id)
    }

    /// Render another frame right after this one (custom animations).
    pub fn request_repaint(&mut self) {
        self.animating = true;
    }

    /// Render a frame after `d` (timers, clocks, polling).
    pub fn request_repaint_after(&mut self, d: Duration) {
        let at = self.now + d;
        self.repaint_at = Some(self.repaint_at.map_or(at, |t| t.min(at)));
    }

    /// A handle background threads use to wake the UI (it then renders a
    /// frame, so `view` can pick up their results).
    pub fn waker(&self) -> crate::Waker {
        self.waker.clone()
    }

    /// Time of the current frame.
    pub fn now(&self) -> Instant {
        self.now
    }

    /// Print a line to the test event log (stdout when `LUMEN_UI_TEST` is
    /// set; nothing otherwise).
    pub fn log(&self, line: &str) {
        self.test.event(line);
    }

    /// Build as if inside `overlay` (key routing); returns the previous one.
    pub(crate) fn swap_overlay(&mut self, overlay: Option<Id>) -> Option<Id> {
        std::mem::replace(&mut self.overlay, overlay)
    }

    /// Focus the first focusable widget of the overlay `owner` once this
    /// pass is built (dialogs opening).
    pub(crate) fn focus_first_in(&mut self, owner: Id) {
        self.focus_first = Some(owner);
    }

    /// The previous frame's focus order (for list keyboard handling).
    pub(crate) fn _focus_order(&self) -> &[(Id, Option<Id>)] {
        &self.prev_focus_order
    }

    /// Helper for widgets: a box with an id, sense and label for tests.
    pub(crate) fn widget_node(&mut self, id: Id, kind: &'static str, label: Option<&str>, layout: Layout, sense: Sense) -> Node {
        let _ = self;
        let mut n = Node::new(layout);
        n.id = Some(id);
        n.kind = kind;
        n.label = label.map(str::to_string);
        n.sense = sense;
        n
    }

}
