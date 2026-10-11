//! Wayland event handlers and input handling.

use std::io::Write;
use std::os::fd::OwnedFd;
use std::time::{Duration, Instant};

use sctk::compositor::CompositorHandler;
use sctk::data_device_manager::data_device::DataDeviceHandler;
use sctk::data_device_manager::data_offer::{DataOfferHandler, DragOffer};
use sctk::data_device_manager::data_source::DataSourceHandler;
use sctk::data_device_manager::WritePipe;
use sctk::output::{OutputHandler, OutputState};
use sctk::primary_selection::device::PrimarySelectionDeviceHandler;
use sctk::primary_selection::selection::PrimarySelectionSourceHandler;
use sctk::reexports::client::protocol::wl_data_device::WlDataDevice;
use sctk::reexports::client::protocol::wl_data_device_manager::DndAction;
use sctk::reexports::client::protocol::wl_data_source::WlDataSource;
use sctk::reexports::client::protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_surface};
use sctk::reexports::client::{Connection, Dispatch, Proxy, QueueHandle};
use sctk::reexports::protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::Shape;
use sctk::reexports::protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1;
use sctk::reexports::protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::{self, WpFractionalScaleV1};
use sctk::reexports::protocols::wp::primary_selection::zv1::client::zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1;
use sctk::reexports::protocols::wp::primary_selection::zv1::client::zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1;
use sctk::reexports::protocols::wp::viewporter::client::wp_viewport::WpViewport;
use sctk::reexports::protocols::wp::viewporter::client::wp_viewporter::WpViewporter;
use sctk::reexports::protocols::xdg::shell::client::xdg_toplevel::ResizeEdge;
use sctk::registry::{ProvidesRegistryState, RegistryState};
use sctk::seat::keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers};
use sctk::seat::pointer::{PointerEvent, PointerEventKind, PointerHandler, BTN_LEFT, BTN_MIDDLE, BTN_RIGHT};
use sctk::seat::{Capability, SeatHandler, SeatState};
use sctk::shell::xdg::window::{Window, WindowConfigure, WindowHandler};
use sctk::shell::WaylandSurface;
use sctk::{
    delegate_compositor, delegate_data_device, delegate_keyboard, delegate_output, delegate_pointer, delegate_primary_selection,
    delegate_registry, delegate_seat, delegate_xdg_shell, delegate_xdg_window, registry_handlers,
};
use smithay_client_toolkit as sctk;

use lumen_term::input::{self, Key, Mods, MouseAction, MouseButton};
use lumen_term::layout::{Dir, Rect};
use lumen_term::select::{Pos, SelMode, Selection};

use crate::app::{App, Drag, Hover};

