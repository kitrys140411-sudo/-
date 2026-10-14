//! XWayland: X11 apps (Steam, Wine/Proton, SDL games, older toolkits).
//!
//! Xwayland is started right after the first frame is on screen, not lazily
//! on the first X connection: smithay 0.7 creates the X11 sockets itself when
//! it spawns the server and its client data cannot be made by a compositor,
//! so a lazy start would mean re-implementing the spawn. It is cheap enough
//! not to bother (measured idle, software rendering: ready 50-80 ms after the
//! spawn, RSS ~47 MB of which ~12 MB private, PSS ~29 MB; `xwayland` on the
//! control socket reports it), and deferring it past the first frame keeps
//! it off the boot path. `LUMEN_XWAYLAND=0` turns it off.
//!
//! Window management: managed X11 windows become [`crate::wm::Win`]s like
//! xdg toplevels (placement, focus, stacking, move/resize, maximize and
//! fullscreen through _NET_WM_STATE, close through WM_DELETE_WINDOW). The X
//! server is always told where a window really is, so clients that place
//! their own menus from their window position get them right. Override-
//! redirect windows (menus, tooltips, drag icons) are not managed: they are
//! drawn above all windows exactly where the client put them.
//!
//! Selections: the clipboard and the primary selection are bridged both ways.
//! X clients may read a Wayland selection only while an X11 window has
//! keyboard focus (pasting into the focused app), like a Wayland client.
//!
//! HiDPI: Xwayland runs with client scale 1. X11 windows are laid out in
//! logical pixels and scaled up by the compositor on outputs with scale > 1
//! (correct size, softer text); games that render at the logical resolution
//! cost less on a 4K panel this way.

use std::os::unix::net::UnixStream;
use std::process::Stdio;
use std::time::{Duration, Instant};

use smithay::desktop::Window;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::RegistrationToken;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Client;
use smithay::utils::{Logical, Rectangle, Size};
use smithay::wayland::selection::data_device::{
    clear_data_device_selection, current_data_device_selection_userdata, request_data_device_client_selection,
    set_data_device_selection,
};
use smithay::wayland::selection::primary_selection::{
    clear_primary_selection, current_primary_selection_userdata, request_primary_client_selection, set_primary_selection,
};
use smithay::wayland::selection::SelectionTarget;
use smithay::wayland::xwayland_shell::{XWaylandShellHandler, XWaylandShellState};
use smithay::xwayland::xwm::{Reorder, ResizeEdge, WmWindowProperty, WmWindowType, XwmId};
use smithay::xwayland::{X11Surface, X11Wm, XWayland, XWaylandEvent, XwmHandler};

use crate::focus::FocusTarget;
use crate::state::Lumen;
use crate::wm::{same_x11, Layout};

/// Give up restarting a crashing Xwayland after this many failures.
const MAX_FAILURES: u32 = 3;

pub struct XwaylandState {
    /// The xwayland_shell_v1 global (only Xwayland can bind it).
    pub shell: XWaylandShellState,
    pub wm: Option<X11Wm>,
    /// Display number once Xwayland was spawned. X clients may connect from
    /// then on: they are accepted once the window manager is attached.
    pub display: Option<u32>,
    /// The window manager is attached and the X server is alive.
    pub ready: bool,
    client: Option<Client>,
    token: Option<RegistrationToken>,
    spawned: Option<Instant>,
    /// Time from spawning to the window manager being attached.
    pub ready_after: Option<Duration>,
    pub pid: Option<i32>,
    pub failures: u32,
    pub enabled: bool,
    pub error: Option<String>,
    /// Mime types of the clipboard and primary selection a Wayland client
    /// set last (None: X or nobody owns it), handed to a window manager
    /// that attaches later (Xwayland starting, or restarted).
    pub wl_selection: [Option<Vec<String>>; 2],
}

/// Index into [`XwaylandState::wl_selection`].
pub fn selection_index(ty: SelectionTarget) -> usize {
    match ty {
        SelectionTarget::Clipboard => 0,
        SelectionTarget::Primary => 1,
    }
}

impl XwaylandState {
    pub fn new(shell: XWaylandShellState) -> XwaylandState {
        let enabled = !matches!(std::env::var("LUMEN_XWAYLAND").as_deref(), Ok("0" | "off" | "false" | "no"));
        XwaylandState {
            shell,
            wm: None,
            display: None,
            ready: false,
            client: None,
            token: None,
            spawned: None,
            ready_after: None,
            pid: None,
            failures: 0,
            enabled,
            error: None,
            wl_selection: [None, None],
        }
    }

