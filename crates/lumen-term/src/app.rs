//! The terminal window: Wayland surface + EGL, tabs of split panes, the
//! frame loop and user actions.

use std::collections::HashMap;
use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};

use sctk::compositor::CompositorState;
use sctk::data_device_manager::data_device::DataDevice;
use sctk::data_device_manager::data_source::CopyPasteSource;
use sctk::data_device_manager::DataDeviceManagerState;
use sctk::output::OutputState;
use sctk::primary_selection::device::PrimarySelectionDevice;
use sctk::primary_selection::selection::PrimarySelectionSource;
use sctk::primary_selection::PrimarySelectionManagerState;
use sctk::reexports::calloop::generic::Generic;
use sctk::reexports::calloop::timer::{TimeoutAction, Timer};
use sctk::reexports::calloop::{Interest, LoopHandle, Mode, PostAction, RegistrationToken};
use sctk::reexports::client::protocol::{wl_keyboard, wl_pointer, wl_seat};
use sctk::reexports::client::{Connection, Proxy, QueueHandle};
use sctk::reexports::protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::WpCursorShapeDeviceV1;
use sctk::reexports::protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::WpFractionalScaleV1;
use sctk::reexports::protocols::wp::viewporter::client::wp_viewport::WpViewport;
use sctk::registry::RegistryState;
use sctk::seat::pointer::cursor_shape::CursorShapeManager;
use sctk::seat::SeatState;
use sctk::shell::xdg::window::Window;
use sctk::shell::WaylandSurface;
use smithay_client_toolkit as sctk;

use lumen_term::config::Config;
use lumen_term::layout::{self, Divider, Node, PaneId, Rect};
use lumen_term::pty::{self, Pty};
use lumen_term::term::{Event, Terminal};

use crate::gfx::glyphs::Fonts;
use crate::gfx::{color, Egl, RectInst, Renderer, Scene, KIND_FADE_DOWN};
use crate::pane::{Pane, Search};
use crate::view::{self, Colors, Damage, IRect};

pub struct Tab {
    pub root: Node,
    pub focus: PaneId,
}

/// What a pointer drag is doing.
#[derive(Clone, Debug, PartialEq)]
pub enum Drag {
    None,
    Select(PaneId),
    Divider(Divider),
    /// Buttons reported to the application (mouse mode).
    Report(PaneId),
}

/// Header elements under the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Hash)]
pub struct Hover {
    pub tab: Option<usize>,
    pub tab_close: Option<usize>,
    pub new_tab: bool,
    pub close: bool,
}

/// Clickable things in the header, in buffer pixels.
#[derive(Clone, Debug, Default)]
pub struct HeaderHits {
    pub tabs: Vec<(Rect, Rect)>,
    pub new_tab: Option<Rect>,
    pub close: Option<Rect>,
}

pub struct App {
    pub conn: Connection,
    pub qh: QueueHandle<App>,
    pub lh: LoopHandle<'static, App>,
    pub registry: RegistryState,
    pub seat_state: SeatState,
    pub output_state: OutputState,
    pub compositor: CompositorState,
    pub ddm: Option<DataDeviceManagerState>,
    pub psm: Option<PrimarySelectionManagerState>,
    pub cursor_shape_mgr: Option<CursorShapeManager>,
    pub window: Window,
    pub viewport: Option<WpViewport>,
    pub fractional: Option<WpFractionalScaleV1>,
    pub egl_win: Option<wayland_egl::WlEglSurface>,
    pub egl: Option<Egl>,
    pub renderer: Option<Renderer>,
    pub cfg: Config,
    pub fonts: Fonts,
    pub colors: Colors,
    /// Logical window size.
    pub size: (u32, u32),
    pub scale120: u32,
    pub int_scale: u32,
    pub configured: bool,
    pub maximized: bool,
    pub fullscreen: bool,
    pub tiled: bool,
    pub frame_pending: bool,
    pub needs_render: bool,
    pub full_redraw: bool,
    pub idle_scheduled: bool,
    pub tabs: Vec<Tab>,
    pub active: usize,
    pub panes: HashMap<PaneId, Pane>,
    pub sources: HashMap<PaneId, RegistrationToken>,
    next_id: PaneId,
    pub seat: Option<wl_seat::WlSeat>,
    pub keyboard: Option<wl_keyboard::WlKeyboard>,
    pub pointer: Option<wl_pointer::WlPointer>,
    pub cursor_device: Option<WpCursorShapeDeviceV1>,
    pub data_device: Option<DataDevice>,
    pub primary_device: Option<PrimarySelectionDevice>,
    pub copy_source: Option<CopyPasteSource>,
    pub primary_source: Option<PrimarySelectionSource>,
    pub clip_text: String,
    pub primary_text: String,
    pub last_serial: u32,
    pub mods: lumen_term::input::Mods,
    pub kb_focus: bool,
    /// Pointer position, logical.
    pub ptr: (f64, f64),
    pub ptr_serial: u32,
    pub clicks: (Instant, u32, (f64, f64)),
    pub drag: Drag,
    pub hover: Hover,
    pub hovered_link: u16,
    pub header_hits: HeaderHits,
    damage: Damage,
    scene: Scene,
    last_tick: Instant,
    wake: Option<(RegistrationToken, Instant)>,
    pub exit: bool,
    pub title_shown: String,
    pub initial: Option<Vec<String>>,
    pub config_error: Option<String>,
    pub font_size: f32,
    /// Touchpad scrolling: time of the last event and the smoothed speed.
    pub finger: (Instant, f64),
}