impl CompositorHandler for App {
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &wl_surface::WlSurface, factor: i32) {
        if surface == self.window.wl_surface() && self.fractional.is_none() {
            let f = factor.max(1) as u32;
            if f != self.int_scale {
                self.int_scale = f;
                self.layout_changed();
            }
        }
    }

    fn transform_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: wl_output::Transform) {}

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {
        self.frame_pending = false;
        if self.needs_render {
            self.render();
        }
    }

    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl WindowHandler for App {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Window) {
        self.exit = true;
    }

    fn configure(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Window, configure: WindowConfigure, _serial: u32) {
        use sctk::reexports::csd_frame::WindowState;
        self.maximized = configure.state.contains(WindowState::MAXIMIZED);
        self.fullscreen = configure.state.contains(WindowState::FULLSCREEN);
        self.tiled = configure.state.intersects(WindowState::TILED);
        self.kb_focus = configure.state.contains(WindowState::ACTIVATED) || self.kb_focus;
        if let (Some(w), Some(h)) = configure.new_size {
            self.size = (w.get(), h.get());
        }
        let first = !self.configured;
        self.configured = true;
        if first {
            self.new_tab();
        } else {
            self.layout_changed();
        }
        self.full_redraw = true;
        self.frame_pending = false;
        self.render();
    }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(&mut self, _: &Connection, qh: &QueueHandle<Self>, seat: wl_seat::WlSeat, capability: Capability) {
        if self.seat.as_ref().is_some_and(|s| s != &seat) {
            return;
        }
        if self.seat.is_none() {
            self.seat = Some(seat.clone());
            if let Some(m) = &self.ddm {
                self.data_device = Some(m.get_data_device(qh, &seat));
            }
            if let Some(m) = &self.psm {
                self.primary_device = Some(m.get_selection_device(qh, &seat));
            }
        }
        match capability {
            Capability::Keyboard if self.keyboard.is_none() => {
                let lh = self.lh.clone();
                match self.seat_state.get_keyboard_with_repeat(
                    qh,
                    &seat,
                    None,
                    lh,
                    Box::new(|app: &mut App, _kb: &wl_keyboard::WlKeyboard, event: KeyEvent| {
                        app.on_key(event, true);
                    }),
                ) {
                    Ok(k) => self.keyboard = Some(k),
                    Err(e) => eprintln!("lumen-term: keyboard: {e}"),
                }
            }
            Capability::Pointer if self.pointer.is_none() => {
                if let Ok(p) = self.seat_state.get_pointer(qh, &seat) {
                    if let Some(m) = &self.cursor_shape_mgr {
                        self.cursor_device = Some(m.get_shape_device(&p, qh));
                    }
                    self.pointer = Some(p);
                }
            }
            _ => {}
        }
    }

    fn remove_capability(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat, capability: Capability) {
        match capability {
            Capability::Keyboard => {
                if let Some(k) = self.keyboard.take() {
                    if k.version() >= 3 {
                        k.release();
                    }
                }
            }
            Capability::Pointer => {
                if let Some(d) = self.cursor_device.take() {
                    d.destroy();
                }
                if let Some(p) = self.pointer.take() {
                    if p.version() >= 3 {
                        p.release();
                    }
                }
            }
            _ => {}
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, seat: wl_seat::WlSeat) {
        if self.seat.as_ref() == Some(&seat) {
            self.seat = None;
            self.keyboard = None;
            self.pointer = None;
            self.cursor_device = None;
            self.data_device = None;
            self.primary_device = None;
        }
    }
}

fn key_of(event: &KeyEvent) -> Option<Key> {
    let k = event.keysym;
    Some(match k {
        Keysym::Return => Key::Enter,
        Keysym::KP_Enter => Key::KpEnter,
        Keysym::Tab | Keysym::ISO_Left_Tab => Key::Tab,
        Keysym::BackSpace => Key::Backspace,
        Keysym::Escape => Key::Escape,
        Keysym::Up | Keysym::KP_Up => Key::Up,
        Keysym::Down | Keysym::KP_Down => Key::Down,
        Keysym::Left | Keysym::KP_Left => Key::Left,
        Keysym::Right | Keysym::KP_Right => Key::Right,
        Keysym::Home | Keysym::KP_Home => Key::Home,
        Keysym::End | Keysym::KP_End => Key::End,
        Keysym::Page_Up | Keysym::KP_Page_Up => Key::PageUp,
        Keysym::Page_Down | Keysym::KP_Page_Down => Key::PageDown,
        Keysym::Insert | Keysym::KP_Insert => Key::Insert,
        Keysym::Delete | Keysym::KP_Delete => Key::Delete,
        _ => {
            let raw = k.raw();
            if (Keysym::F1.raw()..=Keysym::F12.raw()).contains(&raw) {
                Key::F((raw - Keysym::F1.raw() + 1) as u8)
            } else {
                Key::Text(event.utf8.clone()?)
            }
        }
    })
}