    /// `DISPLAY` for X clients started by the compositor.
    pub fn display_env(&self) -> Option<String> {
        self.display.map(|d| format!(":{d}"))
    }

    /// Resident memory of the Xwayland process in KiB (from /proc).
    pub fn rss_kib(&self) -> Option<u64> {
        let status = std::fs::read_to_string(format!("/proc/{}/status", self.pid?)).ok()?;
        kib_field(&status, "VmRSS:")
    }

    /// What Xwayland really costs in KiB: its proportional share of resident
    /// memory (RSS counts all of Mesa and LLVM, which the compositor and
    /// every GL client map too) and its private memory.
    pub fn pss_private_kib(&self) -> Option<(u64, u64)> {
        let rollup = std::fs::read_to_string(format!("/proc/{}/smaps_rollup", self.pid?)).ok()?;
        let private = kib_field(&rollup, "Private_Clean:")? + kib_field(&rollup, "Private_Dirty:")?;
        Some((kib_field(&rollup, "Pss:")?, private))
    }
}

/// The newest child process of the compositor named Xwayland (smithay keeps
/// the child handle to itself).
fn xwayland_child_pid() -> Option<i32> {
    let me = std::process::id().to_string();
    let mut best: Option<(u64, i32)> = None;
    for entry in std::fs::read_dir("/proc").ok()?.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse::<i32>().ok()) else { continue };
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else { continue };
        // "pid (comm) state ppid ... starttime(22nd field) ..."; comm may contain spaces.
        let Some((comm, rest)) = stat.split_once(" (").and_then(|(_, r)| r.rsplit_once(") ")) else { continue };
        let fields: Vec<&str> = rest.split_whitespace().collect();
        if comm != "Xwayland" || fields.get(1) != Some(&me.as_str()) {
            continue;
        }
        let start = fields.get(19).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
        if best.is_none_or(|(s, _)| start >= s) {
            best = Some((start, pid));
        }
    }
    best.map(|(_, pid)| pid)
}

/// A "Name:   1234 kB" line of a /proc file, in KiB.
fn kib_field(text: &str, name: &str) -> Option<u64> {
    text.lines().find_map(|l| l.strip_prefix(name)).and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())
}

/// X11 sockets live in /tmp/.X11-unix (sticky, world-writable, like /tmp).
/// Init normally creates it; a compositor started elsewhere makes it itself.
fn ensure_x11_socket_dir() {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let dir = std::path::Path::new("/tmp/.X11-unix");
    if dir.is_dir() {
        return;
    }
    match std::fs::DirBuilder::new().mode(0o1777).create(dir) {
        // The umask narrowed the mode: set it explicitly.
        Ok(()) => {
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o1777));
        }
        Err(e) => tracing::warn!("cannot create {}: {e}", dir.display()),
    }
}

/// Environment passed on to Xwayland (smithay clears all but PATH and
/// XDG_RUNTIME_DIR): driver selection and debugging knobs.
fn xwayland_env() -> Vec<(String, String)> {
    std::env::vars()
        .filter(|(k, _)| {
            k.starts_with("MESA_") || k.starts_with("LIBGL_") || k.starts_with("GBM_") || k.starts_with("__GLX_") || k == "DRI_PRIME"
                || k.starts_with("XWAYLAND_") || k.starts_with("XKB_") || k == "HOME" || k == "LANG"
        })
        .collect()
}

