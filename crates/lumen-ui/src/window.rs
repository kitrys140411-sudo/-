//! The window runtime: an xdg toplevel through smithay-client-toolkit with
//! fractional scaling (viewporter), keyboard with repeat, pointer with
//! cursor shapes, clipboard and primary selection, client-side decorations,
//! and frame-callback driven rendering that only draws when something
//! changed (or an animation runs) and only redraws the damaged part.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::os::fd::OwnedFd;
use std::rc::Rc;
use std::time::{Duration, Instant};

use sctk::compositor::{CompositorHandler, CompositorState};
use sctk::data_device_manager::data_device::{DataDevice, DataDeviceHandler};
use sctk::data_device_manager::data_offer::{DataOfferHandler, DragOffer};
use sctk::data_device_manager::data_source::{CopyPasteSource, DataSourceHandler};
use sctk::data_device_manager::{DataDeviceManagerState, WritePipe};
use sctk::output::{OutputHandler, OutputState};
use sctk::primary_selection::device::{PrimarySelectionDevice, PrimarySelectionDeviceHandler};
use sctk::primary_selection::selection::{PrimarySelectionSource, PrimarySelectionSourceHandler};
use sctk::primary_selection::PrimarySelectionManagerState;
use sctk::reexports::calloop::timer::{TimeoutAction, Timer};
use sctk::reexports::calloop::{EventLoop, LoopHandle, PostAction, RegistrationToken};
use sctk::reexports::calloop_wayland_source::WaylandSource;
use sctk::reexports::client::globals::registry_queue_init;
use sctk::reexports::client::protocol::wl_data_device::WlDataDevice;
use sctk::reexports::client::protocol::wl_data_device_manager::DndAction;
use sctk::reexports::client::protocol::wl_data_source::WlDataSource;
use sctk::reexports::client::protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_surface};
use sctk::reexports::client::{Connection, Dispatch, Proxy, QueueHandle};
use sctk::reexports::protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::{Shape, WpCursorShapeDeviceV1};
use sctk::reexports::protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1;
use sctk::reexports::protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::{self, WpFractionalScaleV1};
use sctk::reexports::protocols::wp::primary_selection::zv1::client::zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1;
use sctk::reexports::protocols::wp::primary_selection::zv1::client::zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1;
use sctk::reexports::protocols::wp::viewporter::client::wp_viewport::WpViewport;
use sctk::reexports::protocols::wp::viewporter::client::wp_viewporter::WpViewporter;
use sctk::reexports::protocols::xdg::shell::client::xdg_toplevel::ResizeEdge;
use sctk::registry::{ProvidesRegistryState, RegistryState};
use sctk::seat::keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers};
use sctk::seat::pointer::cursor_shape::CursorShapeManager;
use sctk::seat::pointer::{PointerEvent, PointerEventKind, PointerHandler, BTN_LEFT, BTN_MIDDLE, BTN_RIGHT};
use sctk::seat::{Capability, SeatHandler, SeatState};
use sctk::shell::xdg::window::{Window, WindowConfigure, WindowDecorations, WindowHandler};
use sctk::shell::xdg::XdgShell;
use sctk::shell::WaylandSurface;
use sctk::{
    delegate_compositor, delegate_data_device, delegate_keyboard, delegate_output, delegate_pointer, delegate_primary_selection,
    delegate_registry, delegate_seat, delegate_xdg_shell, delegate_xdg_window, registry_handlers,
};
use smithay_client_toolkit as sctk;

use crate::geom::Vec2;
use crate::gfx::{DrawList, Egl, IRect, Renderer};
use crate::image::ImageCache;
use crate::input::{Button, Event, Key, KeyPress, Mods};
use crate::paint::Painter;
use crate::text::Fonts;
use crate::theme::Theme;
use crate::ui::{CursorIcon, Ui};
use crate::{App, Options};

/// Wakes the UI from another thread (the next frame runs `view`).
#[derive(Clone)]
pub struct Waker(Option<sctk::reexports::calloop::ping::Ping>);

impl Waker {
    pub(crate) fn none() -> Waker {
        Waker(None)
    }

    pub fn wake(&self) {
        if let Some(p) = &self.0 {
            p.ping();
        }
    }
}