impl App {
    /// Shortcuts handled by the terminal itself; returns true if used.
    fn shortcut(&mut self, event: &KeyEvent) -> bool {
        let m = self.mods;
        let k = event.keysym;
        // Ctrl+Shift shortcuts arrive as upper case letters.
        let lower = if (0x41..=0x5a).contains(&k.raw()) { Keysym::from(k.raw() + 0x20) } else { k };
        if m.ctrl && m.shift && !m.alt {
            match lower {
                Keysym::c => self.copy_selection(true),
                Keysym::v => self.paste(false),
                Keysym::t => self.new_tab(),
                Keysym::w => self.close_focused(),
                Keysym::f => self.toggle_search(),
                Keysym::e => self.split(true),
                Keysym::o => self.split(false),
                Keysym::Page_Up => {
                    if self.active > 0 {
                        self.tabs.swap(self.active, self.active - 1);
                        self.active -= 1;
                        self.request_render();
                    }
                }
                Keysym::Page_Down => {
                    if self.active + 1 < self.tabs.len() {
                        self.tabs.swap(self.active, self.active + 1);
                        self.active += 1;
                        self.request_render();
                    }
                }
                Keysym::Up => self.scroll_focused(1.0),
                Keysym::Down => self.scroll_focused(-1.0),
                _ => return false,
            }
            return true;
        }
        if m.ctrl && !m.alt && !m.shift {
            match k {
                Keysym::Page_Up => self.select_tab((self.active + self.tabs.len() - 1) % self.tabs.len().max(1)),
                Keysym::Page_Down => self.select_tab((self.active + 1) % self.tabs.len().max(1)),
                Keysym::equal | Keysym::plus | Keysym::KP_Add => self.zoom(1.0),
                Keysym::minus | Keysym::KP_Subtract => self.zoom(-1.0),
                Keysym::_0 | Keysym::KP_0 => {
                    self.font_size = self.cfg.font.size;
                    self.zoom(0.0)
                }
                _ => return false,
            }
            return true;
        }
        if m.shift && !m.ctrl && !m.alt {
            match k {
                Keysym::Page_Up => self.scroll_page(1.0),
                Keysym::Page_Down => self.scroll_page(-1.0),
                Keysym::Home => self.scroll_focused(f64::MAX),
                Keysym::End => {
                    if let Some(p) = self.focused_mut() {
                        p.scroll_to_bottom();
                    }
                    self.request_render();
                }
                _ => return false,
            }
            return true;
        }
        if m.alt && !m.ctrl && !m.shift {
            let dir = match k {
                Keysym::Left => Dir::Left,
                Keysym::Right => Dir::Right,
                Keysym::Up => Dir::Up,
                Keysym::Down => Dir::Down,
                _ => {
                    // Alt+1..9 select tabs.
                    let raw = k.raw();
                    if (Keysym::_1.raw()..=Keysym::_9.raw()).contains(&raw) && self.tabs.len() > 1 {
                        self.select_tab((raw - Keysym::_1.raw()) as usize);
                        return true;
                    }
                    return false;
                }
            };
            if self.tab().is_some_and(|t| t.root.panes().len() > 1) {
                self.focus_dir(dir);
                return true;
            }
            return false;
        }
        if k == Keysym::F11 && !m.any() {
            if self.fullscreen {
                self.window.unset_fullscreen();
            } else {
                self.window.set_fullscreen(None);
            }
            return true;
        }
        false
    }

    fn zoom(&mut self, d: f32) {
        self.font_size = (self.font_size + d).clamp(6.0, 72.0);
        self.apply_font_size();
        self.layout_changed();
    }

    fn scroll_focused(&mut self, lines: f64) {
        if let Some(p) = self.focused_mut() {
            p.scroll_by(lines);
        }
        self.request_render();
    }

    fn scroll_page(&mut self, dir: f64) {
        let rows = self.focused_mut().map(|p| p.rows).unwrap_or(1) as f64;
        self.scroll_focused(dir * (rows - 1.0).max(1.0));
    }

    /// Typing in the search bar; returns true if the key was used.
    fn search_key(&mut self, event: &KeyEvent) -> bool {
        let m = self.mods;
        let Some(p) = self.focused_mut() else { return false };
        let Some(s) = p.search.as_mut() else { return false };
        match event.keysym {
            Keysym::Escape => {
                p.search = None;
            }
            Keysym::Return | Keysym::KP_Enter => p.search_step(!m.shift),
            Keysym::Up => p.search_step(true),
            Keysym::Down => p.search_step(false),
            Keysym::BackSpace => {
                s.query.pop();
                s.stale = true;
                s.current = None;
                p.search_step(true);
            }
            _ => match &event.utf8 {
                Some(t) if !m.ctrl && !t.chars().any(|c| c.is_control()) => {
                    s.query.push_str(t);
                    s.stale = true;
                    s.current = None;
                    p.search_step(true);
                }
                _ => return false,
            },
        }
        self.request_render();
        true
    }

    pub fn on_key(&mut self, event: KeyEvent, _repeat: bool) {
        if self.shortcut(&event) {
            return;
        }
        if self.search_key(&event) {
            return;
        }
        let mods = self.mods;
        let Some(key) = key_of(&event) else { return };
        let Some(p) = self.focused_mut() else { return };
        if let Some(bytes) = input::encode_key(&key, mods, &p.term.term.modes) {
            p.write(&bytes);
            p.scroll_to_bottom();
            p.cursor_moved = Instant::now();
            // Typing replaces a selection's purpose; keep it unless the view changes.
            self.request_render();
        }
    }
}

