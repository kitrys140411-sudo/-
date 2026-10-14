//! `--x11`: the test client as an X11 application (run under Xwayland), for
//! the compositor's X11 window-management tests.
//!
//! Output lines, with the Wayland client's vocabulary where it applies:
//!   ready                                     mapped by the window manager and exposed
//!   configure x=.. y=.. w=.. h=.. synthetic=0|1   synthetic: root coordinates from the WM
//!   focus in|out surface=main|dialog
//!   key code=.. state=pressed|released        X keycode (evdev code + 8)
//!   button code=.. state=pressed|released x=.. y=.. root_x=.. root_y=..
//!   enter kind=pointer x=.. y=.. surface=main|dialog|popup
//!   popup event=open x=.. y=.. root_x=.. root_y=..   an override-redirect menu at the click
//!   popup event=closed reason=click
//!   dialog event=mapped                       --dialog: the transient dialog is shown
//!   grab status=success|...                   --grab-pointer: the pointer grab after the first click
//!   look dx=.. dy=..                          --grab-pointer: motion away from the center (then warped back)
//!   titlebar event=move                       --titlebar: a press on the title strip asked the WM to move
//!   grab keyboard status=success|...          --override-fullscreen: XGrabKeyboard once shown
//!   closed                                    WM_DELETE_WINDOW from the window manager
//!   exit reason=timeout|key

use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    AtomEnum, ChangeWindowAttributesAux, ClientMessageEvent, ConnectionExt as _, CreateGCAux, CreateWindowAux, EventMask, GrabMode,
    GrabStatus, NotifyDetail, PropMode, Rectangle, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use crate::args::{InitialState, Options};
use crate::out::{emit, escape};

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        WM_PROTOCOLS,
        WM_DELETE_WINDOW,
        _NET_WM_NAME,
        UTF8_STRING,
        _NET_WM_STATE,
        _NET_WM_STATE_FULLSCREEN,
        _NET_WM_STATE_MAXIMIZED_HORZ,
        _NET_WM_STATE_MAXIMIZED_VERT,
        _NET_WM_WINDOW_TYPE,
        _NET_WM_WINDOW_TYPE_DIALOG,
        _NET_WM_WINDOW_TYPE_POPUP_MENU,
        _NET_WM_PID,
        _NET_WM_MOVERESIZE,
    }
}

const POPUP_SIZE: (u16, u16) = (200, 120);
const DIALOG_SIZE: (u16, u16) = (320, 200);
/// X keycode of Escape (evdev KEY_ESC + 8).
const KEYCODE_ESCAPE: u8 = 9;

fn grab_status_name(status: GrabStatus) -> &'static str {
    match status {
        GrabStatus::SUCCESS => "success",
        GrabStatus::ALREADY_GRABBED => "already-grabbed",
        GrabStatus::INVALID_TIME => "invalid-time",
        GrabStatus::NOT_VIEWABLE => "not-viewable",
        GrabStatus::FROZEN => "frozen",
        _ => "unknown",
    }
}