const MIME: [&str; 5] = ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING", "TEXT", "STRING"];

struct Runtime {
    conn: Connection,
    qh: QueueHandle<Runtime>,
    lh: LoopHandle<'static, Runtime>,
    registry: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    _compositor: CompositorState,
    ddm: Option<DataDeviceManagerState>,
    psm: Option<PrimarySelectionManagerState>,
    cursor_shape_mgr: Option<CursorShapeManager>,
    window: Window,
    viewport: Option<WpViewport>,
    fractional: Option<WpFractionalScaleV1>,
    egl_win: Option<wayland_egl::WlEglSurface>,
    egl: Option<Egl>,
    renderer: Option<Renderer>,
    ui: Ui,
    app: Box<dyn App>,
    images: ImageCache,
    lib: Rc<lumen_text::Library>,
    list: DrawList,
    prev_list: DrawList,
    full_redraw: bool,
    events: VecDeque<Event>,
    /// Logical window size.
    size: (u32, u32),
    scale120: u32,
    int_scale: u32,
    configured: bool,
    frame_pending: bool,
    needs_render: bool,
    idle_scheduled: bool,
    wake: Option<(RegistrationToken, Instant)>,
    exit: bool,
    seat: Option<wl_seat::WlSeat>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer: Option<wl_pointer::WlPointer>,
    cursor_device: Option<WpCursorShapeDeviceV1>,
    cursor_shown: Option<Shape>,
    data_device: Option<DataDevice>,
    primary_device: Option<PrimarySelectionDevice>,
    copy_source: Option<CopyPasteSource>,
    primary_source: Option<PrimarySelectionSource>,
    clip_text: String,
    primary_text: String,
    last_serial: u32,
    ptr_serial: u32,
    ptr: Vec2,
    mods: Mods,
    /// The pointer pressed on the resize border (events not passed on).
    resizing: bool,
    ready_logged: bool,
    /// Frames, total and worst CPU time (ms) since the last perf line, and
    /// the texture uploads (glyphs and images) when it was printed.
    perf: (u32, f32, f32),
    perf_uploads: u64,
    /// Frames presented, and frames rendered (view run) — for tests.
    presented: u64,
    renders: u64,
}