impl Lumen {
    /// Spawn Xwayland (if enabled and installed). Called after the first frame.
    pub fn start_xwayland(&mut self) {
        let x = &mut self.xwayland;
        if !x.enabled || x.client.is_some() || x.failures >= MAX_FAILURES {
            return;
        }
        ensure_x11_socket_dir();
        // Its log is useful for debugging X apps but noisy (-verbose): it
        // goes next to the control socket, not into the compositor's log.
        let log_path = crate::ipc::socket_path().with_file_name("xwayland.log");
        let stderr: Stdio = std::fs::File::create(&log_path).map(Stdio::from).unwrap_or_else(|_| Stdio::null());
        let spawned = XWayland::spawn(&self.dh, None, xwayland_env(), true, Stdio::null(), stderr, |_| ());
        let (xwayland, client) = match spawned {
            Ok(v) => v,
            Err(e) => {
                let msg = if e.kind() == std::io::ErrorKind::NotFound {
                    "Xwayland is not installed: X11 apps cannot run".to_string()
                } else {
                    format!("cannot start Xwayland: {e}")
                };
                tracing::warn!("{msg}");
                self.xwayland.error = Some(msg);
                self.xwayland.enabled = false;
                return;
            }
        };
        let display = xwayland.display_number();
        // (Not the Wayland client's credentials: Xwayland's connection is
        // one end of a socketpair made here, so those name the compositor.)
        let pid = xwayland_child_pid();
        let token = self.lh.insert_source(xwayland, |event, _, state| match event {
            XWaylandEvent::Ready { x11_socket, display_number } => state.xwayland_ready(x11_socket, display_number),
            XWaylandEvent::Error => state.xwayland_lost("Xwayland exited during startup"),
        });
        let x = &mut self.xwayland;
        match token {
            Ok(t) => {
                x.token = Some(t);
                x.client = Some(client);
                x.display = Some(display);
                x.pid = pid;
                x.spawned = Some(Instant::now());
                let name = format!(":{display}");
                tracing::info!(display = %name, pid, "Xwayland starting");
            }
            Err(e) => tracing::warn!("cannot watch Xwayland: {}", e.error),
        }
    }

    fn xwayland_ready(&mut self, socket: UnixStream, display: u32) {
        let Some(client) = self.xwayland.client.clone() else { return };
        let mut wm = match X11Wm::start_wm(self.lh.clone(), socket, client) {
            Ok(wm) => wm,
            Err(e) => {
                self.xwayland_lost(&format!("cannot attach the X11 window manager: {e}"));
                return;
            }
        };
        // X windows without a cursor of their own show ours, not X's.
        if let Some(c) = self.cursor_theme.x11_default() {
            if let Err(e) = wm.set_cursor(&c.bgra, Size::from((c.width, c.height)), (c.hot_x, c.hot_y).into()) {
                tracing::debug!("X11 default cursor: {e}");
            }
        }
        if let Err(e) = wm.set_randr_primary_output(self.primary_output()) {
            tracing::debug!("X11 primary output: {e:?}");
        }
        for ty in [SelectionTarget::Clipboard, SelectionTarget::Primary] {
            if let Some(mime_types) = self.xwayland.wl_selection[selection_index(ty)].clone() {
                if let Err(e) = wm.new_selection(ty, Some(mime_types)) {
                    tracing::debug!(?ty, "cannot pass the selection to X11: {e}");
                }
            }
        }
        let x = &mut self.xwayland;
        x.wm = Some(wm);
        x.ready = true;
        x.ready_after = x.spawned.map(|t| t.elapsed());
        let name = format!(":{display}");
        tracing::info!(
            display = %name,
            ready_ms = x.ready_after.map(|d| d.as_millis() as u64),
            rss_kib = x.rss_kib(),
            pss_kib = x.pss_private_kib().map(|m| m.0),
            "Xwayland ready"
        );
        self.x11_restack();
    }

    /// Xwayland went away (crashed, or failed to start): forget its windows
    /// and start a new one a moment later, a few times at most.
    fn xwayland_lost(&mut self, why: &str) {
        tracing::warn!("{why}");
        let ids: Vec<u32> = self.wm.windows.iter().filter(|w| w.x11().is_some()).map(|w| w.id).collect();
        for id in ids {
            self.window_gone(id);
        }
        self.wm.overrides.clear();
        let x = &mut self.xwayland;
        x.ready = false;
        x.display = None;
        x.pid = None;
        x.client = None;
        x.failures += 1;
        x.error = Some(why.to_string());
        if let Some(t) = x.token.take() {
            // Possibly called from that very source's callback: remove it later.
            let lh = self.lh.clone();
            self.lh.insert_idle(move |_| lh.remove(t));
        }
        if self.xwayland.failures < MAX_FAILURES {
            let _ = self.lh.insert_source(Timer::from_duration(Duration::from_secs(1)), |_, _, state| {
                state.start_xwayland();
                TimeoutAction::Drop
            });
        } else {
            self.notify("X11 apps unavailable", "Xwayland keeps failing; see xwayland.log in the runtime directory.");
        }
        self.queue_redraw_all();
    }

