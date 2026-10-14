//! lumen-testclient — a small, scriptable Wayland client for automated compositor tests.
//!
//! It maps one xdg_toplevel drawn with shm buffers and reports what the compositor does
//! (configures, scale, input, clipboard, presentation) on stdout, one `key=value` line per
//! event (see `out.rs` and `--help`). Errors go to stderr. Exit status: 0 on a normal exit
//! (toplevel closed, `--exit-after-ms`, `--close-on-key`), 1 when the compositor goes away
//! or a Wayland error occurs, 2 on bad arguments.

mod args;
mod gaming;
mod handlers;
mod out;
mod paint;
mod x11;

use std::io::{Read, Write};
use std::os::fd::OwnedFd;
use std::process::ExitCode;
use std::time::Duration;

use sctk::compositor::{CompositorState, Surface, SurfaceData};
use sctk::data_device_manager::data_device::DataDevice;
use sctk::data_device_manager::data_source::CopyPasteSource;
use sctk::data_device_manager::{DataDeviceManagerState, WritePipe};
use sctk::output::OutputState;
use sctk::reexports::calloop::timer::{TimeoutAction, Timer};
use sctk::reexports::calloop::{EventLoop, LoopHandle, PostAction};
use sctk::reexports::calloop_wayland_source::WaylandSource;
use sctk::reexports::client::globals::registry_queue_init;
use sctk::reexports::client::protocol::{wl_keyboard, wl_pointer, wl_seat, wl_shm, wl_surface};
use sctk::reexports::client::{Connection, Proxy, QueueHandle};
use sctk::reexports::protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::{
    Shape, WpCursorShapeDeviceV1,
};
use sctk::reexports::protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1;
use sctk::reexports::protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::WpFractionalScaleV1;
use sctk::reexports::protocols::wp::presentation_time::client::wp_presentation::WpPresentation;
use sctk::reexports::protocols::wp::viewporter::client::wp_viewport::WpViewport;
use sctk::reexports::protocols::wp::viewporter::client::wp_viewporter::WpViewporter;
use sctk::reexports::protocols::xdg::shell::client::xdg_positioner::{
    Anchor, ConstraintAdjustment, Gravity,
};
use sctk::registry::RegistryState;
use sctk::seat::SeatState;
use sctk::seat::pointer::cursor_shape::CursorShapeManager;
use sctk::shell::WaylandSurface;
use sctk::shell::xdg::popup::Popup;
use sctk::shell::xdg::window::{Window, WindowDecorations};
use sctk::shell::xdg::{XdgPositioner, XdgShell, XdgSurface};
use sctk::shm::Shm;
use sctk::shm::slot::{Buffer, SlotPool};
use smithay_client_toolkit as sctk;

use args::{Command, CursorName, InitialState, Options};
use out::{emit, escape};
use paint::{Layout, Rect, Rgb};

const POPUP_SIZE: (u32, u32) = (200, 120);