/// Logical header height.
pub const HEADER: f32 = 38.0;

impl App {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        conn: Connection,
        qh: QueueHandle<App>,
        lh: LoopHandle<'static, App>,
        globals: &sctk::reexports::client::globals::GlobalList,
        cfg: Config,
        config_error: Option<String>,
        initial: Option<Vec<String>>,
        title: Option<String>,
        app_id: &str,
    ) -> Result<App, String> {
        use sctk::shell::xdg::window::WindowDecorations;
        use sctk::shell::xdg::XdgShell;
        let compositor = CompositorState::bind(globals, &qh).map_err(|e| format!("wl_compositor: {e}"))?;
        let xdg = XdgShell::bind(globals, &qh).map_err(|e| format!("xdg_wm_base: {e}"))?;
        let ddm = DataDeviceManagerState::bind(globals, &qh).ok();
        let psm = PrimarySelectionManagerState::bind(globals, &qh).ok();
        let cursor_shape_mgr = CursorShapeManager::bind(globals, &qh).ok();
        use sctk::reexports::protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1;
        use sctk::reexports::protocols::wp::viewporter::client::wp_viewporter::WpViewporter;
        let viewporter: Option<WpViewporter> = globals.bind(&qh, 1..=1, ()).ok();
        let fractional_mgr: Option<WpFractionalScaleManagerV1> = globals.bind(&qh, 1..=1, ()).ok();
        let surface = compositor.create_surface(&qh);
        let (fractional, viewport) = match (&fractional_mgr, &viewporter) {
            (Some(fm), Some(vp)) => (Some(fm.get_fractional_scale(&surface, &qh, ())), Some(vp.get_viewport(&surface, &qh, ()))),
            _ => (None, None),
        };
        let window = xdg.create_window(surface, WindowDecorations::RequestClient, &qh);
        window.set_title(title.unwrap_or_else(|| "Terminal".into()));
        window.set_app_id(app_id.to_string());
        let font_size = cfg.font.size;
        let fonts = Fonts::new(&cfg.font.family, font_size, cfg.font.line_height, cfg.font.letter_spacing).map_err(|e| format!("fonts: {e}"))?;
        // Initial size from the configured cell grid.
        let c = fonts.cell;
        let pad = cfg.window.padding;
        let w = (c.w * cfg.window.width_cells as f32 + 2.0 * pad).ceil() as u32;
        let h = (c.h as f32 * cfg.window.height_cells as f32 + 2.0 * pad + HEADER).ceil() as u32;
        window.set_min_size(Some((240, 160)));
        let colors = Colors::from(&cfg);
        // xdg_wm_base is kept alive by the window; keep the shell object around.
        std::mem::forget(xdg);
        Ok(App {
            conn,
            qh: qh.clone(),
            lh,
            registry: RegistryState::new(globals),
            seat_state: SeatState::new(globals, &qh),
            output_state: OutputState::new(globals, &qh),
            compositor,
            ddm,
            psm,
            cursor_shape_mgr,
            window,
            viewport,
            fractional,
            egl_win: None,
            egl: None,
            renderer: None,
            cfg,
            fonts,
            colors,
            size: (w, h),
            scale120: 120,
            int_scale: 1,
            configured: false,
            maximized: false,
            fullscreen: false,
            tiled: false,
            frame_pending: false,
            needs_render: true,
            full_redraw: true,
            idle_scheduled: false,
            tabs: Vec::new(),
            active: 0,
            panes: HashMap::new(),
            sources: HashMap::new(),
            next_id: 1,
            seat: None,
            keyboard: None,
            pointer: None,
            cursor_device: None,
            data_device: None,
            primary_device: None,
            copy_source: None,
            primary_source: None,
            clip_text: String::new(),
            primary_text: String::new(),
            last_serial: 0,
            mods: Default::default(),
            kb_focus: false,
            ptr: (0.0, 0.0),
            ptr_serial: 0,
            clicks: (Instant::now(), 0, (0.0, 0.0)),
            drag: Drag::None,
            hover: Hover::default(),
            hovered_link: 0,
            header_hits: HeaderHits::default(),
            damage: Damage::default(),
            scene: Scene::default(),
            last_tick: Instant::now(),
            wake: None,
            exit: false,
            title_shown: String::new(),
            initial,
            config_error,
            font_size,
            finger: (Instant::now(), 0.0),
        })
    }

    /// Buffer pixels per logical pixel.
    pub fn scale(&self) -> f32 {
        if self.fractional.is_some() { self.scale120 as f32 / 120.0 } else { self.int_scale as f32 }
    }

    pub fn buffer_size(&self) -> (i32, i32) {
        let s = self.scale();
        ((self.size.0 as f32 * s).round().max(1.0) as i32, (self.size.1 as f32 * s).round().max(1.0) as i32)
    }

    pub fn header_height(&self) -> f32 {
        if self.fullscreen { 0.0 } else { (HEADER * self.scale()).round() }
    }

    /// Font pixel size for the current scale (Ctrl+±).
    fn font_px(&self) -> f32 {
        self.font_size * self.scale()
    }

    pub fn apply_font_size(&mut self) {
        let px = self.font_px();
        if (self.fonts.px - px).abs() > 0.01 {
            self.fonts.set_size(px, self.cfg.font.line_height, self.cfg.font.letter_spacing * self.scale());
            if let Some(r) = &mut self.renderer {
                r.atlas.clear();
            }
            self.full_redraw = true;
        }
    }

    pub fn tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    pub fn focused_pane(&self) -> Option<PaneId> {
        self.tab().map(|t| t.focus)
    }

    pub fn focused_mut(&mut self) -> Option<&mut Pane> {
        let id = self.focused_pane()?;
        self.panes.get_mut(&id)
    }

    /// Pane rectangles of the active tab (whole areas, buffer pixels).
    pub fn pane_areas(&self) -> Vec<(PaneId, Rect)> {
        let (w, h) = self.buffer_size();
        let top = self.header_height();
        let area = Rect::new(0.0, top, w as f32, (h as f32 - top).max(1.0));
        let mut out = Vec::new();
        if let Some(t) = self.tab() {
            t.root.layout(area, self.gap(), &mut out);
        }
        out
    }

    pub fn gap(&self) -> f32 {
        self.scale().round().max(1.0)
    }

    /// The cell grid inside a pane area.
    pub fn content_rect(&self, area: Rect) -> Rect {
        let pad = (self.cfg.window.padding * self.scale()).round();
        Rect::new(area.x + pad, area.y + pad, (area.w - 2.0 * pad).max(1.0), (area.h - 2.0 * pad).max(1.0))
    }

    fn grid_size(&self, area: Rect) -> (usize, usize) {
        let c = self.content_rect(area);
        let cell = self.fonts.cell;
        (((c.w / cell.w).floor() as usize).max(2), ((c.h / cell.h as f32).floor() as usize).max(1))
    }

    // -- panes ------------------------------------------------------------------------

    /// Start a shell in a new pane of size `area`.
    pub fn spawn_pane(&mut self, area: Rect, cwd: Option<std::path::PathBuf>) -> Option<PaneId> {
        let (cols, rows) = self.grid_size(area);
        let id = self.next_id;
        self.next_id += 1;
        let cmd = match self.initial.take() {
            Some(c) => c,
            None if !self.cfg.shell.program.is_empty() => self.cfg.shell.program.clone(),
            None => pty::default_shell(),
        };
        let cell = self.fonts.cell;
        let size = (cols as u16, rows as u16, (cols as f32 * cell.w) as u16, (rows as u32 * cell.h) as u16);
        let pty = match Pty::spawn(&cmd, cwd.as_deref(), size, &[]) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("lumen-term: cannot start {}: {e}", cmd.join(" "));
                return None;
            }
        };
        let mut term = Terminal::new(cols, rows, self.cfg.scroll.history);
        {
            let t = &mut term.term;
            t.base_palette = lumen_term::term::xterm_palette(&self.cfg.palette16());
            t.base_fg = self.cfg.colors.foreground.tuple();
            t.base_bg = self.cfg.colors.background.tuple();
            t.base_cursor = self.cfg.colors.cursor.tuple();
            t.cell_px = (cell.w.round() as u32, cell.h);
            t.modes.cursor_blink = self.cfg.cursor.blink;
        }
        let fd = pty.master.as_raw_fd();
        let pane = Pane::new(id, term, pty, cols, rows);
        self.panes.insert(id, pane);
        // SAFETY: the descriptor stays open while the pane exists; the source
        // is removed before the pane is dropped.
        let raw = unsafe { std::os::fd::BorrowedFd::borrow_raw(fd) };
        let owned = raw.try_clone_to_owned().ok()?;
        let token = self
            .lh
            .insert_source(Generic::new(owned, Interest::READ, Mode::Level), move |_, _, app: &mut App| {
                app.on_pty_readable(id);
                Ok(PostAction::Continue)
            })
            .ok()?;
        self.sources.insert(id, token);
        Some(id)
    }

    fn on_pty_readable(&mut self, id: PaneId) {
        let mut buf = vec![0u8; 1 << 16];
        let start = Instant::now();
        let mut gone = false;
        let mut got = false;
        {
            let Some(p) = self.panes.get_mut(&id) else { return };
            // Bounded work per wakeup so input and frames stay responsive.
            while start.elapsed() < Duration::from_millis(12) {
                match p.pty.read(&mut buf) {
                    Ok(0) => {
                        gone = true;
                        break;
                    }
                    Ok(n) => {
                        p.term.advance(&buf[..n]);
                        got = true;
                        if n < buf.len() {
                            break;
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => {
                        gone = true;
                        break;
                    }
                }
            }
            if !p.term.term.replies.is_empty() {
                let r = std::mem::take(&mut p.term.term.replies);
                p.write(&r);
            }
            if got {
                if let Some(s) = &mut p.search {
                    s.stale = true;
                }
                if p.term.term.modes.sync {
                    p.sync_since.get_or_insert_with(Instant::now);
                } else {
                    p.sync_since = None;
                }
            }
        }
        if got {
            self.handle_events(id);
            self.request_render();
        }
        if gone {
            self.close_pane(id);
        }
    }

    fn handle_events(&mut self, id: PaneId) {
        let Some(p) = self.panes.get_mut(&id) else { return };
        let events = std::mem::take(&mut p.term.term.events);
        for e in events {
            match e {
                Event::Bell => {
                    if let Some(p) = self.panes.get_mut(&id) {
                        p.bell = Some(Instant::now());
                    }
                }
                Event::Title(_) => self.needs_render = true,
                Event::ClipboardSet(target, text) => {
                    if self.cfg.behavior.osc52_write {
                        if target == 'p' {
                            self.set_primary(text);
                        } else {
                            self.set_clipboard(text);
                        }
                    }
                }
                Event::ClipboardQuery(target) => {
                    if self.cfg.behavior.osc52_read {
                        let text = if target == 'p' { self.primary_text.clone() } else { self.clip_text.clone() };
                        if let Some(p) = self.panes.get_mut(&id) {
                            p.term.term.clipboard_reply(target, &text);
                            let r = std::mem::take(&mut p.term.term.replies);
                            p.write(&r);
                        }
                    }
                }
            }
        }
    }

    pub fn new_tab(&mut self) {
        let cwd = self.focused_pane().and_then(|id| self.panes.get(&id)).and_then(|p| p.pty.foreground_cwd());
        let (w, h) = self.buffer_size();
        let top = self.header_height();
        let area = Rect::new(0.0, top, w as f32, (h as f32 - top).max(1.0));
        if let Some(id) = self.spawn_pane(area, cwd) {
            self.tabs.push(Tab { root: Node::Leaf(id), focus: id });
            self.active = self.tabs.len() - 1;
            self.layout_changed();
        }
    }

    pub fn split(&mut self, horizontal: bool) {
        let Some(focus) = self.focused_pane() else { return };
        let area = self.pane_areas().into_iter().find(|(p, _)| *p == focus).map(|(_, r)| r).unwrap_or(Rect::new(0.0, 0.0, 400.0, 300.0));
        let half = if horizontal { Rect::new(area.x, area.y, area.w / 2.0, area.h) } else { Rect::new(area.x, area.y, area.w, area.h / 2.0) };
        let cwd = self.panes.get(&focus).and_then(|p| p.pty.foreground_cwd());
        if let Some(id) = self.spawn_pane(half, cwd) {
            let t = &mut self.tabs[self.active];
            t.root.split(focus, id, horizontal);
            t.focus = id;
            self.layout_changed();
        }
    }

    pub fn close_pane(&mut self, id: PaneId) {
        if let Some(token) = self.sources.remove(&id) {
            self.lh.remove(token);
        }
        self.panes.remove(&id);
        let Some(ti) = self.tabs.iter().position(|t| t.root.panes().contains(&id)) else { return };
        let t = &mut self.tabs[ti];
        if t.root.remove(id) {
            if t.focus == id {
                t.focus = t.root.panes()[0];
            }
        } else {
            self.tabs.remove(ti);
            if self.tabs.is_empty() {
                self.exit = true;
                return;
            }
            if self.active >= self.tabs.len() || ti < self.active {
                self.active = self.active.saturating_sub(1).min(self.tabs.len() - 1);
            }
        }
        self.layout_changed();
    }

    pub fn close_focused(&mut self) {
        if let Some(id) = self.focused_pane() {
            self.close_pane(id);
        }
    }

    pub fn close_tab(&mut self, i: usize) {
        let Some(t) = self.tabs.get(i) else { return };
        for id in t.root.panes() {
            self.close_pane(id);
        }
    }

    pub fn select_tab(&mut self, i: usize) {
        if i < self.tabs.len() && i != self.active {
            self.active = i;
            self.layout_changed();
        }
    }

    pub fn focus_dir(&mut self, dir: layout::Dir) {
        let rects = self.pane_areas();
        let Some(focus) = self.focused_pane() else { return };
        if let Some(n) = layout::neighbor(&rects, focus, dir) {
            self.tabs[self.active].focus = n;
            self.request_render();
        }
    }

    /// Re-fit every pane to its area (after resize, split, font change).
    pub fn layout_changed(&mut self) {
        let cell = self.fonts.cell;
        for (id, area) in self.pane_areas() {
            let (cols, rows) = self.grid_size(area);
            if let Some(p) = self.panes.get_mut(&id) {
                if p.cols != cols || p.rows != rows || p.term.term.cell_px != (cell.w.round() as u32, cell.h) {
                    p.cols = cols;
                    p.rows = rows;
                    p.term.term.resize(cols, rows);
                    p.term.term.cell_px = (cell.w.round() as u32, cell.h);
                    p.pty.resize(cols as u16, rows as u16, (cols as f32 * cell.w) as u16, (rows as u32 * cell.h) as u16);
                    if let Some(s) = &mut p.search {
                        s.stale = true;
                    }
                }
            }
        }
        self.full_redraw = true;
        self.request_render();
    }

    // -- clipboard ----------------------------------------------------------------------

    pub fn set_clipboard(&mut self, text: String) {
        self.clip_text = text;
        if let (Some(ddm), Some(dev)) = (&self.ddm, &self.data_device) {
            let src = ddm.create_copy_paste_source(&self.qh, ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING", "TEXT", "STRING"]);
            src.set_selection(dev, self.last_serial);
            self.copy_source = Some(src);
        }
    }

    pub fn set_primary(&mut self, text: String) {
        self.primary_text = text;
        if let (Some(psm), Some(dev)) = (&self.psm, &self.primary_device) {
            let src = psm.create_selection_source(&self.qh, ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING", "TEXT", "STRING"]);
            src.set_selection(dev, self.last_serial);
            self.primary_source = Some(src);
        }
    }

    pub fn copy_selection(&mut self, clipboard: bool) {
        let Some(p) = self.focused_pane().and_then(|id| self.panes.get(&id)) else { return };
        let Some(sel) = p.selection else { return };
        if sel.is_empty() {
            return;
        }
        let text = sel.text(p.term.term.grid());
        if text.is_empty() {
            return;
        }
        if clipboard {
            self.set_clipboard(text);
        } else {
            self.set_primary(text);
        }
    }

    /// Paste the clipboard (or primary selection) into the focused pane.
    pub fn paste(&mut self, primary: bool) {
        use std::io::Read;
        let pipe = if primary {
            let Some(offer) = self.primary_device.as_ref().and_then(|d| d.data().selection_offer()) else { return };
            let Some(mime) = offer.with_mime_types(pick_text_mime) else { return };
            offer.receive(mime).ok()
        } else {
            let Some(offer) = self.data_device.as_ref().and_then(|d| d.data().selection_offer()) else { return };
            let Some(mime) = offer.with_mime_types(pick_text_mime) else { return };
            offer.receive(mime).ok()
        };
        let Some(pipe) = pipe else { return };
        let Some(target) = self.focused_pane() else { return };
        let mut data = Vec::new();
        let _ = self.lh.insert_source(pipe, move |(), file, app: &mut App| {
            let mut buf = [0u8; 8192];
            match (&**file).read(&mut buf) {
                Ok(0) => {
                    let text = String::from_utf8_lossy(&data).into_owned();
                    if let Some(p) = app.panes.get_mut(&target) {
                        let bytes = lumen_term::input::encode_paste(&text, &p.term.term.modes);
                        p.write(&bytes);
                        p.scroll_to_bottom();
                    }
                    PostAction::Remove
                }
                Ok(n) => {
                    data.extend_from_slice(&buf[..n]);
                    if data.len() > 64 << 20 {
                        return PostAction::Remove;
                    }
                    PostAction::Continue
                }
                Err(e) if matches!(e.kind(), std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock) => PostAction::Continue,
                Err(_) => PostAction::Remove,
            }
        });
    }

    // -- search -------------------------------------------------------------------------

    pub fn toggle_search(&mut self) {
        if let Some(p) = self.focused_mut() {
            if p.search.is_some() {
                p.search = None;
            } else {
                let query = p.selection.map(|s| s.text(p.term.term.grid())).filter(|t| !t.contains('\n') && t.len() < 200).unwrap_or_default();
                p.search = Some(Search { query, matches: Vec::new(), current: None, stale: true });
                p.search_step(true);
            }
        }
        self.request_render();
    }

    // -- frames -------------------------------------------------------------------------

    /// Something changed: render now if the compositor is ready for a frame,
    /// else when it is (batched after all pending events).
    pub fn request_render(&mut self) {
        self.needs_render = true;
        if self.frame_pending || self.idle_scheduled {
            return;
        }
        self.idle_scheduled = true;
        self.lh.insert_idle(|app| {
            app.idle_scheduled = false;
            if !app.frame_pending && app.needs_render {
                app.render();
            }
        });
    }

    pub fn ensure_egl(&mut self) -> bool {
        if self.egl.is_some() {
            return true;
        }
        let (w, h) = self.buffer_size();
        let surface = self.window.wl_surface().clone();
        let egl_win = match wayland_egl::WlEglSurface::new(surface.id(), w, h) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("lumen-term: wl_egl_window: {e}");
                self.exit = true;
                return false;
            }
        };
        let display = self.conn.backend().display_ptr() as *mut std::ffi::c_void;
        // SAFETY: the display and the egl window outlive the Egl object (both
        // are owned by self and dropped after it: see Drop order in main).
        let egl = match unsafe { Egl::new(display, egl_win.ptr()) } {
            Ok(e) => e,
            Err(e) => {
                eprintln!("lumen-term: EGL: {e}");
                self.exit = true;
                return false;
            }
        };
        let renderer = match Renderer::new() {
            Ok(r) => r,
            Err(e) => {
                eprintln!("lumen-term: GL: {e}");
                self.exit = true;
                return false;
            }
        };
        if std::env::var_os("LUMEN_TERM_DEBUG").is_some() {
            eprintln!("lumen-term: GL renderer: {}", egl.renderer_name());
        }
        self.egl_win = Some(egl_win);
        self.egl = Some(egl);
        self.renderer = Some(renderer);
        true
    }

    pub fn render(&mut self) {
        if !self.configured || self.exit {
            return;
        }
        if !self.ensure_egl() {
            return;
        }
        // Synchronized output: hold frames (up to 150 ms) while an
        // application is in the middle of an update.
        let now = Instant::now();
        let held = self.tab().map(|t| t.root.panes()).unwrap_or_default().iter().any(|id| {
            self.panes.get(id).and_then(|p| p.sync_since).is_some_and(|s| now.duration_since(s) < Duration::from_millis(150))
        });
        if held {
            self.schedule_wake(now + Duration::from_millis(20));
            return;
        }
        let dt = now.duration_since(self.last_tick).as_secs_f64().min(0.1);
        self.last_tick = now;
        let (bw, bh) = self.buffer_size();
        if let Some(e) = &self.egl_win {
            if e.get_size() != (bw, bh) {
                e.resize(bw, bh, 0, 0);
            }
        }
        if self.renderer.as_mut().unwrap().ensure_canvas(bw, bh) {
            self.full_redraw = true;
        }
        self.apply_font_size();
        let scale = self.scale();

        // Animations.
        let mut animating = false;
        let smooth_scroll = self.cfg.scroll.smooth;
        let smooth_cursor = self.cfg.cursor.smooth;
        let mut next_wake: Option<Duration> = None;
        let visible: Vec<(PaneId, Rect)> = self.pane_areas();
        let focus = self.focused_pane();
        for (id, _) in &visible {
            if let Some(p) = self.panes.get_mut(id) {
                p.track_output();
                let max = if p.term.term.alt_active { 0.0 } else { p.history() as f64 };
                p.scroll.step(dt, max, smooth_scroll);
                animating |= p.scroll.animating();
                animating |= p.step_cursor(dt, smooth_cursor);
                if let Some(m) = p.scroll.moved {
                    // Scrollbar fade-out.
                    if now.duration_since(m) < Duration::from_millis(1400) {
                        animating = true;
                    }
                }
                if let Some(b) = p.bell {
                    if now.duration_since(b) < Duration::from_millis(250) {
                        animating = true;
                    } else {
                        p.bell = None;
                    }
                }
            }
        }

        let renderer = self.renderer.as_mut().unwrap();
        let generation = renderer.atlas.generation;
        self.scene.clear();
        self.damage.begin();
        if self.full_redraw {
            self.damage.reset();
            self.damage.add(IRect { x: 0, y: 0, w: bw, h: bh });
        }
        crate::chrome::header(self, bw);
        let window_focused = self.kb_focus;
        let multi = visible.len() > 1;
        for (id, area) in &visible {
            let content = self.content_rect(*area);
            let focused = Some(*id) == focus;
            let Some(p) = self.panes.get_mut(id) else { continue };
            let blink = p.term.term.modes.cursor_blink && self.cfg.cursor.blink && focused && window_focused;
            let (alpha, wake) = p.cursor_alpha(now, blink);
            if let Some(w) = wake {
                if w.is_zero() {
                    animating = true;
                } else {
                    next_wake = Some(next_wake.map_or(w, |n: Duration| n.min(w)));
                }
            }
            let renderer = self.renderer.as_mut().unwrap();
            view::pane(
                view::PaneView { pane: p, rect: content, area: *area, focused, window_focused, cursor_alpha: alpha, hovered_link: if focused { self.hovered_link } else { 0 } },
                &self.cfg,
                &self.colors,
                &mut self.fonts,
                &mut renderer.atlas,
                &mut self.scene,
                &mut self.damage,
                scale,
            );
            crate::chrome::pane_overlays(self, *id, *area, content, focused, multi, now);
        }
        crate::chrome::dividers(self);
        self.damage.finish();
        let renderer = self.renderer.as_mut().unwrap();
        if renderer.atlas.generation != generation {
            // The atlas was reset while building: uvs from before are stale.
            self.full_redraw = true;
            self.needs_render = true;
            self.render_again();
            return;
        }
        let clip = self.damage.dirty.and_then(|d| d.clamp_to(bw, bh));
        renderer.draw(&self.scene, clip.map(|c| (c.x, c.y, c.w, c.h)));
        self.full_redraw = false;

        // Present.
        let surface = self.window.wl_surface().clone();
        let (lw, lh) = self.size;
        if let Some(vp) = &self.viewport {
            vp.set_destination(lw as i32, lh as i32);
            surface.set_buffer_scale(1);
        } else {
            surface.set_buffer_scale(self.int_scale as i32);
        }
        surface.frame(&self.qh, surface.clone());
        let egl = self.egl.as_ref().unwrap();
        let damage = clip.map(|c| (c.x, c.y, c.w, c.h));
        if !egl.swap(damage, bh) {
            eprintln!("lumen-term: eglSwapBuffers failed");
        }
        self.frame_pending = true;
        self.needs_render = animating;
        self.update_title();
        if let Some(w) = next_wake {
            self.schedule_wake(now + w);
        }
    }

    fn render_again(&mut self) {
        self.lh.insert_idle(|app| {
            if !app.frame_pending {
                app.render();
            }
        });
    }

    /// Wake up at `at` to render (cursor blink phases, held frames).
    pub fn schedule_wake(&mut self, at: Instant) {
        if let Some((_, t)) = self.wake {
            if t <= at {
                return;
            }
        }
        if let Some((token, _)) = self.wake.take() {
            self.lh.remove(token);
        }
        let token = self
            .lh
            .insert_source(Timer::from_deadline(at), |_, _, app: &mut App| {
                app.wake = None;
                app.request_render();
                TimeoutAction::Drop
            })
            .ok();
        self.wake = token.map(|t| (t, at));
    }

    pub fn pane_title(&self, id: PaneId) -> String {
        let Some(p) = self.panes.get(&id) else { return String::new() };
        if !p.term.term.title.is_empty() {
            return p.term.term.title.clone();
        }
        p.pty.foreground_name().unwrap_or_else(|| "Terminal".into())
    }

    fn update_title(&mut self) {
        let t = self.focused_pane().map(|id| self.pane_title(id)).unwrap_or_default();
        if t != self.title_shown {
            self.window.set_title(t.clone());
            self.title_shown = t;
        }
    }

    /// Rounded "pill" helper for chrome.
    pub fn pill(&mut self, layer: usize, r: Rect, radius: f32, c: [f32; 4]) {
        self.scene.layers[layer].rects.push(RectInst::rounded(r.x, r.y, r.w, r.h, radius, c));
    }

    pub fn scene_mut(&mut self) -> (&mut Scene, &mut Fonts, &mut crate::gfx::atlas::Atlas<crate::gfx::glyphs::GlyphKey>, &mut Damage) {
        let r = self.renderer.as_mut().expect("renderer");
        (&mut self.scene, &mut self.fonts, &mut r.atlas, &mut self.damage)
    }

    /// GL objects go before the context, the context before the window.
    pub fn teardown(&mut self) {
        self.renderer = None;
        self.egl = None;
        self.egl_win = None;
    }

    pub fn shade(&mut self, r: Rect) {
        let c = color((0, 0, 0), 0.22);
        self.scene.layers[3].rects.push(RectInst { rect: [r.x, r.y, r.w, r.h], color: c, params: [0.0, KIND_FADE_DOWN, 0.0, 0.0] });
    }
}

pub fn pick_text_mime(offered: &[String]) -> Option<String> {
    for want in ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain", "TEXT", "STRING"] {
        if offered.iter().any(|m| m == want) {
            return Some(want.to_string());
        }
    }
    None
}