    /// Mirror our stacking order of X11 windows in the X server (it decides
    /// which X window gets input where windows of one client overlap, and
    /// clients read it through _NET_CLIENT_LIST_STACKING).
    pub fn x11_restack(&mut self) {
        let Some(wm) = self.xwayland.wm.as_mut() else { return };
        let order: Vec<X11Surface> = self.wm.windows.iter().filter_map(|w| w.x11().cloned()).filter(|x| x.alive()).collect();
        // Our list is bottom to top. smithay's "upwards" variant is the one
        // that takes that order (it moves each window above the previous
        // one); despite its documentation, "downwards" expects top to bottom.
        if let Err(e) = wm.update_stacking_order_upwards(order.iter()) {
            tracing::debug!("X11 restack: {e}");
        }
    }

    /// Whether the keyboard focus is on an X11 window.
    pub fn x11_focused(&self) -> bool {
        self.seat.get_keyboard().and_then(|k| k.current_focus()).is_some_and(|f| f.x11().is_some())
    }

    /// Place a newly mapped X11 window: centered (dialogs over their parent),
    /// clamped to the work area, or fullscreen when it asked for that or is
    /// as large as the output (games that predate _NET_WM_STATE_FULLSCREEN
    /// make an undecorated window the size of the screen).
    fn manage_x11(&mut self, window: X11Surface) {
        if self.wm.find_x11(&window).is_some() {
            return;
        }
        let id = self.wm.add(Window::new_x11_window(window.clone()));
        let (work, out) = (self.work_area(), self.output_rect());
        let geo = window.geometry();
        let dialog = window.is_transient_for().is_some() || matches!(window.window_type(), Some(WmWindowType::Dialog));
        let covers_output = geo.size.w as f64 >= out.size.w && geo.size.h as f64 >= out.size.h;
        let size = Size::<i32, Logical>::from((
            geo.size.w.clamp(1, work.size.w.max(1.0) as i32),
            geo.size.h.clamp(1, work.size.h.max(1.0) as i32),
        ));
        let loc = self.wm.place_window(id, size, work);
        if let Some(w) = self.wm.get_mut(id) {
            w.placed = true;
            w.requested_size = Some(size);
            w.target = loc;
            w.pos.snap(loc.x, loc.y);
        }
        if window.is_fullscreen() || (covers_output && !dialog) {
            self.wm.set_layout(id, Layout::Fullscreen { restore: work }, work, out);
        } else if window.is_maximized() {
            self.wm.set_layout(id, Layout::Maximized { restore: work }, work, out);
        } else {
            self.wm.configure_x11(id);
        }
        tracing::debug!(id, title = %window.title(), class = %window.class(), "X11 window managed");
    }

    /// The X11 window of a managed window id.
    fn x11_id(&self, window: &X11Surface) -> Option<u32> {
        self.wm.find_x11(window)
    }

    /// A client-initiated move or resize (_NET_WM_MOVERESIZE): only while a
    /// pointer button is held, as for xdg toplevels.
    fn x11_grab(&mut self, window: &X11Surface, edges: Option<ResizeEdge>) {
        let Some(id) = self.x11_id(window) else { return };
        match edges {
            None => self.start_move_grab(id, None),
            Some(e) => {
                let (left, right, top, bottom) = match e {
                    ResizeEdge::Left => (true, false, false, false),
                    ResizeEdge::Right => (false, true, false, false),
                    ResizeEdge::Top => (false, false, true, false),
                    ResizeEdge::Bottom => (false, false, false, true),
                    ResizeEdge::TopLeft => (true, false, true, false),
                    ResizeEdge::TopRight => (false, true, true, false),
                    ResizeEdge::BottomLeft => (true, false, false, true),
                    ResizeEdge::BottomRight => (false, true, false, true),
                };
                self.start_resize_grab(id, None, left, right, top, bottom);
            }
        }
    }

    fn x11_set_layout(&mut self, window: &X11Surface, layout: Layout) {
        let Some(id) = self.x11_id(window) else { return };
        let (work, out) = (self.work_area(), self.output_rect());
        self.wm.set_layout(id, layout, work, out);
        self.queue_redraw_all();
    }
}

impl XWaylandShellHandler for Lumen {
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
        &mut self.xwayland.shell
    }

    fn surface_associated(&mut self, _xwm: XwmId, _surface: WlSurface, _window: X11Surface) {
        self.queue_redraw_all();
    }
}

impl XwmHandler for Lumen {
    fn xwm_state(&mut self, _xwm: XwmId) -> &mut X11Wm {
        // Event sources of a window manager only run while it exists; a
        // replaced one is kept until the new one takes over.
        self.xwayland.wm.as_mut().expect("X11 event without a window manager")
    }