fn main() -> ExitCode {
    let mut argv = Vec::new();
    for arg in std::env::args_os().skip(1) {
        match arg.into_string() {
            Ok(a) => argv.push(a),
            Err(a) => {
                eprintln!("lumen-testclient: argument is not valid UTF-8: {a:?}");
                return ExitCode::from(2);
            }
        }
    }
    let opts = match args::parse(argv) {
        Ok(Command::Run(opts)) => opts,
        Ok(Command::Help) => {
            println!("{}", args::USAGE);
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("lumen-testclient: {e}\n\n{}", args::USAGE);
            return ExitCode::from(2);
        }
    };
    let result = if opts.x11 { x11::run(&opts) } else { run(opts) };
    match result {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("lumen-testclient: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The shm buffers (at most two: double buffering) and optional viewport of one surface.
#[derive(Default)]
struct Target {
    buffers: Vec<Buffer>,
    viewport: Option<WpViewport>,
}

struct PopupState {
    popup: Popup,
    target: Target,
    size: (u32, u32),
    configured: bool,
    dirty: bool,
}

pub(crate) struct App {
    conn: Connection,
    qh: QueueHandle<App>,
    loop_handle: LoopHandle<'static, App>,
    opts: Options,
    /// Set to the exit status once the client should stop.
    exit: Option<u8>,

    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    compositor: CompositorState,
    shm: Shm,
    xdg_shell: XdgShell,
    data_device_manager: Option<DataDeviceManagerState>,
    cursor_shape_manager: Option<CursorShapeManager>,
    presentation: Option<WpPresentation>,
    viewporter: Option<WpViewporter>,

    pool: SlotPool,
    window: Window,
    main: Target,
    /// Present only when fractional scaling is in use (`--fractional` and both globals).
    fractional: Option<WpFractionalScaleV1>,
    configured: bool,
    /// Current logical size (from the last configure).
    size: (u32, u32),
    /// Logical size of the last committed buffer, i.e. the window geometry the compositor
    /// knows about (popups are positioned relative to it).
    mapped_size: (u32, u32),
    /// Size to use when the compositor sends 0x0 (last size while not maximized/tiled).
    floating_size: (u32, u32),
    /// Preferred fractional scale in 1/120 units, once the compositor sent one.
    scale120: Option<u32>,
    /// Integer buffer scale (used without fractional scaling).
    int_scale: u32,
    /// Whether the compositor sent wl_surface.preferred_buffer_scale; until then the
    /// integer scale follows the scale of the outputs the surface is on.
    preferred_scale_seen: bool,
    /// Whether a `scale` line was printed yet.
    scale_reported: bool,
    ready: bool,
    main_dirty: bool,
    retry_scheduled: bool,
    /// Layout and animated bar of the last committed main buffer, for partial damage.
    last_layout: Option<Layout>,
    last_bar: Option<Rect>,
    step: u64,
    commit_seq: u64,
    frame_callback_pending: bool,

    seat: Option<wl_seat::WlSeat>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer: Option<wl_pointer::WlPointer>,
    cursor_device: Option<WpCursorShapeDeviceV1>,
    data_device: Option<DataDevice>,
    copy_source: Option<CopyPasteSource>,
    /// The toplevel (not the popup) has keyboard focus.
    main_focused: bool,
    /// `--paste`: focus was gained, the selection is read once the wl_display.sync sent
    /// on focus-in returns (the selection event may follow wl_keyboard.enter).
    paste_waiting: bool,
    popup: Option<PopupState>,
    gaming: gaming::Gaming,
}

fn run(opts: Options) -> Result<u8, String> {
    let conn = Connection::connect_to_env()
        .map_err(|e| format!("cannot connect to the Wayland compositor: {e}"))?;
    let (globals, mut event_queue) =
        registry_queue_init::<App>(&conn).map_err(|e| format!("wl_registry: {e}"))?;
    let qh = event_queue.handle();
    let mut event_loop: EventLoop<'static, App> =
        EventLoop::try_new().map_err(|e| format!("event loop: {e}"))?;
    let loop_handle = event_loop.handle();

    let compositor =
        CompositorState::bind(&globals, &qh).map_err(|e| format!("wl_compositor: {e}"))?;
    let xdg_shell = XdgShell::bind(&globals, &qh).map_err(|e| format!("xdg_wm_base: {e}"))?;
    let shm = Shm::bind(&globals, &qh).map_err(|e| format!("wl_shm: {e}"))?;
    let pool = SlotPool::new(opts.size.0 as usize * opts.size.1 as usize * 4 * 2, &shm)
        .map_err(|e| format!("shm pool: {e}"))?;
    let data_device_manager = DataDeviceManagerState::bind(&globals, &qh).ok();
    if data_device_manager.is_none() && (opts.copy.is_some() || opts.paste) {
        eprintln!("lumen-testclient: wl_data_device_manager not available, clipboard disabled");
    }
    let cursor_shape_manager = match opts.cursor_shape {
        Some(_) => CursorShapeManager::bind(&globals, &qh).ok(),
        None => None,
    };
    if opts.cursor_shape.is_some() && cursor_shape_manager.is_none() {
        eprintln!("lumen-testclient: wp_cursor_shape_manager_v1 not available, ignoring");
    }
    let presentation: Option<WpPresentation> = match opts.animate {
        true => globals.bind(&qh, 1..=2, ()).ok(),
        false => None,
    };
    let viewporter: Option<WpViewporter> = globals.bind(&qh, 1..=1, ()).ok();
    let fractional_manager: Option<WpFractionalScaleManagerV1> = match opts.fractional {
        true => globals.bind(&qh, 1..=1, ()).ok(),
        false => None,
    };

    let surface = compositor.create_surface(&qh);
    let (fractional, viewport) = match (&fractional_manager, &viewporter) {
        (Some(fm), Some(vp)) => (
            Some(fm.get_fractional_scale(&surface, &qh, ())),
            Some(vp.get_viewport(&surface, &qh, ())),
        ),
        _ => {
            if opts.fractional {
                eprintln!(
                    "lumen-testclient: wp_fractional_scale_manager_v1/wp_viewporter not \
                     available, using integer scaling"
                );
            }
            (None, None)
        }
    };
    let gaming = gaming::Gaming::bind(&globals, &qh, &opts);
    let window = xdg_shell.create_window(surface, WindowDecorations::ServerDefault, &qh);
    window.set_title(opts.title.clone());
    window.set_app_id(opts.app_id.clone());

    if let Some(ms) = opts.exit_after_ms {
        loop_handle
            .insert_source(
                Timer::from_duration(Duration::from_millis(ms)),
                |_, _, app| {
                    emit("exit reason=timeout");
                    app.exit = Some(0);
                    TimeoutAction::Drop
                },
            )
            .map_err(|e| format!("timer: {}", e.error))?;
    }

    let mut app = App {
        conn: conn.clone(),
        qh: qh.clone(),
        loop_handle,
        registry_state: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        output_state: OutputState::new(&globals, &qh),
        compositor,
        shm,
        xdg_shell,
        data_device_manager,
        cursor_shape_manager,
        presentation,
        viewporter,
        pool,
        window,
        main: Target {
            buffers: Vec::new(),
            viewport,
        },
        fractional,
        configured: false,
        size: opts.size,
        mapped_size: opts.size,
        floating_size: opts.size,
        scale120: None,
        int_scale: 1,
        preferred_scale_seen: false,
        scale_reported: false,
        ready: false,
        main_dirty: false,
        retry_scheduled: false,
        last_layout: None,
        last_bar: None,
        step: 0,
        commit_seq: 0,
        frame_callback_pending: false,
        seat: None,
        keyboard: None,
        pointer: None,
        cursor_device: None,
        data_device: None,
        copy_source: None,
        main_focused: false,
        paste_waiting: false,
        popup: None,
        gaming,
        exit: None,
        opts,
    };

    // Receive the seat capabilities and create the keyboard, pointer and data device before
    // the window is mapped: the compositor gives a new window keyboard focus (and sends the
    // clipboard selection) when it maps, and some compositors (smithay) do not resend the
    // selection to a data device created later.
    event_queue
        .roundtrip(&mut app)
        .map_err(|e| format!("initial roundtrip: {e}"))?;
    app.window.commit();
    WaylandSource::new(conn.clone(), event_queue)
        .insert(app.loop_handle.clone())
        .map_err(|e| format!("event loop: {}", e.error))?;

    while app.exit.is_none() {
        if let Err(e) = event_loop.dispatch(None, &mut app) {
            let detail = match conn.protocol_error() {
                Some(p) => format!("protocol error: {p}"),
                None => innermost_error(&e),
            };
            return Err(format!("connection to the compositor lost: {detail}"));
        }
    }
    let code = app.exit.unwrap_or(0);
    drop(app);
    let _ = conn.flush();
    Ok(code)
}

/// The most specific message of an error chain ("Broken pipe" rather than "other error").
fn innermost_error(e: &dyn std::error::Error) -> String {
    let mut e = e;
    while let Some(source) = e.source() {
        e = source;
    }
    e.to_string()
}

/// `ceil(len * scale120 / 120)`: buffer pixels for a logical length at a fractional scale.
fn scaled_len(len: u32, scale120: u32) -> u32 {
    ((u64::from(len) * u64::from(scale120)).div_ceil(120) as u32).max(1)
}

/// Returns the index of a buffer of the given size that the compositor does not hold,
/// creating one when fewer than two exist. Buffers of another size are dropped (sctk
/// destroys them right away, or once the compositor releases them).
fn acquire_buffer(
    pool: &mut SlotPool,
    buffers: &mut Vec<Buffer>,
    width: u32,
    height: u32,
    format: wl_shm::Format,
) -> Result<Option<usize>, String> {
    let stride = width as i32 * 4;
    buffers.retain(|b| b.stride() == stride && b.height() == height as i32);
    if let Some(i) = buffers.iter().position(|b| b.canvas(pool).is_some()) {
        return Ok(Some(i));
    }
    if buffers.len() >= 2 {
        return Ok(None);
    }
    let (buffer, _) = pool
        .create_buffer(
            width as i32,
            height as i32,
            stride,
            format,
        )
        .map_err(|e| format!("cannot create shm buffer: {e}"))?;
    buffers.push(buffer);
    Ok(Some(buffers.len() - 1))
}

fn damage(surface: &wl_surface::WlSurface, r: Rect) {
    if surface.version() >= 4 {
        surface.damage_buffer(r.x as i32, r.y as i32, r.w as i32, r.h as i32);
    } else {
        surface.damage(0, 0, i32::MAX, i32::MAX);
    }
}

fn cursor_shape(name: CursorName) -> Shape {
    match name {
        CursorName::Default => Shape::Default,
        CursorName::Pointer => Shape::Pointer,
        CursorName::Text => Shape::Text,
        CursorName::Grab => Shape::Grab,
        CursorName::Grabbing => Shape::Grabbing,
        CursorName::Move => Shape::Move,
        CursorName::Crosshair => Shape::Crosshair,
        CursorName::Wait => Shape::Wait,
        CursorName::NotAllowed => Shape::NotAllowed,
        CursorName::EwResize => Shape::EwResize,
        CursorName::NsResize => Shape::NsResize,
    }
}

fn popup_color(fill: Rgb) -> Rgb {
    const AMBER: Rgb = (0xe0, 0xaf, 0x68);
    const BLUE: Rgb = (0x7a, 0xa2, 0xf7);
    let dist = |a: Rgb, b: Rgb| {
        (i32::from(a.0) - i32::from(b.0)).abs()
            + (i32::from(a.1) - i32::from(b.1)).abs()
            + (i32::from(a.2) - i32::from(b.2)).abs()
    };
    if dist(fill, AMBER) < 96 { BLUE } else { AMBER }
}

impl App {
    /// Buffers of the main surface: XRGB8888 tells the compositor the window
    /// is opaque (what fullscreen direct scanout needs), as games' are.
    fn buffer_format(&self) -> wl_shm::Format {
        if self.opts.opaque { wl_shm::Format::Xrgb8888 } else { wl_shm::Format::Argb8888 }
    }

    /// Buffer pixels per logical pixel, and the fractional scale (x120) when it is in use.
    fn scale(&self) -> (f64, Option<u32>) {
        match self.fractional {
            Some(_) => {
                let s = self.scale120.unwrap_or(120);
                (f64::from(s) / 120.0, Some(s))
            }
            None => (f64::from(self.int_scale), None),
        }
    }

    fn buffer_size(&self, logical: (u32, u32)) -> (u32, u32) {
        match self.scale() {
            (_, Some(s120)) => (scaled_len(logical.0, s120), scaled_len(logical.1, s120)),
            (_, None) => (logical.0 * self.int_scale, logical.1 * self.int_scale),
        }
    }

    /// Prepares the surface scale state (viewport destination or buffer scale).
    fn apply_scale(
        &self,
        surface: &wl_surface::WlSurface,
        viewport: Option<&WpViewport>,
        logical: (u32, u32),
    ) {
        let fractional = self.scale().1.is_some();
        if let (true, Some(vp)) = (fractional, viewport) {
            vp.set_destination(logical.0 as i32, logical.1 as i32);
        }
        if surface.version() >= 3 {
            surface.set_buffer_scale(if fractional { 1 } else { self.int_scale as i32 });
        }
    }

    /// Paints and commits the next frame of the toplevel.
    fn draw_main(&mut self) {
        if !self.configured || self.exit.is_some() {
            return;
        }
        let (pw, ph) = self.buffer_size(self.size);
        let format = self.buffer_format();
        let idx = match acquire_buffer(&mut self.pool, &mut self.main.buffers, pw, ph, format) {
            Ok(Some(i)) => i,
            Ok(None) => {
                // Both buffers are still held by the compositor: try again shortly.
                self.main_dirty = true;
                self.schedule_retry();
                return;
            }
            Err(e) => {
                eprintln!("lumen-testclient: {e}");
                self.exit = Some(1);
                return;
            }
        };
        self.main_dirty = false;

        let (scale, _) = self.scale();
        let fill = self.opts.color;
        let layout = Layout::new(pw, ph, scale, true);
        let bar = if self.opts.animate {
            layout.bar(self.step, scale)
        } else {
            None
        };
        let canvas = self.main.buffers[idx]
            .canvas(&mut self.pool)
            .expect("buffer is free");
        let bar_color = paint::mix(fill, (255, 255, 255), 0.75);
        paint::paint(
            canvas,
            pw as usize * 4,
            &layout,
            fill,
            Some(paint::darker(fill)),
            bar.map(|r| (r, bar_color)),
        );
        if self.opts.stripes {
            paint::stripes(canvas, pw as usize * 4, &layout, scale);
        }

        let surface = self.window.wl_surface().clone();
        self.apply_scale(&surface, self.main.viewport.as_ref(), self.size);
        if self.last_layout == Some(layout) && self.opts.animate {
            // Only the animated bar moved: damage its old and new position.
            for r in [self.last_bar, bar].into_iter().flatten() {
                damage(&surface, r);
            }
        } else {
            damage(
                &surface,
                Rect {
                    x: 0,
                    y: 0,
                    w: pw,
                    h: ph,
                },
            );
        }
        self.last_layout = Some(layout);
        self.last_bar = bar;

        self.commit_seq += 1;
        if self.opts.animate {
            if !self.frame_callback_pending {
                surface.frame(&self.qh, surface.clone());
                self.frame_callback_pending = true;
            }
            if let Some(presentation) = &self.presentation {
                presentation.feedback(&surface, &self.qh, self.commit_seq);
            }
        }
        if let Err(e) = self.main.buffers[idx].attach_to(&surface) {
            eprintln!("lumen-testclient: attach: {e}");
        }
        if !self.ready {
            self.gaming_surface_ready();
        }
        surface.commit();
        self.mapped_size = self.size;
        if !self.ready {
            self.ready = true;
            let _ = self.conn.flush();
            emit("ready");
        }
    }

    fn draw_popup(&mut self) {
        let Some(p) = self.popup.as_ref() else { return };
        if !p.configured {
            return;
        }
        let logical = p.size;
        let (pw, ph) = self.buffer_size(logical);
        let (scale, _) = self.scale();
        let p = self.popup.as_mut().expect("checked above");
        let idx = match acquire_buffer(&mut self.pool, &mut p.target.buffers, pw, ph, wl_shm::Format::Argb8888) {
            Ok(Some(i)) => i,
            Ok(None) => {
                p.dirty = true;
                self.schedule_retry();
                return;
            }
            Err(e) => {
                eprintln!("lumen-testclient: {e}");
                return;
            }
        };
        p.dirty = false;
        let layout = Layout::new(pw, ph, scale, false);
        let canvas = p.target.buffers[idx]
            .canvas(&mut self.pool)
            .expect("buffer is free");
        paint::paint(
            canvas,
            pw as usize * 4,
            &layout,
            popup_color(self.opts.color),
            None,
            None,
        );

        let p = self.popup.as_ref().expect("checked above");
        let surface = p.popup.wl_surface().clone();
        self.apply_scale(&surface, p.target.viewport.as_ref(), logical);
        damage(
            &surface,
            Rect {
                x: 0,
                y: 0,
                w: pw,
                h: ph,
            },
        );
        if let Err(e) = p.target.buffers[idx].attach_to(&surface) {
            eprintln!("lumen-testclient: attach: {e}");
        }
        surface.commit();
    }

    fn schedule_retry(&mut self) {
        if self.retry_scheduled {
            return;
        }
        let timer = Timer::from_duration(Duration::from_millis(4));
        let inserted = self.loop_handle.insert_source(timer, |_, _, app| {
            app.retry_scheduled = false;
            if app.main_dirty {
                app.draw_main();
            }
            if app.popup.as_ref().is_some_and(|p| p.dirty) {
                app.draw_popup();
            }
            TimeoutAction::Drop
        });
        match inserted {
            Ok(_) => self.retry_scheduled = true,
            Err(e) => eprintln!("lumen-testclient: cannot schedule redraw: {}", e.error),
        }
    }

    /// Called for every frame callback of the toplevel.
    fn frame_done(&mut self) {
        self.frame_callback_pending = false;
        if !self.opts.animate {
            return;
        }
        if self.presentation.is_none() {
            emit(&format!(
                "frame seq={} presented_us=0 refresh_ns=0 flags=0",
                self.commit_seq
            ));
        }
        self.step += 1;
        self.draw_main();
    }

    fn set_fractional_scale(&mut self, scale120: u32) {
        if self.scale120 == Some(scale120) {
            return;
        }
        self.scale120 = Some(scale120);
        self.scale_reported = true;
        emit(&format!("scale value={}", f64::from(scale120) / 120.0));
        self.draw_main();
        self.draw_popup();
    }

    fn set_integer_scale(&mut self, factor: i32) {
        if self.fractional.is_some() {
            return; // the fractional scale wins
        }
        let factor = factor.clamp(1, 16) as u32;
        if factor == self.int_scale && self.scale_reported {
            return;
        }
        self.int_scale = factor;
        self.scale_reported = true;
        emit(&format!("scale value={factor}"));
        self.draw_main();
        self.draw_popup();
    }

    /// Called on the first configure: without fractional scaling, report the integer scale
    /// even if it is still 1 (sctk only reports preferred_buffer_scale when it differs
    /// from 1, so a test waiting for a `scale` line would otherwise wait forever).
    pub(crate) fn report_initial_scale(&mut self) {
        if !self.scale_reported && self.fractional.is_none() {
            self.scale_reported = true;
            emit(&format!("scale value={}", self.int_scale));
        }
    }

    /// Fallback for compositors that do not send wl_surface.preferred_buffer_scale: use
    /// the highest scale of the outputs the toplevel is on.
    fn update_output_scale(&mut self) {
        if self.preferred_scale_seen || self.fractional.is_some() {
            return;
        }
        let surface = self.window.wl_surface();
        let Some(data) = surface.data::<SurfaceData>() else {
            return;
        };
        let scale = data
            .outputs()
            .filter_map(|o| self.output_state.info(&o))
            .map(|info| info.scale_factor)
            .max();
        if let Some(scale) = scale {
            self.set_integer_scale(scale);
        }
    }

    /// Opens the `--popup` menu at (x, y) on the toplevel. `serial` is the serial of the
    /// button press that triggered it, used for the popup grab.
    fn open_popup(&mut self, x: f64, y: f64, serial: u32) {
        let positioner = match XdgPositioner::new(&self.xdg_shell) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("lumen-testclient: xdg_positioner: {e}");
                return;
            }
        };
        // The 1x1 anchor rectangle must lie inside the parent's window geometry, which is
        // the size of the last committed buffer (a newer configure may not be applied yet).
        let ax = (x.floor() as i32).clamp(0, self.mapped_size.0.max(1) as i32 - 1);
        let ay = (y.floor() as i32).clamp(0, self.mapped_size.1.max(1) as i32 - 1);
        positioner.set_size(POPUP_SIZE.0 as i32, POPUP_SIZE.1 as i32);
        positioner.set_anchor_rect(ax, ay, 1, 1);
        positioner.set_anchor(Anchor::TopLeft);
        positioner.set_gravity(Gravity::BottomRight);
        positioner.set_constraint_adjustment(
            ConstraintAdjustment::FlipX
                | ConstraintAdjustment::FlipY
                | ConstraintAdjustment::SlideX
                | ConstraintAdjustment::SlideY,
        );
        // Popup::new would commit right away, but the grab has to be requested before the
        // popup's initial commit, so build it from a plain surface and commit ourselves.
        let popup = Surface::new(&self.compositor, &self.qh).and_then(|surface| {
            Popup::from_surface(
                Some(self.window.xdg_surface()),
                &positioner,
                &self.qh,
                surface,
                &self.xdg_shell,
            )
        });
        let popup = match popup {
            Ok(p) => p,
            Err(e) => {
                eprintln!("lumen-testclient: xdg_popup: {e}");
                return;
            }
        };
        // An explicit grab makes it a real menu: the compositor dismisses it (popup_done)
        // when the user clicks outside this client's surfaces.
        if let Some(seat) = &self.seat {
            popup.xdg_popup().grab(seat, serial);
        }
        let viewport = match (&self.fractional, &self.viewporter) {
            (Some(_), Some(vp)) => Some(vp.get_viewport(popup.wl_surface(), &self.qh, ())),
            _ => None,
        };
        popup.wl_surface().commit();
        emit(&format!("popup event=open x={ax} y={ay}"));
        self.popup = Some(PopupState {
            popup,
            target: Target {
                buffers: Vec::new(),
                viewport,
            },
            size: POPUP_SIZE,
            configured: false,
            dirty: false,
        });
    }

    fn close_popup(&mut self, reason: &str) {
        if let Some(p) = self.popup.take() {
            if let Some(vp) = &p.target.viewport {
                vp.destroy();
            }
            drop(p);
            emit(&format!("popup event=closed reason={reason}"));
        }
    }

    /// `--copy`: makes our text the clipboard selection (unless we still own it).
    fn offer_selection(&mut self, serial: u32) {
        if self.copy_source.is_some() {
            return;
        }
        let (Some(manager), Some(device)) = (&self.data_device_manager, &self.data_device) else {
            return;
        };
        let source = manager.create_copy_paste_source(&self.qh, out::TEXT_MIME_TYPES);
        source.set_selection(device, serial);
        self.copy_source = Some(source);
        emit("copy event=offered");
    }

    /// `--copy`: a client asked for the selection contents.
    fn send_selection(&mut self, mime: String, pipe: WritePipe) {
        let text = self.opts.copy.clone().unwrap_or_default();
        emit(&format!("copy event=send mime={}", escape(&mime)));
        let fd: OwnedFd = pipe.into();
        // Write from a thread so a slow (or same-process) reader never blocks the loop.
        std::thread::spawn(move || {
            let mut file = std::fs::File::from(fd);
            if let Err(e) = file.write_all(text.as_bytes()) {
                eprintln!("lumen-testclient: writing the selection: {e}");
            }
        });
    }

    /// `--paste`: keyboard focus was gained. Smithay sends the selection right after
    /// wl_keyboard.enter (the protocol says before), so the current selection is only known
    /// for sure once a wl_display.sync sent now comes back.
    pub(crate) fn request_paste(&mut self) {
        self.paste_waiting = true;
        self.conn.display().sync(&self.qh, handlers::PasteSync);
    }

    /// `--paste`: reads the current selection (called when the sync of `request_paste`
    /// is done) and prints it once the source closed the pipe.
    pub(crate) fn paste(&mut self) {
        if !self.paste_waiting || !self.main_focused {
            return;
        }
        self.paste_waiting = false;
        let Some(device) = self.data_device.as_ref() else {
            emit("paste error=no-data-device");
            return;
        };
        let Some(offer) = device.data().selection_offer() else {
            emit("paste error=no-selection");
            return;
        };
        let Some(mime) = offer.with_mime_types(out::pick_text_mime) else {
            emit("paste error=no-text-mime");
            return;
        };
        let pipe = match offer.receive(mime) {
            Ok(pipe) => pipe,
            Err(e) => {
                eprintln!("lumen-testclient: cannot receive the selection: {e}");
                return;
            }
        };
        let mut data = Vec::new();
        let inserted = self.loop_handle.insert_source(pipe, move |(), file, _app| {
            let mut buf = [0u8; 4096];
            match (&**file).read(&mut buf) {
                Ok(0) => {
                    emit(&format!(
                        "paste text={}",
                        escape(&String::from_utf8_lossy(&data))
                    ));
                    PostAction::Remove
                }
                Ok(n) => {
                    data.extend_from_slice(&buf[..n]);
                    PostAction::Continue
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock
                    ) =>
                {
                    PostAction::Continue
                }
                Err(e) => {
                    eprintln!("lumen-testclient: reading the selection: {e}");
                    PostAction::Remove
                }
            }
        });
        if let Err(e) = inserted {
            eprintln!(
                "lumen-testclient: cannot watch the selection pipe: {}",
                e.error
            );
        }
    }

    fn request_initial_state(&self) {
        match self.opts.initial_state {
            Some(InitialState::Maximized) => self.window.set_maximized(),
            Some(InitialState::Fullscreen) => self.window.set_fullscreen(None),
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fractional_buffer_size() {
        assert_eq!(scaled_len(640, 120), 640);
        assert_eq!(scaled_len(640, 180), 960);
        assert_eq!(scaled_len(401, 150), 502); // 501.25 rounds up
        assert_eq!(scaled_len(1, 1), 1);
    }

    #[test]
    fn popup_color_differs_from_fill() {
        assert_ne!(popup_color((0x3b, 0x42, 0x61)), (0x3b, 0x42, 0x61));
        assert_ne!(popup_color((0xe0, 0xaf, 0x68)), (0xe0, 0xaf, 0x68));
    }
}
