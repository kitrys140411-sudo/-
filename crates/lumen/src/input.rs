//! Input handling: keyboard shortcuts, pointer focus, interactive move/resize.

use smithay::backend::input::{
    AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent, KeyState, KeyboardKeyEvent,
    PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
};
use smithay::input::keyboard::{keysyms, FilterResult, Keysym, ModifiersState};
use smithay::input::pointer::{AxisFrame, ButtonEvent, MotionEvent, RelativeMotionEvent};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Serial, SERIAL_COUNTER};

use crate::state::Lumen;
use crate::wm::{Grab, GrabKind, Layout, Tile, WORKSPACES};

pub const BTN_LEFT: u32 = 0x110;
pub const BTN_RIGHT: u32 = 0x111;
pub const BTN_MIDDLE: u32 = 0x112;

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    None,
    Quit,
    Spawn(Vec<String>),
    CloseWindow,
    Maximize,
    Restore,
    Tile(Tile),
    Workspace(usize),
    WorkspaceRelative(i32),
    MoveToWorkspaceRelative(i32),
    ToggleOverview,
    ShowApps,
    SwitcherNext { reverse: bool },
    VtSwitch(i32),
    Screenshot,
    ToggleHud,
    /// A key for the shell UI (overview search, switcher, menus).
    ShellKey { sym: Keysym, utf8: Option<String>, mods: ModifiersState },
}

/// Map a key press (with modifiers) to a compositor action.
fn shortcut(mods: &ModifiersState, sym: Keysym) -> Option<Action> {
    let raw = sym.raw();
    let logo = mods.logo;
    if mods.ctrl && mods.alt && (keysyms::KEY_XF86Switch_VT_1..=keysyms::KEY_XF86Switch_VT_12).contains(&raw) {
        return Some(Action::VtSwitch((raw - keysyms::KEY_XF86Switch_VT_1 + 1) as i32));
    }
    if mods.ctrl && mods.alt && raw == keysyms::KEY_BackSpace {
        return Some(Action::Quit);
    }
    if mods.alt && !mods.ctrl && (raw == keysyms::KEY_Tab || raw == keysyms::KEY_ISO_Left_Tab) {
        return Some(Action::SwitcherNext { reverse: mods.shift || raw == keysyms::KEY_ISO_Left_Tab });
    }
    if mods.alt && raw == keysyms::KEY_F4 {
        return Some(Action::CloseWindow);
    }
    if raw == keysyms::KEY_Print {
        return Some(Action::Screenshot);
    }
    if mods.ctrl && mods.alt && raw == keysyms::KEY_Left {
        return Some(Action::WorkspaceRelative(-1));
    }
    if mods.ctrl && mods.alt && raw == keysyms::KEY_Right {
        return Some(Action::WorkspaceRelative(1));
    }
    if !logo {
        return None;
    }
    Some(match raw {
        keysyms::KEY_Return | keysyms::KEY_t => Action::Spawn(vec!["lumen-term".into()]),
        keysyms::KEY_q => Action::CloseWindow,
        keysyms::KEY_Up => Action::Maximize,
        keysyms::KEY_Down => Action::Restore,
        keysyms::KEY_Left => Action::Tile(Tile::Left),
        keysyms::KEY_Right => Action::Tile(Tile::Right),
        keysyms::KEY_Page_Up if mods.shift => Action::MoveToWorkspaceRelative(-1),
        keysyms::KEY_Page_Down if mods.shift => Action::MoveToWorkspaceRelative(1),
        keysyms::KEY_Page_Up => Action::WorkspaceRelative(-1),
        keysyms::KEY_Page_Down => Action::WorkspaceRelative(1),
        keysyms::KEY_1..=keysyms::KEY_4 => Action::Workspace((raw - keysyms::KEY_1) as usize),
        keysyms::KEY_a => Action::ShowApps,
        keysyms::KEY_s => Action::ToggleOverview,
        keysyms::KEY_h => Action::ToggleHud,
        _ => return None,
    })
}

fn is_super(sym: Keysym) -> bool {
    matches!(sym.raw(), keysyms::KEY_Super_L | keysyms::KEY_Super_R)
}