    fn new_window(&mut self, _xwm: XwmId, _window: X11Surface) {}

    fn new_override_redirect_window(&mut self, _xwm: XwmId, _window: X11Surface) {}

    fn map_window_request(&mut self, _xwm: XwmId, window: X11Surface) {
        if let Err(e) = window.set_mapped(true) {
            tracing::warn!("cannot map X11 window: {e}");
            return;
        }
        self.manage_x11(window);
    }

    fn mapped_override_redirect_window(&mut self, _xwm: XwmId, window: X11Surface) {
        self.wm.overrides.retain(|o| !same_x11(o, &window));
        self.wm.overrides.push(window);
        self.refresh_pointer_focus();
        self.queue_redraw_all();
    }

    fn unmapped_window(&mut self, _xwm: XwmId, window: X11Surface) {
        if window.is_override_redirect() {
            self.wm.overrides.retain(|o| !same_x11(o, &window));
            self.refresh_pointer_focus();
            self.queue_redraw_all();
            return;
        }
        if let Some(id) = self.x11_id(&window) {
            self.window_gone(id);
        }
        let _ = window.set_mapped(false);
    }

    fn destroyed_window(&mut self, _xwm: XwmId, window: X11Surface) {
        let before = self.wm.overrides.len();
        self.wm.overrides.retain(|o| !same_x11(o, &window));
        if let Some(id) = self.x11_id(&window) {
            self.window_gone(id);
        } else if before != self.wm.overrides.len() {
            self.queue_redraw_all();
        }
    }

    fn configure_request(
        &mut self,
        _xwm: XwmId,
        window: X11Surface,
        x: Option<i32>,
        y: Option<i32>,
        w: Option<u32>,
        h: Option<u32>,
        _reorder: Option<Reorder>,
    ) {
        let Some(id) = self.x11_id(&window) else {
            // Not managed (yet): the client may arrange its window as it likes.
            let mut geo = window.geometry();
            geo.loc.x = x.unwrap_or(geo.loc.x);
            geo.loc.y = y.unwrap_or(geo.loc.y);
            geo.size.w = w.map(|v| v as i32).unwrap_or(geo.size.w).max(1);
            geo.size.h = h.map(|v| v as i32).unwrap_or(geo.size.h).max(1);
            let _ = window.configure(geo);
            return;
        };
        let (work, out) = (self.work_area(), self.output_rect());
        let Some(win) = self.wm.get(id) else { return };
        if !matches!(win.layout, Layout::Floating) {
            // Maximized, tiled and fullscreen windows keep the geometry the
            // layout gives them (smithay re-sends it as a synthetic configure).
            return;
        }
        let cur = window.geometry();
        let size = Size::<i32, Logical>::from((
            w.map(|v| v as i32).unwrap_or(cur.size.w).max(1),
            h.map(|v| v as i32).unwrap_or(cur.size.h).max(1),
        ));
        if size.w as f64 >= out.size.w && size.h as f64 >= out.size.h {
            // A game switching to "fullscreen" by covering the screen.
            self.wm.set_layout(id, Layout::Fullscreen { restore: work }, work, out);
            self.queue_redraw_all();
            return;
        }
        let size = Size::from((size.w.min(work.size.w as i32), size.h.min(work.size.h as i32)));
        let mapped = win.mapped;
        let mut loc = win.target;
        if !mapped {
            // Before the window is shown it may still choose its position.
            if let (Some(x), Some(y)) = (x, y) {
                loc = (x as f64, y as f64).into();
            }
        }
        let loc = (
            loc.x.min(work.loc.x + work.size.w - size.w as f64).max(work.loc.x),
            loc.y.min(work.loc.y + work.size.h - size.h as f64).max(work.loc.y),
        );
        if let Some(win) = self.wm.get_mut(id) {
            win.requested_size = Some(size);
        }
        self.wm.set_target(id, loc.into(), mapped);
        self.wm.configure_x11(id);
        self.queue_redraw_all();
    }

    fn configure_notify(&mut self, _xwm: XwmId, window: X11Surface, _geometry: Rectangle<i32, Logical>, _above: Option<u32>) {
        if window.is_override_redirect() {
            // Menus and tooltips move themselves.
            self.refresh_pointer_focus();
            self.queue_redraw_all();
        }
    }