impl KeyboardHandler for App {
    fn enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: &wl_surface::WlSurface, serial: u32, _: &[u32], _: &[Keysym]) {
        self.kb_focus = true;
        self.last_serial = serial;
        if let Some(p) = self.focused_mut() {
            p.cursor_moved = Instant::now();
            if p.term.term.modes.focus_events {
                p.write(b"\x1b[I");
            }
        }
        self.request_render();
    }

    fn leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: &wl_surface::WlSurface, _: u32) {
        self.kb_focus = false;
        self.mods = Mods::default();
        if let Some(p) = self.focused_mut() {
            if p.term.term.modes.focus_events {
                p.write(b"\x1b[O");
            }
        }
        self.request_render();
    }

    fn press_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, serial: u32, event: KeyEvent) {
        self.last_serial = serial;
        self.on_key(event, false);
    }

    fn release_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, _: KeyEvent) {}

    fn update_modifiers(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, m: Modifiers, _: u32) {
        self.mods = Mods { shift: m.shift, alt: m.alt, ctrl: m.ctrl, logo: m.logo };
    }
}

/// Edges for interactive resizing near the window border (logical px).
fn resize_edge(size: (u32, u32), x: f64, y: f64) -> Option<ResizeEdge> {
    const B: f64 = 6.0;
    let (w, h) = (size.0 as f64, size.1 as f64);
    let (l, r, t, b) = (x < B, x > w - B, y < B, y > h - B);
    Some(match (l, r, t, b) {
        (true, _, true, _) => ResizeEdge::TopLeft,
        (_, true, true, _) => ResizeEdge::TopRight,
        (true, _, _, true) => ResizeEdge::BottomLeft,
        (_, true, _, true) => ResizeEdge::BottomRight,
        (true, ..) => ResizeEdge::Left,
        (_, true, ..) => ResizeEdge::Right,
        (_, _, true, _) => ResizeEdge::Top,
        (_, _, _, true) => ResizeEdge::Bottom,
        _ => return None,
    })
}

impl App {
    fn set_cursor(&self, shape: Shape) {
        if let Some(d) = &self.cursor_device {
            d.set_shape(self.ptr_serial, shape);
        }
    }

    /// The pane (and its content rectangle) under a buffer-pixel point.
    fn pane_at(&self, bx: f32, by: f32) -> Option<(u32, Rect, Rect)> {
        self.pane_areas().into_iter().find(|(_, r)| r.contains(bx, by)).map(|(id, r)| (id, r, self.content_rect(r)))
    }

    /// Grid position (absolute line) under a buffer-pixel point of a pane.
    fn grid_pos(&self, id: u32, content: Rect, bx: f32, by: f32) -> Option<Pos> {
        let p = self.panes.get(&id)?;
        let cell = self.fonts.cell;
        let grid = p.term.term.grid();
        let col = ((bx - content.x) / cell.w).floor().clamp(0.0, (grid.cols - 1) as f32) as usize;
        // Rows above/below the view while dragging still map to lines.
        let top = grid.history() as f64 - p.scroll.offset.min(grid.history() as f64);
        let row = ((by - content.y) as f64 / cell.h as f64 + top).floor();
        let max = (grid.total_lines() - 1) as f64;
        let line = row.clamp(0.0, max) as u64 + grid.dropped;
        Some(Pos { line, col })
    }

    fn update_hover(&mut self, bx: f32, by: f32) {
        let mut h = Hover::default();
        let hits = &self.header_hits;
        if hits.close.is_some_and(|r| r.contains(bx, by)) {
            h.close = true;
        }
        if hits.new_tab.is_some_and(|r| r.contains(bx, by)) {
            h.new_tab = true;
        }
        for (i, (r, c)) in hits.tabs.iter().enumerate() {
            if r.contains(bx, by) {
                h.tab = Some(i);
                if c.contains(bx, by) {
                    h.tab_close = Some(i);
                }
            }
        }
        if h != self.hover {
            self.hover = h;
            self.request_render();
        }
        // Hyperlinks: highlighted under the pointer.
        let mut link = 0;
        if by >= self.header_height() {
            if let Some((id, _, content)) = self.pane_at(bx, by) {
                if let Some(pos) = self.grid_pos(id, content, bx, by) {
                    if let Some(p) = self.panes.get(&id) {
                        let g = p.term.term.grid();
                        if let Some(row) = pos.line.checked_sub(g.dropped).and_then(|i| g.line(i as usize)) {
                            link = row.cells.get(pos.col).map(|c| c.link).unwrap_or(0);
                        }
                    }
                }
            }
        }
        if link != self.hovered_link {
            self.hovered_link = link;
            self.request_render();
        }
    }