impl Lumen {
    pub fn process_input_event<B: InputBackend>(&mut self, event: InputEvent<B>) {
        match event {
            InputEvent::Keyboard { event } => self.on_key::<B>(event),
            InputEvent::PointerMotion { event } => {
                let delta = event.delta();
                let pos = self.pointer_pos + delta;
                self.on_pointer_moved(pos, event.time_msec(), Some((delta, event.delta_unaccel(), event.time())));
            }
            InputEvent::PointerMotionAbsolute { event } => {
                let out = self.output_rect();
                let pos = Point::from((out.loc.x + event.x_transformed(out.size.w as i32), out.loc.y + event.y_transformed(out.size.h as i32)));
                self.on_pointer_moved(pos, event.time_msec(), None);
            }
            InputEvent::PointerButton { event } => self.handle_button(event.button_code(), event.state(), event.time_msec()),
            InputEvent::PointerAxis { event } => self.on_axis::<B>(event),
            _ => {}
        }
    }

    fn on_key<B: InputBackend>(&mut self, event: B::KeyboardKeyEvent) {
        let time = Event::time_msec(&event);
        self.handle_key(event.key_code(), event.state(), time);
    }

    /// Process a key from any source (input backend or the IPC socket).
    pub fn handle_key(&mut self, keycode: smithay::input::keyboard::Keycode, state: KeyState, time: u32) {
        let serial = SERIAL_COUNTER.next_serial();
        let Some(keyboard) = self.seat.get_keyboard() else { return };
        let shell_grabs = self.shell.wants_keyboard();
        let action = keyboard.input::<Action, _>(self, keycode, state, serial, time, |lumen, mods, handle| {
            let sym = handle.modified_sym();
            let raw_sym = handle.raw_syms().first().copied().unwrap_or(sym);
            if state == KeyState::Pressed {
                if is_super(raw_sym) {
                    lumen.shell.super_tap = true;
                } else {
                    lumen.shell.super_tap = false;
                }
                if let Some(a) = shortcut(mods, raw_sym).or_else(|| shortcut(mods, sym)) {
                    lumen.suppressed_keys.push(raw_sym);
                    return FilterResult::Intercept(a);
                }
                if shell_grabs {
                    lumen.suppressed_keys.push(raw_sym);
                    return FilterResult::Intercept(Action::ShellKey { sym, utf8: handle.modified_sym().key_char().map(String::from), mods: *mods });
                }
                FilterResult::Forward
            } else {
                // Releasing Super after tapping it alone toggles the overview.
                if is_super(raw_sym) && std::mem::take(&mut lumen.shell.super_tap) {
                    return FilterResult::Intercept(Action::ToggleOverview);
                }
                // Releasing Alt ends the window switcher.
                if matches!(raw_sym.raw(), keysyms::KEY_Alt_L | keysyms::KEY_Alt_R) && lumen.shell.switcher_active() {
                    lumen.shell_switcher_commit();
                }
                if let Some(i) = lumen.suppressed_keys.iter().position(|k| *k == raw_sym) {
                    lumen.suppressed_keys.remove(i);
                    return FilterResult::Intercept(Action::None);
                }
                FilterResult::Forward
            }
        });
        if let Some(a) = action {
            self.run_action(a);
        }
    }

    pub fn run_action(&mut self, action: Action) {
        let (work, out) = (self.work_area(), self.output_rect());
        let focused = self.wm.focused;
        match action {
            Action::None => {}
            Action::Quit => {
                tracing::info!("quit requested");
                self.signal.stop();
            }
            Action::Spawn(cmd) => self.spawn(&cmd),
            Action::CloseWindow => {
                if let Some(id) = focused {
                    self.close_window(id);
                }
            }
            Action::Maximize => {
                if let Some(id) = focused {
                    self.wm.set_layout(id, Layout::Maximized { restore: work }, work, out);
                }
            }
            Action::Restore => {
                if let Some(id) = focused {
                    self.wm.set_layout(id, Layout::Floating, work, out);
                }
            }
            Action::Tile(side) => {
                if let Some(id) = focused {
                    let layout = match self.wm.get(id).map(|w| w.layout) {
                        // Tiling to the opposite side of a tiled window restores it.
                        Some(Layout::Tiled { side: s, .. }) if s != side => Layout::Floating,
                        _ => Layout::Tiled { side, restore: work },
                    };
                    self.wm.set_layout(id, layout, work, out);
                }
            }
            Action::Workspace(n) => self.switch_workspace(n),
            Action::WorkspaceRelative(d) => {
                let n = (self.wm.current as i32 + d).clamp(0, WORKSPACES as i32 - 1) as usize;
                self.switch_workspace(n);
            }
            Action::MoveToWorkspaceRelative(d) => {
                if let Some(id) = focused {
                    let n = (self.wm.current as i32 + d).clamp(0, WORKSPACES as i32 - 1) as usize;
                    if let Some(w) = self.wm.get_mut(id) {
                        w.ws = n;
                    }
                    self.switch_workspace(n);
                    self.focus_window(Some(id));
                }
            }
            Action::ToggleOverview => self.shell_toggle_overview(false),
            Action::ShowApps => self.shell_toggle_overview(true),
            Action::SwitcherNext { reverse } => self.shell_switcher_next(reverse),
            Action::VtSwitch(vt) => self.backend.change_vt(vt),
            Action::Screenshot => self.screenshot_to_file(),
            Action::ToggleHud => self.shell.hud = !self.shell.hud,
            Action::ShellKey { sym, utf8, mods } => self.shell_key(sym, utf8, mods),
        }
        self.queue_redraw_all();
    }