/// Open the window and run `app` until it closes.
pub fn run(options: Options, app: impl App + 'static) -> Result<(), String> {
    // Writing to a closed clipboard pipe must not kill the app.
    // SAFETY: setting a signal disposition.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
    let conn = Connection::connect_to_env().map_err(|e| format!("cannot connect to the Wayland compositor: {e}"))?;
    let (globals, mut queue) = registry_queue_init::<Runtime>(&conn).map_err(|e| format!("wl_registry: {e}"))?;
    let qh = queue.handle();
    let mut event_loop: EventLoop<'static, Runtime> = EventLoop::try_new().map_err(|e| format!("event loop: {e}"))?;
    let lh = event_loop.handle();

    let compositor = CompositorState::bind(&globals, &qh).map_err(|e| format!("wl_compositor: {e}"))?;
    let xdg = XdgShell::bind(&globals, &qh).map_err(|e| format!("xdg_wm_base: {e}"))?;
    let viewporter: Option<WpViewporter> = globals.bind(&qh, 1..=1, ()).ok();
    let fractional_mgr: Option<WpFractionalScaleManagerV1> = globals.bind(&qh, 1..=1, ()).ok();
    let surface = compositor.create_surface(&qh);
    let (fractional, viewport) = match (&fractional_mgr, &viewporter) {
        (Some(fm), Some(vp)) => (Some(fm.get_fractional_scale(&surface, &qh, ())), Some(vp.get_viewport(&surface, &qh, ()))),
        _ => (None, None),
    };
    let window = xdg.create_window(surface, WindowDecorations::RequestClient, &qh);
    window.set_title(options.title.clone());
    window.set_app_id(options.app_id.clone());
    window.set_min_size(Some(options.min_size));
    // The window keeps xdg_wm_base alive; the shell object itself is not needed.
    std::mem::forget(xdg);

    let theme = Theme::load();
    let fonts = Fonts::new(&theme.font_ui, &theme.font_mono)?;
    let lib = fonts.db.lib.clone();
    let (ping, ping_source) = sctk::reexports::calloop::ping::make_ping().map_err(|e| format!("event loop: {e}"))?;
    lh.insert_source(ping_source, |_, _, rt: &mut Runtime| rt.request_render()).map_err(|e| format!("event loop: {e}"))?;
    let mut ui = Ui::new(theme, fonts, Waker(Some(ping)));
    ui.title = options.title.clone();

    let mut rt = Runtime {
        conn: conn.clone(),
        qh: qh.clone(),
        lh: lh.clone(),
        registry: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        output_state: OutputState::new(&globals, &qh),
        ddm: DataDeviceManagerState::bind(&globals, &qh).ok(),
        psm: PrimarySelectionManagerState::bind(&globals, &qh).ok(),
        cursor_shape_mgr: CursorShapeManager::bind(&globals, &qh).ok(),
        _compositor: compositor,
        window,
        viewport,
        fractional,
        egl_win: None,
        egl: None,
        renderer: None,
        ui,
        app: Box::new(app),
        images: ImageCache::default(),
        lib,
        list: DrawList::default(),
        prev_list: DrawList::default(),
        full_redraw: true,
        events: VecDeque::new(),
        size: options.size,
        scale120: 120,
        int_scale: 1,
        configured: false,
        frame_pending: false,
        needs_render: true,
        idle_scheduled: false,
        wake: None,
        exit: false,
        seat: None,
        keyboard: None,
        pointer: None,
        cursor_device: None,
        cursor_shown: None,
        data_device: None,
        primary_device: None,
        copy_source: None,
        primary_source: None,
        clip_text: String::new(),
        primary_text: String::new(),
        last_serial: 0,
        ptr_serial: 0,
        ptr: Vec2::ZERO,
        mods: Mods::NONE,
        resizing: false,
        ready_logged: false,
        perf: (0, 0.0, 0.0),
        perf_uploads: 0,
        presented: 0,
        renders: 0,
    };
    // Seat capabilities (keyboard, data devices) before mapping.
    queue.roundtrip(&mut rt).map_err(|e| format!("roundtrip: {e}"))?;
    rt.window.commit();
    WaylandSource::new(conn.clone(), queue).insert(lh).map_err(|e| format!("event loop: {}", e.error))?;
    while !rt.exit {
        if let Err(e) = event_loop.dispatch(None, &mut rt) {
            return Err(format!("connection lost: {e}"));
        }
    }
    // GL objects go before the context, the context before the window.
    rt.renderer = None;
    rt.egl = None;
    rt.egl_win = None;
    drop(rt);
    let _ = conn.flush();
    Ok(())
}

impl Runtime {
    fn scale(&self) -> f32 {
        if self.fractional.is_some() { self.scale120 as f32 / 120.0 } else { self.int_scale as f32 }
    }

    fn buffer_size(&self) -> (i32, i32) {
        let s = self.scale();
        ((self.size.0 as f32 * s).round().max(1.0) as i32, (self.size.1 as f32 * s).round().max(1.0) as i32)
    }

    fn push(&mut self, e: Event) {
        self.events.push_back(e);
        self.request_render();
    }

    /// Something changed: render when the compositor is ready for a frame
    /// (batched after all pending events).
    fn request_render(&mut self) {
        self.needs_render = true;
        if self.frame_pending || self.idle_scheduled {
            return;
        }
        self.idle_scheduled = true;
        self.lh.insert_idle(|rt| {
            rt.idle_scheduled = false;
            if !rt.frame_pending && rt.needs_render {
                rt.render();
            }
        });
    }

    fn schedule_wake(&mut self, at: Instant) {
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
            .insert_source(Timer::from_deadline(at), |_, _, rt: &mut Runtime| {
                rt.wake = None;
                rt.request_render();
                TimeoutAction::Drop
            })
            .ok();
        self.wake = token.map(|t| (t, at));
    }