fn pixel((r, g, b): (u8, u8, u8)) -> u32 {
    (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
}

fn darker((r, g, b): (u8, u8, u8)) -> (u8, u8, u8) {
    (r / 2, g / 2, b / 2)
}

struct Client {
    conn: RustConnection,
    root: Window,
    atoms: Atoms,
    main: Window,
    dialog: Option<Window>,
    popup: Option<Window>,
    /// Fills the title strip at the top of the windows.
    gc: u32,
    size: (u16, u16),
    ready: bool,
    dialog_shown: bool,
    /// --grab-pointer: the pointer is grabbed (mouse look).
    grabbed: bool,
}

impl Client {
    fn surface_name(&self, w: Window) -> &'static str {
        if w == self.main {
            "main"
        } else if Some(w) == self.dialog {
            "dialog"
        } else if Some(w) == self.popup {
            "popup"
        } else {
            "other"
        }
    }

    /// A top-level window with a title, WM_CLASS and WM_DELETE_WINDOW.
    fn create_toplevel(&self, title: &str, app_id: &str, size: (u16, u16), color: (u8, u8, u8), motion: bool) -> Result<Window, String> {
        let c = &self.conn;
        let w = c.generate_id().map_err(|e| e.to_string())?;
        let motion = if motion { EventMask::POINTER_MOTION } else { EventMask::NO_EVENT };
        let mask = motion
            | EventMask::EXPOSURE
            | EventMask::STRUCTURE_NOTIFY
            | EventMask::FOCUS_CHANGE
            | EventMask::KEY_PRESS
            | EventMask::KEY_RELEASE
            | EventMask::BUTTON_PRESS
            | EventMask::BUTTON_RELEASE
            | EventMask::ENTER_WINDOW;
        let aux = CreateWindowAux::new().background_pixel(pixel(color)).event_mask(mask);
        c.create_window(x11rb::COPY_DEPTH_FROM_PARENT, w, self.root, 0, 0, size.0, size.1, 0, WindowClass::INPUT_OUTPUT, x11rb::COPY_FROM_PARENT, &aux)
            .map_err(|e| e.to_string())?;
        let a = &self.atoms;
        let err = |e: x11rb::errors::ConnectionError| e.to_string();
        c.change_property8(PropMode::REPLACE, w, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes()).map_err(err)?;
        c.change_property8(PropMode::REPLACE, w, a._NET_WM_NAME, a.UTF8_STRING, title.as_bytes()).map_err(err)?;
        // WM_CLASS: instance and class, NUL-terminated.
        let class = format!("{}\0{}\0", app_id.to_lowercase(), app_id);
        c.change_property8(PropMode::REPLACE, w, AtomEnum::WM_CLASS, AtomEnum::STRING, class.as_bytes()).map_err(err)?;
        c.change_property32(PropMode::REPLACE, w, a.WM_PROTOCOLS, AtomEnum::ATOM, &[a.WM_DELETE_WINDOW]).map_err(err)?;
        c.change_property32(PropMode::REPLACE, w, a._NET_WM_PID, AtomEnum::CARDINAL, &[std::process::id()]).map_err(err)?;
        Ok(w)
    }

    /// Ask the window manager to add EWMH states (a client message to the root).
    fn request_states(&self, states: &[u32]) -> Result<(), String> {
        let a = &self.atoms;
        let data = [1, states.first().copied().unwrap_or(0), states.get(1).copied().unwrap_or(0), 1, 0];
        let ev = ClientMessageEvent::new(32, self.main, a._NET_WM_STATE, data);
        self.conn
            .send_event(false, self.root, EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY, ev)
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn paint_strip(&self, w: Window, width: u16) {
        let r = Rectangle { x: 0, y: 0, width, height: 32 };
        let _ = self.conn.poly_fill_rectangle(w, self.gc, &[r]);
    }

    /// Open an override-redirect menu at window coordinates (x, y): it is
    /// placed in root coordinates computed by the X server from where it
    /// believes the window is.
    fn open_popup(&mut self, x: i16, y: i16, color: (u8, u8, u8)) -> Result<(), String> {
        let c = &self.conn;
        let t = c.translate_coordinates(self.main, self.root, x, y).map_err(|e| e.to_string())?.reply().map_err(|e| e.to_string())?;
        let w = c.generate_id().map_err(|e| e.to_string())?;
        let aux = CreateWindowAux::new()
            .override_redirect(1)
            .background_pixel(pixel(color))
            .event_mask(EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::ENTER_WINDOW);
        c.create_window(
            x11rb::COPY_DEPTH_FROM_PARENT,
            w,
            self.root,
            t.dst_x,
            t.dst_y,
            POPUP_SIZE.0,
            POPUP_SIZE.1,
            0,
            WindowClass::INPUT_OUTPUT,
            x11rb::COPY_FROM_PARENT,
            &aux,
        )
        .map_err(|e| e.to_string())?;
        let a = &self.atoms;
        c.change_property32(PropMode::REPLACE, w, a._NET_WM_WINDOW_TYPE, AtomEnum::ATOM, &[a._NET_WM_WINDOW_TYPE_POPUP_MENU])
            .map_err(|e| e.to_string())?;
        c.map_window(w).map_err(|e| e.to_string())?;
        self.popup = Some(w);
        emit(&format!("popup event=open x={x} y={y} root_x={} root_y={}", t.dst_x, t.dst_y));
        Ok(())
    }

    /// Client-side title bar (Steam, Chromium, many games' launchers): a
    /// press on it lets the window manager move the window. The implicit
    /// grab of the press is released first, as EWMH asks.
    fn titlebar_move(&self, root_x: i16, root_y: i16, button: u8) -> Result<(), String> {
        let c = &self.conn;
        c.ungrab_pointer(x11rb::CURRENT_TIME).map_err(|e| e.to_string())?;
        // _NET_WM_MOVERESIZE_MOVE = 8; source indication 1 = application.
        let data = [root_x as u32, root_y as u32, 8, u32::from(button), 1];
        let ev = ClientMessageEvent::new(32, self.main, self.atoms._NET_WM_MOVERESIZE, data);
        c.send_event(false, self.root, EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY, ev)
            .map_err(|e| e.to_string())?;
        emit("titlebar event=move");
        Ok(())
    }

    /// The center of the main window, where mouse look keeps the pointer.
    fn center(&self) -> (i16, i16) {
        ((self.size.0 / 2) as i16, (self.size.1 / 2) as i16)
    }

    /// Mouse look as SDL (relative mouse mode without XInput2) and Wine do
    /// it: an active grab confined to the window, an invisible cursor and the
    /// pointer warped to the center. Xwayland turns this into a pointer lock
    /// with relative motion on the Wayland side.
    fn grab_pointer(&mut self) -> Result<(), String> {
        let c = &self.conn;
        let err = |e: x11rb::errors::ConnectionError| e.to_string();
        let pixmap = c.generate_id().map_err(|e| e.to_string())?;
        c.create_pixmap(1, pixmap, self.main, 1, 1).map_err(err)?;
        let cursor = c.generate_id().map_err(|e| e.to_string())?;
        c.create_cursor(cursor, pixmap, pixmap, 0, 0, 0, 0, 0, 0, 0, 0).map_err(err)?;
        c.free_pixmap(pixmap).map_err(err)?;
        let mask = EventMask::POINTER_MOTION | EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE;
        let reply = c
            .grab_pointer(false, self.main, mask, GrabMode::ASYNC, GrabMode::ASYNC, self.main, cursor, x11rb::CURRENT_TIME)
            .map_err(err)?
            .reply()
            .map_err(|e| e.to_string())?;
        emit(&format!("grab status={}", grab_status_name(reply.status)));
        if reply.status == GrabStatus::SUCCESS {
            self.grabbed = true;
            let (cx, cy) = self.center();
            c.warp_pointer(x11rb::NONE, self.main, 0, 0, 0, 0, cx, cy).map_err(err)?;
        }
        Ok(())
    }

    fn close_popup(&mut self) {
        if let Some(w) = self.popup.take() {
            let _ = self.conn.destroy_window(w);
            emit("popup event=closed reason=click");
        }
    }

    fn open_dialog(&mut self, opts: &Options) -> Result<(), String> {
        let color = ((opts.color.0 / 2).saturating_add(100), opts.color.1, opts.color.2);
        let w = self.create_toplevel(&format!("{} dialog", opts.title), &opts.app_id, DIALOG_SIZE, color, false)?;
        let a = &self.atoms;
        let c = &self.conn;
        c.change_property32(PropMode::REPLACE, w, AtomEnum::WM_TRANSIENT_FOR, AtomEnum::WINDOW, &[self.main]).map_err(|e| e.to_string())?;
        c.change_property32(PropMode::REPLACE, w, a._NET_WM_WINDOW_TYPE, AtomEnum::ATOM, &[a._NET_WM_WINDOW_TYPE_DIALOG])
            .map_err(|e| e.to_string())?;
        c.map_window(w).map_err(|e| e.to_string())?;
        self.dialog = Some(w);
        Ok(())
    }
}

pub fn run(opts: &Options) -> Result<u8, String> {
    let (conn, screen_num) = x11rb::connect(None).map_err(|e| format!("cannot connect to the X server: {e}"))?;
    let screen = conn.setup().roots[screen_num].clone();
    let atoms = Atoms::new(&conn).map_err(|e| e.to_string())?.reply().map_err(|e| e.to_string())?;
    let gc = conn.generate_id().map_err(|e| e.to_string())?;
    conn.create_gc(gc, screen.root, &CreateGCAux::new().foreground(pixel(darker(opts.color)))).map_err(|e| e.to_string())?;
    let size = if opts.override_fullscreen {
        (screen.width_in_pixels, screen.height_in_pixels)
    } else {
        (opts.size.0.min(u16::MAX as u32) as u16, opts.size.1.min(u16::MAX as u32) as u16)
    };
    let mut c = Client {
        conn,
        root: screen.root,
        atoms,
        main: 0,
        dialog: None,
        popup: None,
        gc,
        size,
        ready: false,
        dialog_shown: false,
        grabbed: false,
    };
    c.main = c.create_toplevel(&opts.title, &opts.app_id, size, opts.color, opts.grab_pointer)?;
    if opts.initial_state == Some(InitialState::Fullscreen) {
        // EWMH: a window may ask for states before it is mapped.
        let a = &c.atoms;
        c.conn
            .change_property32(PropMode::REPLACE, c.main, a._NET_WM_STATE, AtomEnum::ATOM, &[a._NET_WM_STATE_FULLSCREEN])
            .map_err(|e| e.to_string())?;
    }
    if opts.override_fullscreen {
        // Unmanaged and covering the screen, as SDL 1.2 and some Wine games
        // do fullscreen: the window manager never sees the window.
        c.conn
            .change_window_attributes(c.main, &ChangeWindowAttributesAux::new().override_redirect(1))
            .map_err(|e| e.to_string())?;
    }
    c.conn.map_window(c.main).map_err(|e| e.to_string())?;

    let deadline = opts.exit_after_ms.map(|ms| Instant::now() + Duration::from_millis(ms));
    let mut mapped_once = false;
    loop {
        c.conn.flush().map_err(|e| format!("X connection: {e}"))?;
        let event = match c.conn.poll_for_event().map_err(|e| format!("X connection lost: {e}"))? {
            Some(ev) => ev,
            None => {
                if deadline.is_some_and(|d| Instant::now() >= d) {
                    emit("exit reason=timeout");
                    return Ok(0);
                }
                std::thread::sleep(Duration::from_millis(4));
                continue;
            }
        };
        match event {
            Event::MapNotify(e) if e.window == c.main && !mapped_once => {
                mapped_once = true;
                let a = &c.atoms;
                match opts.initial_state {
                    Some(InitialState::Fullscreen) => c.request_states(&[a._NET_WM_STATE_FULLSCREEN])?,
                    Some(InitialState::Maximized) => c.request_states(&[a._NET_WM_STATE_MAXIMIZED_HORZ, a._NET_WM_STATE_MAXIMIZED_VERT])?,
                    None => {}
                }
            }
            Event::Expose(e) if e.count == 0 => {
                if e.window == c.main {
                    c.paint_strip(c.main, c.size.0);
                    if !c.ready {
                        c.ready = true;
                        emit("ready");
                        if opts.dialog {
                            c.open_dialog(opts)?;
                        }
                        if opts.override_fullscreen {
                            let reply = c
                                .conn
                                .grab_keyboard(false, c.main, x11rb::CURRENT_TIME, GrabMode::ASYNC, GrabMode::ASYNC)
                                .map_err(|e| e.to_string())?
                                .reply()
                                .map_err(|e| e.to_string())?;
                            emit(&format!("grab keyboard status={}", grab_status_name(reply.status)));
                        }
                    }
                } else if Some(e.window) == c.dialog {
                    c.paint_strip(e.window, DIALOG_SIZE.0);
                    if !c.dialog_shown {
                        c.dialog_shown = true;
                        emit("dialog event=mapped");
                    }
                }
            }
            Event::ConfigureNotify(e) if e.window == c.main => {
                let synthetic = e.response_type & 0x80 != 0;
                if !synthetic {
                    c.size = (e.width, e.height);
                }
                emit(&format!("configure x={} y={} w={} h={} synthetic={}", e.x, e.y, e.width, e.height, u8::from(synthetic)));
            }
            Event::FocusIn(e) if e.detail != NotifyDetail::POINTER => {
                emit(&format!("focus in surface={}", c.surface_name(e.event)));
            }
            Event::FocusOut(e) if e.detail != NotifyDetail::POINTER => {
                emit(&format!("focus out surface={}", c.surface_name(e.event)));
            }
            Event::KeyPress(e) => {
                emit(&format!("key code={} state=pressed", e.detail));
                if opts.close_on_key && e.detail == KEYCODE_ESCAPE {
                    emit("exit reason=key");
                    return Ok(0);
                }
            }
            Event::KeyRelease(e) => emit(&format!("key code={} state=released", e.detail)),
            Event::ButtonPress(e) => {
                emit(&format!(
                    "button code={} state=pressed x={} y={} root_x={} root_y={} surface={}",
                    e.detail,
                    e.event_x,
                    e.event_y,
                    e.root_x,
                    e.root_y,
                    c.surface_name(e.event)
                ));
                if opts.grab_pointer && !c.grabbed {
                    c.grab_pointer()?;
                }
                if opts.titlebar && e.event == c.main && e.detail == 1 && e.event_y < 32 {
                    c.titlebar_move(e.root_x, e.root_y, e.detail)?;
                }
                if opts.popup {
                    if c.popup.is_some() {
                        c.close_popup();
                    } else if e.detail == 3 && e.event == c.main {
                        let color = (0xe0, 0xaf, 0x68);
                        c.open_popup(e.event_x, e.event_y, color)?;
                    }
                }
            }
            Event::ButtonRelease(e) => emit(&format!(
                "button code={} state=released x={} y={} surface={}",
                e.detail,
                e.event_x,
                e.event_y,
                c.surface_name(e.event)
            )),
            Event::MotionNotify(e) if c.grabbed => {
                let (cx, cy) = c.center();
                let (dx, dy) = (e.event_x - cx, e.event_y - cy);
                if (dx, dy) != (0, 0) {
                    emit(&format!("look dx={dx} dy={dy}"));
                    let _ = c.conn.warp_pointer(x11rb::NONE, c.main, 0, 0, 0, 0, cx, cy);
                }
            }
            Event::EnterNotify(e) => {
                emit(&format!("enter kind=pointer x={} y={} surface={}", e.event_x, e.event_y, c.surface_name(e.event)))
            }
            Event::ClientMessage(e) if e.type_ == c.atoms.WM_PROTOCOLS && e.data.as_data32()[0] == c.atoms.WM_DELETE_WINDOW => {
                if Some(e.window) == c.dialog {
                    let _ = c.conn.destroy_window(e.window);
                    c.dialog = None;
                    emit("dialog event=closed");
                    continue;
                }
                emit("closed");
                let _ = c.conn.destroy_window(c.main);
                let _ = c.conn.flush();
                return Ok(0);
            }
            Event::Error(e) => eprintln!("lumen-testclient: X error: {}", escape(&format!("{e:?}"))),
            _ => {}
        }
    }
}