    fn property_notify(&mut self, _xwm: XwmId, _window: X11Surface, property: WmWindowProperty) {
        if matches!(property, WmWindowProperty::Title | WmWindowProperty::Class) {
            self.shell.on_windows_changed();
            self.queue_redraw_all();
        }
    }

    fn maximize_request(&mut self, _xwm: XwmId, window: X11Surface) {
        let work = self.work_area();
        self.x11_set_layout(&window, Layout::Maximized { restore: work });
    }

    fn unmaximize_request(&mut self, _xwm: XwmId, window: X11Surface) {
        self.x11_set_layout(&window, Layout::Floating);
    }

    fn fullscreen_request(&mut self, _xwm: XwmId, window: X11Surface) {
        if self.x11_id(&window).is_none() {
            // Asked before mapping: remembered in its state, applied on map.
            let _ = window.set_fullscreen(true);
            return;
        }
        let work = self.work_area();
        self.x11_set_layout(&window, Layout::Fullscreen { restore: work });
    }

    fn unfullscreen_request(&mut self, _xwm: XwmId, window: X11Surface) {
        if self.x11_id(&window).is_none() {
            let _ = window.set_fullscreen(false);
            return;
        }
        self.x11_set_layout(&window, Layout::Floating);
    }

    fn resize_request(&mut self, _xwm: XwmId, window: X11Surface, _button: u32, edges: ResizeEdge) {
        self.x11_grab(&window, Some(edges));
    }

    fn move_request(&mut self, _xwm: XwmId, window: X11Surface, _button: u32) {
        self.x11_grab(&window, None);
    }

    fn allow_selection_access(&mut self, _xwm: XwmId, _selection: SelectionTarget) -> bool {
        self.x11_focused()
    }

    fn send_selection(&mut self, _xwm: XwmId, selection: SelectionTarget, mime_type: String, fd: std::os::fd::OwnedFd) {
        let res = match selection {
            SelectionTarget::Clipboard => request_data_device_client_selection(&self.seat, mime_type, fd).map_err(|e| e.to_string()),
            SelectionTarget::Primary => request_primary_client_selection(&self.seat, mime_type, fd).map_err(|e| e.to_string()),
        };
        if let Err(e) = res {
            tracing::debug!(?selection, "Wayland selection for X11: {e}");
        }
    }

    fn new_selection(&mut self, _xwm: XwmId, selection: SelectionTarget, mime_types: Vec<String>) {
        tracing::debug!(?selection, ?mime_types, "X11 selection");
        self.xwayland.wl_selection[selection_index(selection)] = None;
        match selection {
            SelectionTarget::Clipboard => set_data_device_selection(&self.dh, &self.seat, mime_types, ()),
            SelectionTarget::Primary => set_primary_selection(&self.dh, &self.seat, mime_types, ()),
        }
    }

    fn cleared_selection(&mut self, _xwm: XwmId, selection: SelectionTarget) {
        // Only clear what X owned (a Wayland client may have taken over since).
        match selection {
            SelectionTarget::Clipboard => {
                if current_data_device_selection_userdata(&self.seat).is_some() {
                    clear_data_device_selection(&self.dh, &self.seat);
                }
            }
            SelectionTarget::Primary => {
                if current_primary_selection_userdata(&self.seat).is_some() {
                    clear_primary_selection(&self.dh, &self.seat);
                }
            }
        }
    }

    fn disconnected(&mut self, _xwm: XwmId) {
        self.xwayland_lost("Xwayland connection lost");
    }
}

/// Keyboard focus for a window: X11 windows through their X11 surface (so the
/// X server's input focus follows), Wayland windows through their surface.
pub fn focus_for(w: &crate::wm::Win) -> Option<FocusTarget> {
    match w.x11() {
        Some(x) => Some(FocusTarget::X11(x.clone())),
        None => w.surface().map(FocusTarget::Surface),
    }
}

#[cfg(test)]
mod tests {
    use super::kib_field;

    #[test]
    fn proc_kib_fields() {
        let rollup = "55d0-7ffc ---p 00000000 00:00 0 [rollup]\nRss:              177788 kB\nPss:               41234 kB\nPrivate_Clean:      1200 kB\nPrivate_Dirty:     20480 kB\n";
        assert_eq!(kib_field(rollup, "Rss:"), Some(177788));
        assert_eq!(kib_field(rollup, "Pss:"), Some(41234));
        assert_eq!(kib_field(rollup, "Private_Dirty:"), Some(20480));
        assert_eq!(kib_field(rollup, "Swap:"), None);
    }
}