    fn ensure_egl(&mut self) -> bool {
        if self.egl.is_some() {
            return true;
        }
        let (w, h) = self.buffer_size();
        let surface = self.window.wl_surface().clone();
        let egl_win = match wayland_egl::WlEglSurface::new(surface.id(), w, h) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("lumen-ui: wl_egl_window: {e}");
                self.exit = true;
                return false;
            }
        };
        let display = self.conn.backend().display_ptr() as *mut std::ffi::c_void;
        // SAFETY: the display and the EGL window outlive the Egl object (it
        // is dropped first, see the end of `run`).
        let egl = match unsafe { Egl::new(display, egl_win.ptr()) } {
            Ok(e) => e,
            Err(e) => {
                eprintln!("lumen-ui: EGL: {e}");
                self.exit = true;
                return false;
            }
        };
        match Renderer::new() {
            Ok(r) => self.renderer = Some(r),
            Err(e) => {
                eprintln!("lumen-ui: GL: {e}");
                self.exit = true;
                return false;
            }
        }
        self.egl_win = Some(egl_win);
        self.egl = Some(egl);
        true
    }

    /// Build, lay out and paint a frame; present it if anything changed.
    fn render(&mut self) {
        if !self.configured || self.exit || !self.ensure_egl() {
            return;
        }
        let t0 = Instant::now();
        let now = t0;
        self.renders += 1;
        let (bw, bh) = self.buffer_size();
        if let Some(e) = &self.egl_win {
            if e.get_size() != (bw, bh) {
                e.resize(bw, bh, 0, 0);
            }
        }
        if self.renderer.as_mut().unwrap().ensure_canvas(bw, bh) {
            self.full_redraw = true;
        }
        self.ui.window.size = Vec2::new(self.size.0 as f32, self.size.1 as f32);
        self.ui.window.scale = self.scale();
        self.run_passes(now);
        if self.exit {
            return;
        }
        self.apply_requests();
        // Paint; an atlas that overflowed while painting invalidates what
        // was painted before, so build and paint again from scratch.
        for attempt in 0..2 {
            let renderer = self.renderer.as_mut().unwrap();
            let generation = renderer.atlas.generation;
            self.list.clear();
            {
                let backdrop = self.ui.theme.window_bg;
                let mut p = Painter::new(&mut self.list, renderer, &mut self.images, &self.lib, self.ui.window.scale, backdrop);
                self.ui.paint(&mut p);
            }
            if self.renderer.as_ref().unwrap().atlas.generation == generation || attempt == 1 {
                break;
            }
            self.full_redraw = true;
            self.run_passes(now);
        }
        let damage = if self.full_redraw { Some(IRect::new(0, 0, bw, bh)) } else { self.list.damage_since(&self.prev_list) };
        let animating = self.ui.animating;
        let renderer = self.renderer.as_mut().unwrap();
        if self.ui.frame % 300 == 0 {
            self.images.collect(renderer);
        }
        let Some(damage) = damage else {
            // Nothing visible changed: no buffer, no frame callback. Keep
            // animations ticking with a timer.
            std::mem::swap(&mut self.list, &mut self.prev_list);
            self.needs_render = false;
            if animating {
                self.schedule_wake(now + Duration::from_millis(16));
            }
            // The burst may end with a frame that changed nothing.
            self.report_perf(animating);
            self.after_frame(now);
            return;
        };
        renderer.draw_list(&self.list, Some(damage));
        let cpu = t0.elapsed().as_secs_f32() * 1000.0;
        self.full_redraw = false;
        let surface = self.window.wl_surface().clone();
        if let Some(vp) = &self.viewport {
            vp.set_destination(self.size.0 as i32, self.size.1 as i32);
            surface.set_buffer_scale(1);
        } else {
            surface.set_buffer_scale(self.int_scale as i32);
        }
        surface.frame(&self.qh, surface.clone());
        let d = damage.intersect(&IRect::new(0, 0, bw, bh));
        if !self.egl.as_ref().unwrap().swap(Some((d.x, d.y, d.w, d.h)), bh) {
            eprintln!("lumen-ui: eglSwapBuffers failed");
        }
        std::mem::swap(&mut self.list, &mut self.prev_list);
        self.presented += 1;
        self.frame_pending = true;
        self.needs_render = animating;
        self.perf.0 += 1;
        self.perf.1 += cpu;
        self.perf.2 = self.perf.2.max(cpu);
        self.report_perf(animating);
        if !self.ready_logged {
            self.ready_logged = true;
            self.ui.log(&format!("ready {}x{}", self.size.0, self.size.1));
        }
        self.after_frame(now);
    }

    /// Test builds report frame times every 120 presented frames and at the
    /// end of each burst of activity (of at least a few frames; shorter ones
    /// count toward the next report), with the textures uploaded meanwhile.
    fn report_perf(&mut self, animating: bool) {
        if !self.ui.test.enabled() || self.perf.0 < 5 || (self.perf.0 < 120 && animating) {
            return;
        }
        let r = self.renderer.as_ref().unwrap();
        let uploads = r.atlas.uploads + r.texture_uploads;
        self.ui.log(&format!(
            "perf frames={} avg_ms={:.2} max_ms={:.2} uploads={}",
            self.perf.0,
            self.perf.1 / self.perf.0 as f32,
            self.perf.2,
            uploads - self.perf_uploads
        ));
        self.perf = (0, 0.0, 0.0);
        self.perf_uploads = uploads;
    }

    fn after_frame(&mut self, now: Instant) {
        if let Some(at) = self.ui.repaint_at {
            self.schedule_wake(at.max(now + Duration::from_millis(1)));
        }
        let focus = self.ui.input.focus;
        let hovered = self.ui.input.hovered;
        let size = (self.size.0 as f32, self.size.1 as f32);
        let scale = self.scale();
        self.ui.test.write_map(&self.ui.tree, size, scale, focus, hovered, (self.presented, self.renders));
        self.update_cursor();
    }

    fn run_passes(&mut self, now: Instant) {
        if self.ui.run_frame(&mut *self.app, &mut self.events, now) {
            self.exit = true;
        }
    }

    fn apply_requests(&mut self) {
        let out = std::mem::take(&mut self.ui.out);
        if let Some(t) = out.title {
            self.window.set_title(t);
        }
        if out.toggle_maximize {
            if self.ui.window.maximized {
                self.window.unset_maximized();
            } else {
                self.window.set_maximized();
            }
        }
        if let Some(f) = out.fullscreen {
            if f {
                self.window.set_fullscreen(None);
            } else {
                self.window.unset_fullscreen();
            }
        }
        if out.start_move {
            if let Some(seat) = &self.seat {
                self.window.move_(seat, self.last_serial);
            }
        }
        if let Some(p) = out.window_menu {
            if let Some(seat) = &self.seat {
                self.window.show_window_menu(seat, self.last_serial, (p.x as i32, p.y as i32));
            }
        }
        if let Some((text, primary)) = out.copy {
            if primary {
                self.set_primary(text);
            } else {
                self.set_clipboard(text);
            }
        }
        if let Some((target, primary)) = out.paste {
            self.paste(target, primary);
        }
    }

    fn set_clipboard(&mut self, text: String) {
        self.clip_text = text;
        if let (Some(ddm), Some(dev)) = (&self.ddm, &self.data_device) {
            let src = ddm.create_copy_paste_source(&self.qh, MIME);
            src.set_selection(dev, self.last_serial);
            self.copy_source = Some(src);
        }
    }

    fn set_primary(&mut self, text: String) {
        self.primary_text = text;
        if let (Some(psm), Some(dev)) = (&self.psm, &self.primary_device) {
            let src = psm.create_selection_source(&self.qh, MIME);
            src.set_selection(dev, self.last_serial);
            self.primary_source = Some(src);
        }
    }

    /// Read the clipboard asynchronously; the text arrives as a Paste event.
    fn paste(&mut self, target: crate::Id, primary: bool) {
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
        let mut data = Vec::new();
        let _ = self.lh.insert_source(pipe, move |(), file, rt: &mut Runtime| {
            let mut buf = [0u8; 8192];
            match (&**file).read(&mut buf) {
                Ok(0) => {
                    let text = String::from_utf8_lossy(&data).into_owned();
                    rt.push(Event::Paste { target, text });
                    PostAction::Remove
                }
                Ok(n) => {
                    data.extend_from_slice(&buf[..n]);
                    if data.len() > 16 << 20 { PostAction::Remove } else { PostAction::Continue }
                }
                Err(e) if matches!(e.kind(), std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock) => PostAction::Continue,
                Err(_) => PostAction::Remove,
            }
        });
    }

    fn resizable(&self) -> bool {
        !self.ui.window.maximized && !self.ui.window.fullscreen && !self.ui.window.tiled
    }

    fn resize_edge_at(&self, p: Vec2) -> Option<ResizeEdge> {
        if !self.resizable() {
            return None;
        }
        let (l, r, t, b) = crate::chrome::resize_edge((self.size.0 as f32, self.size.1 as f32), p.x, p.y)?;
        Some(match (l, r, t, b) {
            (true, _, true, _) => ResizeEdge::TopLeft,
            (_, true, true, _) => ResizeEdge::TopRight,
            (true, _, _, true) => ResizeEdge::BottomLeft,
            (_, true, _, true) => ResizeEdge::BottomRight,
            (true, ..) => ResizeEdge::Left,
            (_, true, ..) => ResizeEdge::Right,
            (_, _, true, _) => ResizeEdge::Top,
            _ => ResizeEdge::Bottom,
        })
    }

    fn update_cursor(&mut self) {
        let shape = match self.resize_edge_at(self.ptr) {
            Some(ResizeEdge::Left | ResizeEdge::Right) => Shape::EwResize,
            Some(ResizeEdge::Top | ResizeEdge::Bottom) => Shape::NsResize,
            Some(ResizeEdge::TopLeft | ResizeEdge::BottomRight) => Shape::NwseResize,
            Some(_) => Shape::NeswResize,
            None => match self.ui.cursor {
                CursorIcon::Default => Shape::Default,
                CursorIcon::Pointer => Shape::Pointer,
                CursorIcon::Text => Shape::Text,
                CursorIcon::Grab => Shape::Grab,
                CursorIcon::Grabbing => Shape::Grabbing,
                CursorIcon::Move => Shape::Move,
                CursorIcon::ResizeEw => Shape::EwResize,
                CursorIcon::ResizeNs => Shape::NsResize,
                CursorIcon::ResizeNwse => Shape::NwseResize,
                CursorIcon::ResizeNesw => Shape::NeswResize,
                CursorIcon::NotAllowed => Shape::NotAllowed,
                CursorIcon::Progress => Shape::Progress,
            },
        };
        if self.cursor_shown != Some(shape) {
            if let Some(d) = &self.cursor_device {
                d.set_shape(self.ptr_serial, shape);
                self.cursor_shown = Some(shape);
            }
        }
    }

    fn on_key(&mut self, event: KeyEvent, repeat: bool) {
        let Some(key) = key_of(&event) else { return };
        let text = event.utf8.clone().filter(|t| !t.chars().any(|c| c.is_control()));
        self.push(Event::Key(KeyPress { key, mods: self.mods, text, repeat }));
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

fn key_of(event: &KeyEvent) -> Option<Key> {
    let k = event.keysym;
    Some(match k {
        Keysym::Return | Keysym::KP_Enter => Key::Enter,
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
        Keysym::Menu => Key::Menu,
        Keysym::space => Key::Char(' '),
        _ => {
            let raw = k.raw();
            if (Keysym::F1.raw()..=Keysym::F12.raw()).contains(&raw) {
                Key::F((raw - Keysym::F1.raw() + 1) as u8)
            } else if let Some(c) = k.key_char() {
                // Letters as lower case: Shift is in the modifiers.
                let c = c.to_lowercase().next().unwrap_or(c);
                if c.is_control() {
                    return None;
                }
                // In a non-Latin layout (Russian) a key is named by what it
                // types in the US layout, so Ctrl+S is Ctrl+S in every
                // layout; the typed text is still the layout's.
                Key::Char(if c.is_ascii() { c } else { us_char(event.raw_code).unwrap_or(c) })
            } else if is_modifier(k) {
                return None;
            } else {
                Key::Other(raw)
            }
        }
    })
}

/// The character an evdev key code types in the US layout (letters lower
/// case), for naming keys independently of the active layout.
pub(crate) fn us_char(code: u32) -> Option<char> {
    const ROWS: [(u32, &str); 4] = [(2, "1234567890-="), (16, "qwertyuiop[]"), (30, "asdfghjkl;'`"), (44, "zxcvbnm,./")];
    match code {
        43 => return Some('\\'),
        57 => return Some(' '),
        _ => {}
    }
    ROWS.iter().find_map(|&(first, chars)| chars.chars().nth(code.checked_sub(first)? as usize))
}

fn is_modifier(k: Keysym) -> bool {
    matches!(
        k,
        Keysym::Shift_L
            | Keysym::Shift_R
            | Keysym::Control_L
            | Keysym::Control_R
            | Keysym::Alt_L
            | Keysym::Alt_R
            | Keysym::Super_L
            | Keysym::Super_R
            | Keysym::Meta_L
            | Keysym::Meta_R
            | Keysym::ISO_Level3_Shift
            | Keysym::Caps_Lock
            | Keysym::Num_Lock
    )
}

impl CompositorHandler for Runtime {
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &wl_surface::WlSurface, factor: i32) {
        if surface == self.window.wl_surface() && self.fractional.is_none() {
            let f = factor.max(1) as u32;
            if f != self.int_scale {
                self.int_scale = f;
                self.full_redraw = true;
                self.request_render();
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

impl OutputHandler for Runtime {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl WindowHandler for Runtime {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Window) {
        if self.app.close_requested() {
            self.exit = true;
        } else {
            self.request_render();
        }
    }

    fn configure(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Window, configure: WindowConfigure, _serial: u32) {
        use sctk::reexports::csd_frame::WindowState;
        let w = &mut self.ui.window;
        w.maximized = configure.state.contains(WindowState::MAXIMIZED);
        w.fullscreen = configure.state.contains(WindowState::FULLSCREEN);
        w.tiled = w.maximized || configure.state.intersects(WindowState::TILED);
        let activated = configure.state.contains(WindowState::ACTIVATED);
        if activated != self.ui.input.window_focused {
            self.events.push_back(Event::Focus(activated));
        }
        if let (Some(w), Some(h)) = configure.new_size {
            self.size = (w.get(), h.get());
        }
        self.configured = true;
        self.full_redraw = true;
        self.frame_pending = false;
        self.needs_render = true;
        self.render();
    }
}

impl SeatHandler for Runtime {
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
                    Box::new(|rt: &mut Runtime, _kb: &wl_keyboard::WlKeyboard, event: KeyEvent| rt.on_key(event, true)),
                ) {
                    Ok(k) => self.keyboard = Some(k),
                    Err(e) => eprintln!("lumen-ui: keyboard: {e}"),
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

impl KeyboardHandler for Runtime {
    fn enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: &wl_surface::WlSurface, serial: u32, _: &[u32], _: &[Keysym]) {
        self.last_serial = serial;
        self.push(Event::Focus(true));
    }

    fn leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: &wl_surface::WlSurface, _: u32) {
        self.mods = Mods::NONE;
        self.push(Event::Focus(false));
    }

    fn press_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, serial: u32, event: KeyEvent) {
        self.last_serial = serial;
        self.on_key(event, false);
    }

    fn release_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, _: KeyEvent) {}

    fn update_modifiers(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, m: Modifiers, _: u32) {
        self.mods = Mods { ctrl: m.ctrl, shift: m.shift, alt: m.alt, logo: m.logo };
        self.ui.input.mods = self.mods;
    }
}

impl PointerHandler for Runtime {
    fn pointer_frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_pointer::WlPointer, events: &[PointerEvent]) {
        for e in events {
            if &e.surface != self.window.wl_surface() {
                continue;
            }
            let p = Vec2::new(e.position.0 as f32, e.position.1 as f32);
            self.ptr = p;
            match e.kind {
                PointerEventKind::Enter { serial } => {
                    self.ptr_serial = serial;
                    self.cursor_shown = None;
                    self.push(Event::PointerMove(p));
                }
                PointerEventKind::Leave { .. } => self.push(Event::PointerLeave),
                PointerEventKind::Motion { .. } => {
                    self.update_cursor();
                    if !self.resizing {
                        self.push(Event::PointerMove(p));
                    }
                }
                PointerEventKind::Press { button, serial, .. } => {
                    self.last_serial = serial;
                    if button == BTN_LEFT {
                        if let (Some(edge), Some(seat)) = (self.resize_edge_at(p), &self.seat) {
                            self.window.resize(seat, serial, edge);
                            self.resizing = true;
                            continue;
                        }
                    }
                    if let Some(b) = button_of(button) {
                        self.push(Event::PointerButton { pos: p, button: b, pressed: true });
                    }
                }
                PointerEventKind::Release { button, serial, .. } => {
                    self.last_serial = serial;
                    if std::mem::take(&mut self.resizing) {
                        continue;
                    }
                    if let Some(b) = button_of(button) {
                        self.push(Event::PointerButton { pos: p, button: b, pressed: false });
                    }
                }
                PointerEventKind::Axis { horizontal, vertical, source, .. } => {
                    let finger = matches!(source, Some(wl_pointer::AxisSource::Finger | wl_pointer::AxisSource::Continuous));
                    // Wheel notches scroll three lines' worth; touchpads
                    // move the content with the fingers.
                    let step = |a: &sctk::seat::pointer::AxisScroll| if a.discrete != 0 { a.discrete as f32 * 56.0 } else if finger { a.absolute as f32 } else { a.absolute as f32 * 3.7 };
                    let delta = Vec2::new(step(&horizontal), step(&vertical));
                    let stop = vertical.stop || horizontal.stop;
                    if delta != Vec2::ZERO || stop {
                        self.push(Event::Scroll { pos: p, delta, wheel: !finger, kinetic_end: stop && finger });
                    }
                }
            }
        }
    }
}