    fn pointer_shape(&self, bx: f32, by: f32) -> Shape {
        if !self.maximized && !self.fullscreen && !self.tiled {
            if let Some(e) = resize_edge(self.size, self.ptr.0, self.ptr.1) {
                return match e {
                    ResizeEdge::Left | ResizeEdge::Right => Shape::EwResize,
                    ResizeEdge::Top | ResizeEdge::Bottom => Shape::NsResize,
                    ResizeEdge::TopLeft | ResizeEdge::BottomRight => Shape::NwseResize,
                    _ => Shape::NeswResize,
                };
            }
        }
        if by < self.header_height() {
            return Shape::Default;
        }
        let s = self.scale();
        if self.tab().is_some() {
            let top = self.header_height();
            let (bw, bh) = self.buffer_size();
            let mut divs = Vec::new();
            if let Some(t) = self.tab() {
                t.root.dividers(Rect::new(0.0, top, bw as f32, bh as f32 - top), self.gap(), &mut Vec::new(), &mut divs);
            }
            for d in divs {
                let grow = 4.0 * s;
                let r = Rect::new(d.rect.x - grow, d.rect.y - grow, d.rect.w + 2.0 * grow, d.rect.h + 2.0 * grow);
                if r.contains(bx, by) {
                    return if d.horizontal { Shape::ColResize } else { Shape::RowResize };
                }
            }
        }
        if self.hovered_link != 0 && self.mods.ctrl {
            return Shape::Pointer;
        }
        Shape::Text
    }

    fn mouse_report(&mut self, id: u32, content: Rect, bx: f32, by: f32, button: MouseButton, action: MouseAction, held: bool) -> bool {
        let mods = self.mods;
        if mods.shift {
            return false;
        }
        let cell = self.fonts.cell;
        let Some(p) = self.panes.get_mut(&id) else { return false };
        let col = ((bx - content.x) / cell.w).floor().max(0.0) as usize;
        let row = ((by - content.y) / cell.h as f32).floor().max(0.0) as usize;
        let (col, row) = (col.min(p.cols.saturating_sub(1)), row.min(p.rows.saturating_sub(1)));
        match input::encode_mouse(button, action, held, col, row, mods, &p.term.term.modes) {
            Some(bytes) => {
                p.write(&bytes);
                true
            }
            None => false,
        }
    }

    fn on_press(&mut self, button: u32, serial: u32) {
        self.last_serial = serial;
        let s = self.scale();
        let (bx, by) = (self.ptr.0 as f32 * s, self.ptr.1 as f32 * s);
        // Window border: interactive resize.
        if button == BTN_LEFT && !self.maximized && !self.fullscreen && !self.tiled {
            if let (Some(e), Some(seat)) = (resize_edge(self.size, self.ptr.0, self.ptr.1), &self.seat) {
                self.window.resize(seat, serial, e);
                return;
            }
        }
        if by < self.header_height() {
            self.header_press(button, serial, bx, by);
            return;
        }
        // Split dividers.
        if button == BTN_LEFT {
            let top = self.header_height();
            let (bw, bh) = self.buffer_size();
            let mut divs = Vec::new();
            if let Some(t) = self.tab() {
                t.root.dividers(Rect::new(0.0, top, bw as f32, bh as f32 - top), self.gap(), &mut Vec::new(), &mut divs);
            }
            let grow = 4.0 * s;
            if let Some(d) = divs.into_iter().find(|d| Rect::new(d.rect.x - grow, d.rect.y - grow, d.rect.w + 2.0 * grow, d.rect.h + 2.0 * grow).contains(bx, by)) {
                self.drag = Drag::Divider(d);
                return;
            }
        }
        let Some((id, _, content)) = self.pane_at(bx, by) else { return };
        if self.focused_pane() != Some(id) {
            self.tabs[self.active].focus = id;
            self.request_render();
        }
        let mb = match button {
            BTN_LEFT => MouseButton::Left,
            BTN_MIDDLE => MouseButton::Middle,
            BTN_RIGHT => MouseButton::Right,
            _ => return,
        };
        if self.mouse_report(id, content, bx, by, mb, MouseAction::Press, true) {
            self.drag = Drag::Report(id);
            return;
        }
        match button {
            BTN_LEFT => {
                // Click counting: double = word, triple = line.
                let now = Instant::now();
                let (t, n, at) = self.clicks;
                let near = (at.0 - self.ptr.0).abs() < 4.0 && (at.1 - self.ptr.1).abs() < 4.0;
                let n = if now.duration_since(t) < Duration::from_millis(400) && near { n % 3 + 1 } else { 1 };
                self.clicks = (now, n, self.ptr);
                let Some(pos) = self.grid_pos(id, content, bx, by) else { return };
                let shift = self.mods.shift;
                let Some(p) = self.panes.get_mut(&id) else { return };
                let mode = match n {
                    2 => SelMode::Word,
                    3 => SelMode::Line,
                    _ => SelMode::Char,
                };
                match (&mut p.selection, shift) {
                    (Some(sel), true) => sel.head = pos,
                    _ => p.selection = Some(Selection::new(pos, mode)),
                }
                p.selecting = true;
                self.drag = Drag::Select(id);
                self.request_render();
            }
            BTN_MIDDLE => self.paste(true),
            _ => {}
        }
    }

