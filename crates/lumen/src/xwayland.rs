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
//! Failures: an Xwayland that exits or hangs while starting, or crashes
//! later, is restarted a second later, up to 3 times in a row (a crash after
//! a minute of running starts the count afresh); then X11 apps are reported
//! unavailable. DISPLAY is passed on from the spawn until a failure.
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
use smithay::reexports::wayland_server::{Client, DisplayHandle};
use smithay::utils::{Logical, Rectangle, Size};
use smithay::wayland::selection::data_device::{
    clear_data_device_selection, current_data_device_selection_userdata, request_data_device_client_selection,
    set_data_device_selection,
};
use smithay::wayland::selection::primary_selection::{
    clear_primary_selection, current_primary_selection_userdata, request_primary_client_selection, set_primary_selection,
};
use smithay::wayland::selection::SelectionTarget;
use smithay::input::Seat;
use smithay::reexports::wayland_protocols::xwayland::keyboard_grab::zv1::server::zwp_xwayland_keyboard_grab_v1::ZwpXwaylandKeyboardGrabV1;
use smithay::reexports::wayland_server::Resource;
use smithay::utils::{IsAlive, SERIAL_COUNTER};
use smithay::wayland::xwayland_keyboard_grab::{XWaylandKeyboardGrab, XWaylandKeyboardGrabHandler, XWaylandKeyboardGrabState};
use smithay::wayland::xwayland_shell::{XWaylandShellHandler, XWaylandShellState};
use smithay::xwayland::xwm::{Reorder, ResizeEdge, WmWindowProperty, WmWindowType, XwmId};
use smithay::xwayland::{X11Surface, X11Wm, XWayland, XWaylandEvent, XwmHandler};

use crate::focus::FocusTarget;
use crate::state::Lumen;
use crate::wm::{same_x11, Layout, Override};

/// Restart a crashing Xwayland up to this many times in a row.
const MAX_RESTARTS: u32 = 3;
/// A crash after Xwayland ran this long starts counting afresh.
const STABLE_AFTER: Duration = Duration::from_secs(60);
/// An Xwayland that is not ready by then is killed and counts as a failure
/// (software rendering under emulation can take seconds to start).
const STARTUP_TIMEOUT: Duration = Duration::from_secs(60);

pub struct XwaylandState {
    /// The xwayland_shell_v1 global (only Xwayland can bind it).
    pub shell: XWaylandShellState,
    /// zwp_xwayland_keyboard_grab_manager_v1 (Xwayland only, too).
    _keyboard_grab: XWaylandKeyboardGrabState,
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
    /// When the window manager attached (crashes long after do not add up).
    ready_at: Option<Instant>,
    /// Failures in a row; Xwayland is restarted up to [`MAX_RESTARTS`] times.
    pub failures: u32,
    pub enabled: bool,
    pub error: Option<String>,
    /// Mime types of the clipboard and primary selection a Wayland client
    /// set last (None: X or nobody owns it), handed to a window manager
    /// that attaches later (Xwayland starting, or restarted).
    pub wl_selection: [Option<Vec<String>>; 2],
    /// An X client's keyboard grab the compositor honours (see [`X11KeyboardGrab`]).
    pub keyboard_grab: Option<X11KeyboardGrab>,
}

/// An X client grabbed the keyboard (XGrabKeyboard) on an override-redirect
/// window, a menu or an unmanaged fullscreen game, and Xwayland asked for
/// the keys (zwp_xwayland_keyboard_grab_v1). Unlike smithay's own grab
/// object, which keeps the focus on the grab surface until the client lets
/// go, this is only a preference of the focus logic ([`Lumen::keyboard_target`]):
/// the compositor stays in charge. Super shortcuts and the shell run first,
/// focusing another window explicitly (click, Alt+Tab, the overview, a
/// workspace switch) suspends the grab, clicking the grab window resumes it,
/// and when the grab ends or its window goes away the focus returns to the
/// focused window at once.
pub struct X11KeyboardGrab {
    pub surface: WlSurface,
    /// Destroyed by Xwayland when the X client ungrabs.
    resource: ZwpXwaylandKeyboardGrabV1,
    /// False while the user is elsewhere (see above).
    pub active: bool,
    /// The grab window was seen among the override-redirect windows (a menu
    /// may grab before its surface is associated with it).
    seen: bool,
}