fn button_of(b: u32) -> Option<Button> {
    match b {
        BTN_LEFT => Some(Button::Left),
        BTN_RIGHT => Some(Button::Right),
        BTN_MIDDLE => Some(Button::Middle),
        _ => None,
    }
}

impl DataDeviceHandler for Runtime {
    fn enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice, _: f64, _: f64, _: &wl_surface::WlSurface) {}
    fn leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {}
    fn motion(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice, _: f64, _: f64) {}
    fn selection(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {}
    fn drop_performed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {}
}

impl DataOfferHandler for Runtime {
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

impl DataSourceHandler for Runtime {
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

impl PrimarySelectionDeviceHandler for Runtime {
    fn selection(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &ZwpPrimarySelectionDeviceV1) {}
}

impl PrimarySelectionSourceHandler for Runtime {
    fn send_request(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &ZwpPrimarySelectionSourceV1, _mime: String, pipe: WritePipe) {
        send_text(self.primary_text.clone(), pipe);
    }
    fn cancelled(&mut self, _: &Connection, _: &QueueHandle<Self>, source: &ZwpPrimarySelectionSourceV1) {
        if self.primary_source.as_ref().is_some_and(|s| s.inner() == source) {
            self.primary_source = None;
        }
    }
}

impl ProvidesRegistryState for Runtime {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState, SeatState];
}

impl Dispatch<WpViewporter, ()> for Runtime {
    fn event(_: &mut Self, _: &WpViewporter, _: <WpViewporter as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WpViewport, ()> for Runtime {
    fn event(_: &mut Self, _: &WpViewport, _: <WpViewport as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WpFractionalScaleManagerV1, ()> for Runtime {
    fn event(_: &mut Self, _: &WpFractionalScaleManagerV1, _: <WpFractionalScaleManagerV1 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WpFractionalScaleV1, ()> for Runtime {
    fn event(rt: &mut Self, _: &WpFractionalScaleV1, event: wp_fractional_scale_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            if scale != rt.scale120 {
                rt.scale120 = scale;
                rt.full_redraw = true;
                rt.request_render();
            }
        }
    }
}

delegate_compositor!(Runtime);
delegate_output!(Runtime);
delegate_seat!(Runtime);
delegate_keyboard!(Runtime);
delegate_pointer!(Runtime);
delegate_xdg_shell!(Runtime);
delegate_xdg_window!(Runtime);
delegate_data_device!(Runtime);
delegate_primary_selection!(Runtime);
delegate_registry!(Runtime);

#[cfg(test)]
mod tests {
    #[test]
    fn us_key_names() {
        use super::us_char;
        assert_eq!(us_char(31), Some('s'), "KEY_S");
        assert_eq!(us_char(16), Some('q'));
        assert_eq!(us_char(11), Some('0'));
        assert_eq!(us_char(53), Some('/'));
        assert_eq!(us_char(43), Some('\\'));
        assert_eq!(us_char(28), None, "Enter is not a character");
        assert_eq!(us_char(1), None);
    }
}