    fn header_press(&mut self, button: u32, serial: u32, bx: f32, by: f32) {
        let hits = self.header_hits.clone();
        if button == BTN_LEFT {
            if hits.close.is_some_and(|r| r.contains(bx, by)) {
                self.exit = true;
                return;
            }
            if hits.new_tab.is_some_and(|r| r.contains(bx, by)) {
                self.new_tab();
                return;
            }
            for (i, (r, c)) in hits.tabs.iter().enumerate() {
                if c.contains(bx, by) && (i == self.active || self.hover.tab == Some(i)) {
                    self.close_tab(i);
                    return;
                }
                if r.contains(bx, by) {
                    self.select_tab(i);
                    return;
                }
            }
            // Empty header: double click maximizes, drag moves.
            let now = Instant::now();
            let (t, n, at) = self.clicks;
            let near = (at.0 - self.ptr.0).abs() < 4.0 && (at.1 - self.ptr.1).abs() < 4.0;
            if now.duration_since(t) < Duration::from_millis(400) && near && n == 1 {
                self.clicks = (now, 2, self.ptr);
                if self.maximized {
                    self.window.unset_maximized();
                } else {
                    self.window.set_maximized();
                }
                return;
            }
            self.clicks = (now, 1, self.ptr);
            if let Some(seat) = &self.seat {
                self.window.move_(seat, serial);
            }
        } else if button == BTN_MIDDLE {
            for (i, (r, _)) in hits.tabs.iter().enumerate() {
                if r.contains(bx, by) {
                    self.close_tab(i);
                    return;
                }
            }
        } else if button == BTN_RIGHT {
            if let Some(seat) = &self.seat {
                self.window.show_window_menu(seat, serial, (self.ptr.0 as i32, self.ptr.1 as i32));
            }
        }
    }

    fn on_release(&mut self, button: u32) {
        let s = self.scale();
        let (bx, by) = (self.ptr.0 as f32 * s, self.ptr.1 as f32 * s);
        match std::mem::replace(&mut self.drag, Drag::None) {
            Drag::Select(id) => {
                let copy_clip = self.cfg.behavior.copy_on_select;
                if let Some(p) = self.panes.get_mut(&id) {
                    p.selecting = false;
                    if p.selection.is_some_and(|s| s.is_empty()) {
                        p.selection = None;
                    }
                }
                self.copy_selection(false);
                if copy_clip {
                    self.copy_selection(true);
                }
                self.request_render();
            }
            Drag::Report(id) => {
                if let Some((_, _, content)) = self.pane_at(bx, by).filter(|(p, ..)| *p == id).or_else(|| {
                    self.pane_areas().into_iter().find(|(p, _)| *p == id).map(|(p, r)| (p, r, self.content_rect(r)))
                }) {
                    let mb = match button {
                        BTN_LEFT => MouseButton::Left,
                        BTN_MIDDLE => MouseButton::Middle,
                        _ => MouseButton::Right,
                    };
                    self.mouse_report(id, content, bx, by, mb, MouseAction::Release, false);
                }
            }
            _ => {}
        }
    }