/// Index into [`XwaylandState::wl_selection`].
pub fn selection_index(ty: SelectionTarget) -> usize {
    match ty {
        SelectionTarget::Clipboard => 0,
        SelectionTarget::Primary => 1,
    }
}

impl XwaylandState {
    pub fn new(dh: &DisplayHandle) -> XwaylandState {
        let enabled = !matches!(std::env::var("LUMEN_XWAYLAND").as_deref(), Ok("0" | "off" | "false" | "no"));
        XwaylandState {
            shell: XWaylandShellState::new::<Lumen>(dh),
            _keyboard_grab: XWaylandKeyboardGrabState::new::<Lumen>(dh),
            wm: None,
            display: None,
            ready: false,
            client: None,
            token: None,
            spawned: None,
            ready_after: None,
            pid: None,
            ready_at: None,
            failures: 0,
            enabled,
            error: None,
            wl_selection: [None, None],
            keyboard_grab: None,
        }
    }

    /// `DISPLAY` for X clients started by the compositor: from the spawn on
    /// (clients that connect early wait until the server is up) until
    /// Xwayland fails.
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

/// The command name and the fields after it of /proc/PID/stat ("pid (comm)
/// state ppid ... starttime(22nd field) ..."; comm may contain spaces).
fn proc_stat(pid: i32) -> Option<(String, Vec<String>)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (comm, rest) = stat.split_once(" (").and_then(|(_, r)| r.rsplit_once(") "))?;
    Some((comm.to_string(), rest.split_whitespace().map(String::from).collect()))
}

/// Whether a process exists and has not exited (a zombie has).
fn process_alive(pid: i32) -> bool {
    proc_stat(pid).is_some_and(|(_, f)| !matches!(f.first().map(String::as_str), Some("Z" | "X")))
}