    pub fn switch_workspace(&mut self, n: usize) {
        if self.wm.switch_workspace(n) {
            let next = self
                .wm
                .mru
                .iter()
                .copied()
                .find(|id| self.wm.get(*id).is_some_and(|w| w.mapped && w.ws == n));
            self.focus_window(next);
        }
    }

    pub fn spawn(&mut self, cmd: &[String]) {
        let Some((prog, args)) = cmd.split_first() else { return };
        let mut c = std::process::Command::new(prog);
        c.args(args)
            .env("WAYLAND_DISPLAY", &self.socket_name)
            .env("XDG_SESSION_TYPE", "wayland")
            .env("LUMEN_IPC", crate::ipc::socket_path())
            .env_remove("DISPLAY");
        c.stdin(std::process::Stdio::null());
        // Children must not inherit the compositor's signal mask/dispositions.
        use std::os::unix::process::CommandExt;
        unsafe {
            c.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        match c.spawn() {
            Ok(child) => {
                tracing::info!("spawned {prog} (pid {})", child.id());
                // Reap it in the background so it never becomes a zombie.
                std::thread::spawn(move || {
                    let mut child = child;
                    let _ = child.wait();
                });
            }
            Err(e) => {
                tracing::warn!("cannot spawn {prog}: {e}");
                self.shell.notify(&self.theme, "Cannot start application", &format!("{prog}: {e}"), self.now());
            }
        }
    }

    fn clamp_pointer(&self, p: Point<f64, Logical>) -> Point<f64, Logical> {
        let out = self.output_rect();
        let x = p.x.clamp(out.loc.x, out.loc.x + out.size.w - 1.0);
        let y = p.y.clamp(out.loc.y, out.loc.y + out.size.h - 1.0);
        (x, y).into()
    }

    pub fn on_pointer_moved(
        &mut self,
        pos: Point<f64, Logical>,
        time: u32,
        relative: Option<(Point<f64, Logical>, Point<f64, Logical>, u64)>,
    ) {
        let pos = self.clamp_pointer(pos);
        let prev = self.pointer_pos;
        self.pointer_pos = pos;
        let serial = SERIAL_COUNTER.next_serial();
        let Some(pointer) = self.seat.get_pointer() else { return };

        if self.update_grab(pos) {
            pointer.motion(self, None, &MotionEvent { location: pos, serial, time });
            pointer.frame(self);
            self.queue_redraw_all();
            return;
        }

        // Hot corner: entering the top-left corner opens the overview.
        if self.theme.geometry.hot_corner && pos.x <= 0.5 && pos.y <= 0.5 && (prev.x > 0.5 || prev.y > 0.5) {
            self.shell_toggle_overview(false);
        }

        let under = if self.shell_pointer_motion(pos) {
            None
        } else {
            self.wm.surface_under(pos).map(|(_, s, loc)| (s, loc))
        };
        self.note_pointer_target(&under);
        pointer.motion(self, under.clone(), &MotionEvent { location: pos, serial, time });
        if let Some((delta, delta_unaccel, utime)) = relative {
            pointer.relative_motion(self, under, &RelativeMotionEvent { delta, delta_unaccel, utime });
        }
        pointer.frame(self);
        self.queue_redraw_all();
    }

    /// Re-evaluate what is under the unmoved pointer (windows moved, mapped,
    /// closed or restacked, a workspace switched), so enter/leave and the
    /// cursor follow without waiting for the next motion.
    pub fn refresh_pointer_focus(&mut self) {
        let Some(pointer) = self.seat.get_pointer() else { return };
        if pointer.is_grabbed() || self.wm.grab.is_some() {
            return;
        }
        let pos = self.pointer_pos;
        let under = if self.shell_pointer_motion(pos) { None } else { self.wm.surface_under(pos).map(|(_, s, loc)| (s, loc)) };
        if under == self.pointer_target {
            return;
        }
        self.note_pointer_target(&under);
        let serial = SERIAL_COUNTER.next_serial();
        let time = self.now().as_millis() as u32;
        pointer.motion(self, under, &MotionEvent { location: pos, serial, time });
        pointer.frame(self);
    }

    /// Track the pointer's target; leaving client surfaces for the shell or
    /// the desktop restores the default cursor (clients only set theirs on enter).
    fn note_pointer_target(&mut self, under: &Option<(WlSurface, Point<f64, Logical>)>) {
        let left_client = self.pointer_target.is_some() && under.is_none();
        self.pointer_target = under.clone();
        if left_client && self.wm.grab.is_none() {
            self.cursor_status = smithay::input::pointer::CursorImageStatus::default_named();
        }
    }

    pub fn handle_button(&mut self, button: u32, state: ButtonState, time: u32) {
        let serial = SERIAL_COUNTER.next_serial();
        self.refresh_pointer_focus();
        let Some(pointer) = self.seat.get_pointer() else { return };
        let pressed = state == ButtonState::Pressed;

        if !pressed {
            if self.wm.grab.as_ref().is_some_and(|g| g.button == button) {
                self.end_grab();
                return;
            }
        }
        if pressed && !pointer.is_grabbed() {
            // Shell UI first (panel, overview, dock, menus).
            if self.shell_click(button, self.pointer_pos) {
                self.queue_redraw_all();
                return;
            }
            let logo = self.seat.get_keyboard().is_some_and(|k| k.modifier_state().logo);
            let under = self.wm.window_under(self.pointer_pos);
            if let Some(id) = under {
                self.focus_window(Some(id));
                if logo && (button == BTN_LEFT || button == BTN_RIGHT) {
                    if button == BTN_LEFT {
                        self.begin_move(id, button);
                    } else {
                        let geo = self.wm.get(id).map(|w| w.geo()).unwrap();
                        let p = self.pointer_pos;
                        let left = p.x < geo.loc.x + geo.size.w / 2.0;
                        let top = p.y < geo.loc.y + geo.size.h / 2.0;
                        self.begin_resize(id, button, left, !left, top, !top);
                    }
                    return;
                }
            } else if self.wm.focused.is_some() {
                self.focus_window(None);
            }
        }
        pointer.button(self, &ButtonEvent { button, state: state.into(), serial, time });
        pointer.frame(self);
    }

    fn on_axis<B: InputBackend>(&mut self, evt: B::PointerAxisEvent) {
        let h = evt.amount(Axis::Horizontal).unwrap_or_else(|| evt.amount_v120(Axis::Horizontal).unwrap_or(0.0) * 15.0 / 120.0);
        let v = evt.amount(Axis::Vertical).unwrap_or_else(|| evt.amount_v120(Axis::Vertical).unwrap_or(0.0) * 15.0 / 120.0);
        if self.shell_scroll(v) {
            self.queue_redraw_all();
            return;
        }
        self.refresh_pointer_focus();
        let mut frame = AxisFrame::new(evt.time_msec()).source(evt.source());
        if h != 0.0 {
            frame = frame.relative_direction(Axis::Horizontal, evt.relative_direction(Axis::Horizontal)).value(Axis::Horizontal, h);
            if let Some(d) = evt.amount_v120(Axis::Horizontal) {
                frame = frame.v120(Axis::Horizontal, d as i32);
            }
        }
        if v != 0.0 {
            frame = frame.relative_direction(Axis::Vertical, evt.relative_direction(Axis::Vertical)).value(Axis::Vertical, v);
            if let Some(d) = evt.amount_v120(Axis::Vertical) {
                frame = frame.v120(Axis::Vertical, d as i32);
            }
        }
        if evt.source() == AxisSource::Finger {
            if evt.amount(Axis::Horizontal) == Some(0.0) {
                frame = frame.stop(Axis::Horizontal);
            }
            if evt.amount(Axis::Vertical) == Some(0.0) {
                frame = frame.stop(Axis::Vertical);
            }
        }
        if let Some(pointer) = self.seat.get_pointer() {
            pointer.axis(self, frame);
            pointer.frame(self);
        }
    }

    // -- interactive move / resize -----------------------------------------------------

    /// Client-initiated move (title bar drag): only valid while a button is held.
    pub fn start_move_grab(&mut self, id: u32, serial: Serial) {
        let Some(pointer) = self.seat.get_pointer() else { return };
        if !pointer.has_grab(serial) {
            return;
        }
        let button = pointer.grab_start_data().map(|d| d.button).unwrap_or(BTN_LEFT);
        // Dragging a maximized/tiled window restores it under the pointer.
        if let Some(w) = self.wm.get(id) {
            if !matches!(w.layout, Layout::Floating) {
                let (work, out) = (self.work_area(), self.output_rect());
                let geo = w.geo();
                let frac = ((self.pointer_pos.x - geo.loc.x) / geo.size.w.max(1.0)).clamp(0.0, 1.0);
                let restore_w = match w.layout {
                    Layout::Maximized { restore } | Layout::Tiled { restore, .. } | Layout::Fullscreen { restore } => restore.size.w,
                    Layout::Floating => geo.size.w,
                };
                self.wm.set_layout(id, Layout::Floating, work, out);
                let x = self.pointer_pos.x - restore_w * frac;
                let y = geo.loc.y;
                self.wm.set_target(id, (x, y).into(), true);
            }
        }
        pointer.unset_grab(self, serial, 0);
        self.begin_move(id, button);
    }

    pub fn start_resize_grab(&mut self, id: u32, serial: Serial, left: bool, right: bool, top: bool, bottom: bool) {
        let Some(pointer) = self.seat.get_pointer() else { return };
        if !pointer.has_grab(serial) {
            return;
        }
        let button = pointer.grab_start_data().map(|d| d.button).unwrap_or(BTN_LEFT);
        pointer.unset_grab(self, serial, 0);
        self.begin_resize(id, button, left, right, top, bottom);
    }

    fn begin_move(&mut self, id: u32, button: u32) {
        let Some(w) = self.wm.get(id) else { return };
        self.wm.grab = Some(Grab { win: id, button, kind: GrabKind::Move { pointer_start: self.pointer_pos, win_start: w.target } });
        self.cursor_status = smithay::input::pointer::CursorImageStatus::Named(smithay::input::pointer::CursorIcon::Grabbing);
    }

    fn begin_resize(&mut self, id: u32, button: u32, left: bool, right: bool, top: bool, bottom: bool) {
        let Some(w) = self.wm.get(id) else { return };
        if !matches!(w.layout, Layout::Floating) {
            return;
        }
        let start = w.target_geo();
        self.wm.grab = Some(Grab {
            win: id,
            button,
            kind: GrabKind::Resize { pointer_start: self.pointer_pos, start, left, right, top, bottom },
        });
        if let Some(w) = self.wm.get_mut(id) {
            let anchor = Point::from((start.loc.x + start.size.w, start.loc.y + start.size.h));
            w.resize_anchor = Some((anchor, left, top));
        }
    }

    /// Apply pointer motion to an active grab; returns true if one is active.
    fn update_grab(&mut self, pos: Point<f64, Logical>) -> bool {
        let Some(grab) = &self.wm.grab else { return false };
        let id = grab.win;
        match grab.kind {
            GrabKind::Move { pointer_start, win_start } => {
                let mut loc = win_start + (pos - pointer_start);
                let work = self.work_area();
                // Keep the title bar reachable below the panel.
                loc.y = loc.y.max(work.loc.y);
                self.wm.set_target(id, loc, false);
            }
            GrabKind::Resize { pointer_start, start, left, right, top, bottom } => {
                let d = pos - pointer_start;
                let mut w = start.size.w;
                let mut h = start.size.h;
                if right {
                    w += d.x;
                }
                if left {
                    w -= d.x;
                }
                if bottom {
                    h += d.y;
                }
                if top {
                    h -= d.y;
                }
                let size = smithay::utils::Size::from((w.max(160.0).round() as i32, h.max(100.0).round() as i32));
                self.wm.configure(id, Some(size), false, false, None);
            }
        }
        true
    }

    fn end_grab(&mut self) {
        if let Some(g) = self.wm.grab.take() {
            if let Some(w) = self.wm.get_mut(g.win) {
                w.resize_anchor = None;
            }
            // Dropping a window at the screen's top edge maximizes it, at the
            // sides tiles it (like GNOME).
            if let GrabKind::Move { .. } = g.kind {
                let (work, out) = (self.work_area(), self.output_rect());
                let p = self.pointer_pos;
                if p.y <= out.loc.y + 2.0 {
                    self.wm.set_layout(g.win, Layout::Maximized { restore: work }, work, out);
                } else if p.x <= out.loc.x + 1.0 {
                    self.wm.set_layout(g.win, Layout::Tiled { side: Tile::Left, restore: work }, work, out);
                } else if p.x >= out.loc.x + out.size.w - 2.0 {
                    self.wm.set_layout(g.win, Layout::Tiled { side: Tile::Right, restore: work }, work, out);
                }
            }
        }
        self.cursor_status = smithay::input::pointer::CursorImageStatus::default_named();
        self.queue_redraw_all();
    }
}