    fn on_motion(&mut self) {
        let s = self.scale();
        let (bx, by) = (self.ptr.0 as f32 * s, self.ptr.1 as f32 * s);
        self.update_hover(bx, by);
        self.set_cursor(self.pointer_shape(bx, by));
        match self.drag.clone() {
            Drag::Divider(d) => {
                let ratio = if d.horizontal { (bx - d.area.x) / d.area.w } else { (by - d.area.y) / d.area.h };
                self.tabs[self.active].root.set_ratio(&d.path, ratio);
                self.layout_changed();
            }
            Drag::Select(id) => {
                let content = self.pane_areas().into_iter().find(|(p, _)| *p == id).map(|(_, r)| self.content_rect(r));
                if let Some(content) = content {
                    if let Some(pos) = self.grid_pos(id, content, bx, by) {
                        if let Some(p) = self.panes.get_mut(&id) {
                            if let Some(sel) = &mut p.selection {
                                sel.head = pos;
                            }
                            // Dragging past the edges scrolls.
                            if by < content.y {
                                p.scroll_by(1.0);
                            } else if by > content.y + content.h {
                                p.scroll_by(-1.0);
                            }
                        }
                        self.request_render();
                    }
                }
            }
            Drag::Report(id) => {
                if let Some((pid, _, content)) = self.pane_at(bx, by) {
                    if pid == id {
                        self.mouse_report(id, content, bx, by, MouseButton::Left, MouseAction::Motion, true);
                    }
                }
            }
            Drag::None => {
                if let Some((id, _, content)) = self.pane_at(bx, by) {
                    if by >= self.header_height() {
                        self.mouse_report(id, content, bx, by, MouseButton::None, MouseAction::Motion, false);
                    }
                }
            }
        }
    }

    fn on_axis(&mut self, dx: f64, dy: f64, discrete: i32, finger: bool, stop: bool) {
        let s = self.scale();
        let (bx, by) = (self.ptr.0 as f32 * s, self.ptr.1 as f32 * s);
        let Some((id, _, content)) = self.pane_at(bx, by) else { return };
        let _ = dx;
        let cell_h = self.fonts.cell.h as f64 / s as f64;
        // Lines to scroll (positive = back in history).
        let lines = if discrete != 0 {
            -(discrete as f64) * self.cfg.scroll.wheel_lines as f64
        } else {
            -dy / cell_h
        };
        let Some(p) = self.panes.get(&id) else { return };
        let modes = p.term.term.modes;
        let alt = p.term.term.alt_active;
        // Applications with mouse reporting get wheel events.
        if modes.mouse != lumen_term::term::MouseMode::None && !self.mods.shift {
            let n = lines.abs().round().max(1.0) as usize;
            let b = if lines > 0.0 { MouseButton::WheelUp } else { MouseButton::WheelDown };
            if lines.abs() > 0.01 {
                for _ in 0..n.min(10) {
                    self.mouse_report(id, content, bx, by, b, MouseAction::Press, false);
                }
            }
            return;
        }
        // Full-screen programs without mouse support: arrows (alternate scroll).
        if alt && modes.alternate_scroll {
            let n = lines.abs().round() as usize;
            let key = if lines > 0.0 { Key::Up } else { Key::Down };
            if let Some(p) = self.panes.get_mut(&id) {
                let bytes = input::encode_key(&key, Mods::default(), &p.term.term.modes).unwrap_or_default();
                for _ in 0..n.min(30) {
                    p.write(&bytes);
                }
            }
            return;
        }
        let now = Instant::now();
        let (last, vel) = self.finger;
        let Some(p) = self.panes.get_mut(&id) else { return };
        if finger {
            let max = p.history() as f64;
            if stop {
                // Kinetic continuation of a flick, if the fingers were moving.
                if now.duration_since(last) < Duration::from_millis(80) && vel.abs() > 4.0 {
                    p.scroll.velocity = vel.clamp(-400.0, 400.0);
                }
                self.finger = (now, 0.0);
            } else {
                p.scroll.velocity = 0.0;
                p.scroll.target = (p.scroll.target + lines).clamp(0.0, max);
                p.scroll.offset = p.scroll.target;
                p.scroll.moved = Some(now);
                // Track the speed (lines per second, smoothed).
                let dt = now.duration_since(last).as_secs_f64().clamp(0.004, 0.1);
                let v = lines / dt;
                self.finger = (now, if now.duration_since(last) > Duration::from_millis(100) { v } else { vel * 0.6 + v * 0.4 });
            }
        } else {
            p.scroll_by(lines);
        }
        self.request_render();
    }
}