/// The newest live child process of the compositor named Xwayland (smithay
/// keeps the child handle to itself). None until the child has exec'd.
fn xwayland_child_pid() -> Option<i32> {
    let me = std::process::id().to_string();
    let mut best: Option<(u64, i32)> = None;
    for entry in std::fs::read_dir("/proc").ok()?.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse::<i32>().ok()) else { continue };
        let Some((comm, fields)) = proc_stat(pid) else { continue };
        if comm != "Xwayland" || fields.get(1) != Some(&me) || matches!(fields[0].as_str(), "Z" | "X") {
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
        if !x.enabled || x.client.is_some() || x.failures > MAX_RESTARTS {
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
                let spawned = Instant::now();
                x.spawned = Some(spawned);
                let name = format!(":{display}");
                tracing::info!(display = %name, pid, "Xwayland starting");
                // A hung start is given up on; a crash is noticed sooner
                // (check_xwayland_startup runs after every loop iteration).
                let _ = self.lh.insert_source(Timer::from_duration(STARTUP_TIMEOUT), move |_, _, state| {
                    if state.xwayland.spawned == Some(spawned) {
                        state.check_xwayland_startup();
                    }
                    TimeoutAction::Drop
                });
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
        x.ready_at = Some(Instant::now());
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

    /// Xwayland is starting: notice when it dies or hangs before it is ready.
    /// smithay only reports a startup failure when reading its ready pipe
    /// fails, not when the pipe is closed (Xwayland exited): its event source
    /// would then wake the loop forever without an event, so the process is
    /// looked at directly (after every loop iteration, and at the timeout).
    pub fn check_xwayland_startup(&mut self) {
        let x = &mut self.xwayland;
        if x.ready || x.token.is_none() {
            return;
        }
        if x.pid.is_none() {
            x.pid = xwayland_child_pid();
        }
        // Its Wayland connection closes when it exits (a quick exit may be
        // over before its pid was seen).
        let disconnected = x.client.as_ref().is_some_and(|c| c.get_credentials(&self.dh).is_err());
        let dead = disconnected || x.pid.is_some_and(|pid| !process_alive(pid));
        let late = x.spawned.is_some_and(|t| t.elapsed() >= STARTUP_TIMEOUT);
        if dead {
            self.xwayland_lost("Xwayland exited during startup");
        } else if late {
            if let Some(pid) = x.pid {
                // SAFETY: plain syscall; the pid is our own child.
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
            self.xwayland_lost("Xwayland did not start in time");
        }
    }

    /// Xwayland went away (crashed, or failed to start): forget its windows
    /// and selections, and start a new one a moment later, a few times in a
    /// row at most.
    fn xwayland_lost(&mut self, why: &str) {
        tracing::warn!("{why}");
        let ids: Vec<u32> = self.wm.windows.iter().filter(|w| w.x11().is_some()).map(|w| w.id).collect();
        for id in ids {
            self.window_gone(id);
        }
        self.wm.overrides.clear();
        if self.xwayland.keyboard_grab.take().is_some() {
            self.sync_keyboard_focus();
        }
        // Selections X clients owned are gone with them (ours are the only
        // ones with compositor user data).
        if current_data_device_selection_userdata(&self.seat).is_some() {
            clear_data_device_selection(&self.dh, &self.seat);
        }
        if current_primary_selection_userdata(&self.seat).is_some() {
            clear_primary_selection(&self.dh, &self.seat);
        }
        let x = &mut self.xwayland;
        if x.ready_at.is_some_and(|t| t.elapsed() >= STABLE_AFTER) {
            x.failures = 0;
        }
        x.ready = false;
        x.ready_at = None;
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
        if self.xwayland.failures <= MAX_RESTARTS {
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

    /// Whether the keyboard focus is on a surface of Xwayland: an X11
    /// window, or a menu or game holding a keyboard grab.
    pub fn x11_focused(&self) -> bool {
        let Some(client) = self.xwayland.client.as_ref() else { return false };
        let focus = self.seat.get_keyboard().and_then(|k| k.current_focus());
        let surface = focus.and_then(|f| smithay::wayland::seat::WaylandFocus::wl_surface(&f).map(|s| s.into_owned()));
        surface.is_some_and(|s| s.client().is_some_and(|c| c.id() == client.id()))
    }

    /// Where keys go: nowhere while the shell has the keyboard (overview,
    /// switcher, menus), else to an X11 keyboard grab that is in effect, else
    /// to the focused window.
    pub fn keyboard_target(&self) -> Option<FocusTarget> {
        if self.shell.wants_keyboard() {
            return None;
        }
        if let Some(g) = self.xwayland.keyboard_grab.as_ref().filter(|g| g.active) {
            return Some(FocusTarget::Surface(g.surface.clone()));
        }
        self.wm.focused.and_then(|id| self.wm.get(id)).and_then(focus_for)
    }

    /// Give the keyboard to [`Lumen::keyboard_target`] (no events if it has it already).
    pub fn sync_keyboard_focus(&mut self) {
        let target = self.keyboard_target();
        if let Some(kb) = self.seat.get_keyboard() {
            kb.set_focus(self, target, SERIAL_COUNTER.next_serial());
        }
    }

    /// End the X11 keyboard grab once the X client let go (Xwayland destroys
    /// the grab object) or its window is gone. Checked after every batch of
    /// client requests, so the next key already goes to the focused window.
    pub fn check_x11_keyboard_grab(&mut self) {
        let Some(g) = self.xwayland.keyboard_grab.as_mut() else { return };
        let shown = self.wm.override_with(&g.surface).is_some();
        g.seen |= shown;
        if g.resource.is_alive() && g.surface.alive() && (shown || !g.seen) {
            return;
        }
        tracing::debug!("X11 keyboard grab ended");
        self.xwayland.keyboard_grab = None;
        self.sync_keyboard_focus();
    }

    /// The user chose a window explicitly (even the one that has the focus,
    /// e.g. Alt+Tab with a single window): an X11 keyboard grab stops
    /// holding the keys until its window is clicked.
    pub fn suspend_x11_keyboard_grab(&mut self) {
        if let Some(g) = self.xwayland.keyboard_grab.as_mut() {
            g.active = false;
        }
    }

    /// A click on an override-redirect window: if it holds the keyboard
    /// grab, the grab is in effect again.
    pub fn resume_x11_keyboard_grab(&mut self, surface: &WlSurface) {
        if let Some(g) = self.xwayland.keyboard_grab.as_mut().filter(|g| !g.active && &g.surface == surface) {
            g.active = true;
            self.sync_keyboard_focus();
        }
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

    /// The workspace an override-redirect window belongs to: that of the
    /// window it is transient for, else of its X client's focused or topmost
    /// window (an X client's resource ids share their upper bits: X servers
    /// give each client a 21-bit id range), else the current one.
    fn override_workspace(&self, window: &X11Surface) -> usize {
        let client = |id: u32| id >> 21;
        let x11 = |w: &&crate::wm::Win| w.mapped && w.x11().is_some();
        let parent = window.is_transient_for().and_then(|p| self.wm.windows.iter().filter(x11).find(|w| w.x11().is_some_and(|x| x.window_id() == p)));
        let same_client = |w: &&crate::wm::Win| w.x11().is_some_and(|x| client(x.window_id()) == client(window.window_id()));
        let focused = self.wm.focused.and_then(|id| self.wm.get(id)).filter(x11).filter(same_client);
        let any = || self.wm.windows.iter().rev().filter(x11).find(same_client);
        parent.or(focused).or_else(any).map(|w| w.ws).unwrap_or(self.wm.current)
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

    /// Xwayland may commit a window's first buffer before the X11 window
    /// names its surface (the WL_SURFACE_SERIAL message arrives later): that
    /// commit found no window to map. A window that draws only once (message
    /// boxes, dialogs waiting for input) is mapped here instead.
    fn surface_associated(&mut self, _xwm: XwmId, _surface: WlSurface, window: X11Surface) {
        if let Some(id) = self.wm.find_x11(&window) {
            if let Some(w) = self.wm.get(id) {
                w.window.on_commit();
            }
            self.on_x11_commit(id);
        } else {
            // A menu or tooltip got its surface: it may be under the pointer.
            self.refresh_pointer_focus();
        }
        self.queue_redraw_all();
    }
}

/// An X client grabbed the keyboard (XGrabKeyboard) on a window whose
/// surface does not have the keyboard focus, typically a game's fullscreen
/// override-redirect window or a menu: Xwayland asks for the keys to go to
/// that surface. See [`X11KeyboardGrab`] for how the compositor keeps control.
impl XWaylandKeyboardGrabHandler for Lumen {
    /// Granted to override-redirect windows only, and only while an X11 window
    /// (or nothing) has the focus or when the window covers the output (an
    /// unmanaged fullscreen game, which is drawn above everything): an X
    /// client in the background cannot take the keys from a Wayland app.
    /// Grabs on managed windows need nothing (or would steal the focus).
    /// The focus moves now, not with the next key: that key press would
    /// otherwise arrive as already held in the enter event, and X clients
    /// would only see its release.
    fn grab(&mut self, surface: WlSurface, _seat: Seat<Self>, grab: XWaylandKeyboardGrab<Self>) {
        if self.wm.find_toplevel(&surface).is_some() {
            tracing::debug!("X11 keyboard grab on a managed window: nothing to do");
            return;
        }
        // Not managed: an override-redirect window, possibly one whose
        // surface is not associated yet (a menu grabbing right after mapping).
        let or = self.wm.override_with(&surface);
        let covers = or.is_some_and(|o| o.x.geometry().to_f64().contains_rect(self.output_rect()));
        let x11_or_nothing = match self.wm.focused.and_then(|id| self.wm.get(id)) {
            Some(w) => w.x11().is_some() || self.x11_focused(),
            None => true,
        };
        if !covers && !x11_or_nothing {
            tracing::debug!("X11 keyboard grab denied: a Wayland window has the focus");
            return;
        }
        tracing::debug!(covers, "X11 keyboard grab");
        let seen = or.is_some();
        self.xwayland.keyboard_grab = Some(X11KeyboardGrab { surface, resource: grab.grab().clone(), active: true, seen });
        self.sync_keyboard_focus();
    }

    /// Any surface of Xwayland qualifies here (a menu may grab before its
    /// surface is associated with its X11 window); [`Self::grab`] decides.
    fn keyboard_focus_for_xsurface(&self, surface: &WlSurface) -> Option<FocusTarget> {
        if let Some(w) = self.wm.windows.iter().find(|w| w.x11().is_some() && w.surface().as_ref() == Some(surface)) {
            return focus_for(w);
        }
        Some(FocusTarget::Surface(surface.clone()))
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
        self.wm.overrides.retain(|o| !same_x11(&o.x, &window));
        let ws = self.override_workspace(&window);
        self.wm.overrides.push(Override { x: window, ws });
        self.refresh_pointer_focus();
        self.queue_redraw_all();
    }

    fn unmapped_window(&mut self, _xwm: XwmId, window: X11Surface) {
        if window.is_override_redirect() {
            self.wm.overrides.retain(|o| !same_x11(&o.x, &window));
            self.check_x11_keyboard_grab();
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
        self.wm.overrides.retain(|o| !same_x11(&o.x, &window));
        if let Some(id) = self.x11_id(&window) {
            self.window_gone(id);
        } else if before != self.wm.overrides.len() {
            self.check_x11_keyboard_grab();
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