impl PointerHandler for App {
    fn pointer_frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_pointer::WlPointer, events: &[PointerEvent]) {
        for e in events {
            if &e.surface != self.window.wl_surface() {
                continue;
            }
            self.ptr = e.position;
            match e.kind {
                PointerEventKind::Enter { serial } => {
                    self.ptr_serial = serial;
                    self.on_motion();
                }
                PointerEventKind::Leave { .. } => {
                    self.hover = Hover::default();
                    self.request_render();
                }
                PointerEventKind::Motion { .. } => self.on_motion(),
                PointerEventKind::Press { button, serial, .. } => self.on_press(button, serial),
                PointerEventKind::Release { button, .. } => self.on_release(button),
                PointerEventKind::Axis { horizontal, vertical, source, .. } => {
                    let finger = matches!(source, Some(wl_pointer::AxisSource::Finger));
                    let stop = vertical.stop;
                    self.on_axis(horizontal.absolute, vertical.absolute, vertical.discrete, finger, stop);
                }
            }
        }
    }
}

impl DataDeviceHandler for App {
    fn enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice, _: f64, _: f64, _: &wl_surface::WlSurface) {}
    fn leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {}
    fn motion(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice, _: f64, _: f64) {}
    fn selection(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {}
    fn drop_performed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {}
}

impl DataOfferHandler for App {
    fn source_actions(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &mut DragOffer, _: DndAction) {}
    fn selected_action(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &mut DragOffer, _: DndAction) {}
}

fn send_text(text: String, pipe: WritePipe) {
    let fd: OwnedFd = pipe.into();
    // A thread, so a slow reader (or ourselves) never blocks the loop.
    std::thread::spawn(move || {
        let mut f = std::fs::File::from(fd);
        let _ = f.write_all(text.as_bytes());
    });
}

impl DataSourceHandler for App {
    fn accept_mime(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource, _: Option<String>) {}
    fn send_request(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource, _mime: String, pipe: WritePipe) {
        send_text(self.clip_text.clone(), pipe);
    }
    fn cancelled(&mut self, _: &Connection, _: &QueueHandle<Self>, source: &WlDataSource) {
        if self.copy_source.as_ref().is_some_and(|s| s.inner() == source) {
            self.copy_source = None;
        }
    }
    fn dnd_dropped(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource) {}
    fn dnd_finished(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource) {}
    fn action(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource, _: DndAction) {}
}

impl PrimarySelectionDeviceHandler for App {
    fn selection(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &ZwpPrimarySelectionDeviceV1) {}
}

impl PrimarySelectionSourceHandler for App {
    fn send_request(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &ZwpPrimarySelectionSourceV1, _mime: String, pipe: WritePipe) {
        send_text(self.primary_text.clone(), pipe);
    }
    fn cancelled(&mut self, _: &Connection, _: &QueueHandle<Self>, source: &ZwpPrimarySelectionSourceV1) {
        if self.primary_source.as_ref().is_some_and(|s| s.inner() == source) {
            self.primary_source = None;
        }
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState, SeatState];
}

impl Dispatch<WpViewporter, ()> for App {
    fn event(_: &mut Self, _: &WpViewporter, _: <WpViewporter as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WpViewport, ()> for App {
    fn event(_: &mut Self, _: &WpViewport, _: <WpViewport as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WpFractionalScaleManagerV1, ()> for App {
    fn event(_: &mut Self, _: &WpFractionalScaleManagerV1, _: <WpFractionalScaleManagerV1 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WpFractionalScaleV1, ()> for App {
    fn event(app: &mut Self, _: &WpFractionalScaleV1, event: wp_fractional_scale_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            if scale != app.scale120 {
                app.scale120 = scale;
                app.layout_changed();
            }
        }
    }
}

delegate_compositor!(App);
delegate_output!(App);
delegate_seat!(App);
delegate_keyboard!(App);
delegate_pointer!(App);
delegate_xdg_shell!(App);
delegate_xdg_window!(App);
delegate_data_device!(App);
delegate_primary_selection!(App);
delegate_registry!(App);
