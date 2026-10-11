//! Wayland protocol handlers.

use smithay::backend::renderer::utils::{on_commit_buffer_handler, with_renderer_surface_state};
use smithay::desktop::{find_popup_root_surface, get_popup_toplevel_coords, PopupKind, PopupKeyboardGrab, PopupPointerGrab, PopupUngrabStrategy, Window};
use smithay::input::pointer::{CursorImageStatus, CursorImageSurfaceData, Focus};
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::reexports::wayland_server::protocol::wl_seat::WlSeat;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Client, Resource};
use smithay::utils::{Rectangle, Serial};
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{get_parent, is_sync_subsurface, with_states, CompositorClientState, CompositorHandler, CompositorState, SurfaceAttributes};
use smithay::wayland::dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier};
use smithay::wayland::fractional_scale::{with_fractional_scale, FractionalScaleHandler};
use smithay::wayland::output::OutputHandler;
use smithay::wayland::selection::data_device::{set_data_device_focus, ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler};
use smithay::wayland::selection::primary_selection::{set_primary_focus, PrimarySelectionHandler, PrimarySelectionState};
use smithay::wayland::selection::SelectionHandler;
use smithay::wayland::shell::xdg::decoration::XdgDecorationHandler;
use smithay::wayland::shell::xdg::{PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState};
use smithay::wayland::shm::{ShmHandler, ShmState};
use smithay::wayland::xdg_activation::{XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData};
use smithay::{
    delegate_compositor, delegate_cursor_shape, delegate_dmabuf, delegate_fractional_scale,
    delegate_output, delegate_presentation, delegate_seat, delegate_shm,
    delegate_single_pixel_buffer, delegate_viewporter, delegate_xdg_activation, delegate_xdg_decoration,
    delegate_xdg_shell,
};
use smithay::backend::allocator::dmabuf::Dmabuf;

use crate::state::{ClientState, Lumen};
use crate::wm::Layout;

impl BufferHandler for Lumen {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) {}
}

impl CompositorHandler for Lumen {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().expect("unknown client data").compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if let Some(id) = self.wm.find_toplevel(&root) {
                self.on_window_commit(id, &root == surface);
            }
        }
        self.popups.commit(surface);
        if let Some(PopupKind::Xdg(popup)) = self.popups.find_popup(surface) {
            if !popup.is_initial_configure_sent() {
                let _ = popup.send_configure();
            }
        }
        if matches!(&self.cursor_status, CursorImageStatus::Surface(s) if s == surface) {
            with_states(surface, |states| {
                if let Some(attrs) = states.data_map.get::<CursorImageSurfaceData>() {
                    let delta = states.cached_state.get::<SurfaceAttributes>().current().buffer_delta.take();
                    if let Some(delta) = delta {
                        attrs.lock().unwrap().hotspot -= delta;
                    }
                }
            });
        }
        self.queue_redraw_all();
    }
}

impl Lumen {
    fn on_window_commit(&mut self, id: u32, is_root: bool) {
        let Some(win) = self.wm.get(id) else { return };
        win.window.on_commit();
        let Some(toplevel) = win.window.toplevel().cloned() else { return };
        if !toplevel.is_initial_configure_sent() {
            // Let the client choose its size, but tell it which edges are free.
            toplevel.with_pending_state(|s| {
                s.capabilities.set(xdg_toplevel::WmCapabilities::Maximize);
                s.capabilities.set(xdg_toplevel::WmCapabilities::Fullscreen);
            });
            toplevel.send_configure();
            return;
        }
        if !is_root {
            return;
        }
        let has_buffer = with_renderer_surface_state(toplevel.wl_surface(), |s| s.buffer().is_some()).unwrap_or(false);
        let mapped = win.mapped;
        if !mapped && has_buffer {
            let work = self.work_area();
            self.wm.map(id, work);
            self.focus_window(Some(id));
            tracing::info!(id, title = %self.wm.get(id).map(|w| w.title()).unwrap_or_default(), "window mapped");
        } else if mapped && !has_buffer {
            self.close_animation(id);
            if let Some(w) = self.wm.get_mut(id) {
                w.mapped = false;
                w.open.snap(0.0);
            }
            if self.wm.focused == Some(id) {
                let next = self.wm.top_on_current();
                self.focus_window(next);
            }
        } else if mapped {
            self.on_window_resized(id);
        }
    }

    /// Keep the right edges anchored during top/left resizes; clamp new windows.
    fn on_window_resized(&mut self, id: u32) {
        let grabbing = self.wm.grab.is_some();
        let work = self.work_area();
        let Some(w) = self.wm.get_mut(id) else { return };
        let size = w.size();
        if size == w.last_size {
            return;
        }
        let old = w.last_size;
        w.last_size = size;
        let requested = w.requested_size == Some(size);
        if let Some((anchor, left, top)) = w.resize_anchor {
            let x = if left { anchor.x - size.w as f64 } else { w.target.x };
            let y = if top { anchor.y - size.h as f64 } else { w.target.y };
            w.target = (x, y).into();
            w.pos.snap(x, y);
        } else if matches!(w.layout, Layout::Floating) && old.w > 0 && !grabbing && !requested {
            // Client-initiated resize (e.g. font change): keep the window centered
            // on where it was so it grows symmetrically.
            let dx = (size.w - old.w) as f64 / 2.0;
            let dy = (size.h - old.h) as f64 / 2.0;
            let target = (w.target.x - dx, w.target.y - dy).into();
            let target = clamp_to(target, size.to_f64(), work);
            self.wm.set_target(id, target, true);
        }
    }

    /// Area available to windows: the primary output minus the top panel.
    pub fn work_area(&self) -> Rectangle<f64, smithay::utils::Logical> {
        let out = self.output_rect();
        let panel = self.theme.geometry.panel_height;
        Rectangle::new((out.loc.x, out.loc.y + panel).into(), (out.size.w, (out.size.h - panel).max(1.0)).into())
    }

    pub fn output_rect(&self) -> Rectangle<f64, smithay::utils::Logical> {
        self.primary_output()
            .and_then(|o| {
                let mode = o.current_mode()?;
                let scale = o.current_scale().fractional_scale();
                let size = mode.size.to_f64().to_logical(scale);
                let size = o.current_transform().transform_size(size);
                Some(Rectangle::new(o.current_location().to_f64(), size))
            })
            .unwrap_or_else(|| Rectangle::new((0.0, 0.0).into(), (1280.0, 800.0).into()))
    }

    pub fn focus_window(&mut self, id: Option<u32>) {
        let serial = smithay::utils::SERIAL_COUNTER.next_serial();
        if let Some(id) = id {
            self.wm.raise(id);
        }
        let changed = self.wm.set_focused(id);
        for w in &self.wm.windows {
            let active = Some(w.id) == id;
            if w.window.set_activated(active) {
                if let Some(t) = w.window.toplevel() {
                    if t.is_initial_configure_sent() {
                        t.send_pending_configure();
                    }
                }
            }
        }
        let surface = id.and_then(|id| self.wm.get(id)).map(|w| w.surface().clone());
        if let Some(kb) = self.seat.get_keyboard() {
            if self.shell.wants_keyboard() {
                kb.set_focus(self, None, serial);
            } else {
                kb.set_focus(self, surface, serial);
            }
        }
        if changed {
            self.queue_redraw_all();
        }
    }

    pub fn close_window(&mut self, id: u32) {
        if let Some(t) = self.wm.get(id).and_then(|w| w.window.toplevel().cloned()) {
            t.send_close();
        }
    }
}

fn clamp_to(
    p: smithay::utils::Point<f64, smithay::utils::Logical>,
    size: smithay::utils::Size<f64, smithay::utils::Logical>,
    work: Rectangle<f64, smithay::utils::Logical>,
) -> smithay::utils::Point<f64, smithay::utils::Logical> {
    let x = p.x.min(work.loc.x + work.size.w - size.w).max(work.loc.x);
    let y = p.y.min(work.loc.y + work.size.h - size.h).max(work.loc.y);
    (x, y).into()
}

impl ShmHandler for Lumen {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl XdgShellHandler for Lumen {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let window = Window::new_wayland_window(surface);
        let id = self.wm.add(window);
        tracing::debug!(id, "new toplevel");
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        self.unconstrain_popup(&surface);
        if let Err(e) = self.popups.track_popup(PopupKind::from(surface)) {
            tracing::warn!("cannot track popup: {e}");
        }
    }

    fn reposition_request(&mut self, surface: PopupSurface, positioner: PositionerState, token: u32) {
        surface.with_pending_state(|s| {
            s.geometry = positioner.get_geometry();
            s.positioner = positioner;
        });
        self.unconstrain_popup(&surface);
        surface.send_repositioned(token);
    }

    fn move_request(&mut self, surface: ToplevelSurface, _seat: WlSeat, serial: Serial) {
        if let Some(id) = self.wm.find_toplevel(surface.wl_surface()) {
            self.start_move_grab(id, serial);
        }
    }

    fn resize_request(&mut self, surface: ToplevelSurface, _seat: WlSeat, serial: Serial, edges: xdg_toplevel::ResizeEdge) {
        if let Some(id) = self.wm.find_toplevel(surface.wl_surface()) {
            use xdg_toplevel::ResizeEdge as E;
            let (left, right, top, bottom) = match edges {
                E::Left => (true, false, false, false),
                E::Right => (false, true, false, false),
                E::Top => (false, false, true, false),
                E::Bottom => (false, false, false, true),
                E::TopLeft => (true, false, true, false),
                E::TopRight => (false, true, true, false),
                E::BottomLeft => (true, false, false, true),
                E::BottomRight => (false, true, false, true),
                _ => return,
            };
            self.start_resize_grab(id, serial, left, right, top, bottom);
        }
    }

    fn grab(&mut self, surface: PopupSurface, _seat: WlSeat, serial: Serial) {
        let seat = self.seat.clone();
        let kind = PopupKind::Xdg(surface);
        let Ok(root) = find_popup_root_surface(&kind) else { return };
        let Ok(mut grab) = self.popups.grab_popup::<Lumen>(root, kind, &seat, serial) else { return };
        if let Some(kb) = seat.get_keyboard() {
            if kb.is_grabbed() && !(kb.has_grab(serial) || kb.has_grab(grab.previous_serial().unwrap_or(serial))) {
                grab.ungrab(PopupUngrabStrategy::All);
                return;
            }
            kb.set_focus(self, grab.current_grab(), serial);
            kb.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
        }
        if let Some(ptr) = seat.get_pointer() {
            if ptr.is_grabbed() && !(ptr.has_grab(serial) || ptr.has_grab(grab.previous_serial().unwrap_or_else(|| grab.serial()))) {
                grab.ungrab(PopupUngrabStrategy::All);
                return;
            }
            ptr.set_grab(self, PopupPointerGrab::new(&grab), serial, Focus::Keep);
        }
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        if let Some(id) = self.wm.find_toplevel(surface.wl_surface()) {
            let (work, out) = (self.work_area(), self.output_rect());
            self.wm.set_layout(id, Layout::Maximized { restore: work }, work, out);
            self.queue_redraw_all();
        } else if surface.is_initial_configure_sent() {
            surface.send_configure();
        }
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        if let Some(id) = self.wm.find_toplevel(surface.wl_surface()) {
            let (work, out) = (self.work_area(), self.output_rect());
            self.wm.set_layout(id, Layout::Floating, work, out);
            self.queue_redraw_all();
        }
    }

    fn fullscreen_request(&mut self, surface: ToplevelSurface, _output: Option<WlOutput>) {
        if let Some(id) = self.wm.find_toplevel(surface.wl_surface()) {
            let (work, out) = (self.work_area(), self.output_rect());
            self.wm.set_layout(id, Layout::Fullscreen { restore: work }, work, out);
            self.queue_redraw_all();
        }
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        self.unmaximize_request(surface);
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        let Some(id) = self.wm.find_toplevel(surface.wl_surface()) else { return };
        if self.wm.get(id).is_some_and(|w| w.mapped) {
            self.close_animation(id);
        }
        let was_focused = self.wm.focused == Some(id);
        self.wm.remove(id);
        if was_focused {
            let next = self.wm.mru.iter().copied().find(|i| self.wm.get(*i).is_some_and(|w| w.mapped && w.ws == self.wm.current));
            self.focus_window(next);
        }
        self.shell.on_windows_changed();
        self.queue_redraw_all();
    }

    fn popup_destroyed(&mut self, _surface: PopupSurface) {
        self.queue_redraw_all();
    }

    fn title_changed(&mut self, _surface: ToplevelSurface) {
        self.shell.on_windows_changed();
        self.queue_redraw_all();
    }

    fn app_id_changed(&mut self, _surface: ToplevelSurface) {
        self.shell.on_windows_changed();
        self.queue_redraw_all();
    }
}

impl Lumen {
    /// Keep popups inside the output (flip/slide as the positioner allows).
    fn unconstrain_popup(&self, popup: &PopupSurface) {
        let Ok(root) = find_popup_root_surface(&PopupKind::Xdg(popup.clone())) else { return };
        let Some(id) = self.wm.find_toplevel(&root) else { return };
        let Some(w) = self.wm.get(id) else { return };
        let out = self.output_rect();
        // Target rectangle in the coordinate space of the popup's parent surface.
        let window_loc = w.target - w.window.geometry().loc.to_f64();
        let mut target = out.to_i32_round();
        target.loc -= window_loc.to_i32_round();
        target.loc -= get_popup_toplevel_coords(&PopupKind::Xdg(popup.clone()));
        popup.with_pending_state(|s| {
            s.geometry = s.positioner.get_unconstrained_geometry(target);
        });
    }
}

impl XdgDecorationHandler for Lumen {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        // Client-side decorations everywhere, like GNOME; the compositor adds
        // the rounded corners and shadows.
        toplevel.with_pending_state(|s| s.decoration_mode = Some(DecorationMode::ClientSide));
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, _mode: DecorationMode) {
        toplevel.with_pending_state(|s| s.decoration_mode = Some(DecorationMode::ClientSide));
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        self.request_mode(toplevel, DecorationMode::ClientSide);
    }
}

impl SeatHandler for Lumen {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Lumen> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        let client = focused.and_then(|s| self.dh.get_client(s.id()).ok());
        set_data_device_focus(&self.dh, seat, client.clone());
        set_primary_focus(&self.dh, seat, client);
    }

    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        self.cursor_status = image;
        self.queue_redraw_all();
    }
}

impl SelectionHandler for Lumen {
    type SelectionUserData = ();
}

impl DataDeviceHandler for Lumen {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for Lumen {}
impl ServerDndGrabHandler for Lumen {}

impl PrimarySelectionHandler for Lumen {
    fn primary_selection_state(&self) -> &PrimarySelectionState {
        &self.primary_selection_state
    }
}

impl OutputHandler for Lumen {}

// Cursor-shape requests may come from tablet tools too; tablets use the default cursor.
impl smithay::wayland::tablet_manager::TabletSeatHandler for Lumen {}

impl FractionalScaleHandler for Lumen {
    fn new_fractional_scale(&mut self, surface: WlSurface) {
        let scale = self.primary_output().map(|o| o.current_scale().fractional_scale()).unwrap_or(1.0);
        with_states(&surface, |states| {
            with_fractional_scale(states, |fs| fs.set_preferred_scale(scale));
        });
    }
}

impl XdgActivationHandler for Lumen {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.xdg_activation_state
    }

    fn token_created(&mut self, _token: XdgActivationToken, data: XdgActivationTokenData) -> bool {
        // Only grant tokens requested while the requesting client had focus.
        let Some((serial, _seat)) = data.serial else { return false };
        self.seat.get_keyboard().and_then(|k| k.last_enter()).is_some_and(|last| serial.is_no_older_than(&last))
    }

    fn request_activation(&mut self, _token: XdgActivationToken, data: XdgActivationTokenData, surface: WlSurface) {
        if data.timestamp.elapsed().as_secs() > 10 {
            return;
        }
        if let Some(id) = self.wm.find_toplevel(&surface) {
            let ws = self.wm.get(id).map(|w| w.ws).unwrap_or(self.wm.current);
            self.wm.switch_workspace(ws);
            self.focus_window(Some(id));
        }
    }
}

impl DmabufHandler for Lumen {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }

    fn dmabuf_imported(&mut self, _global: &DmabufGlobal, dmabuf: Dmabuf, notifier: ImportNotifier) {
        if self.backend.import_dmabuf(&dmabuf) {
            let _ = notifier.successful::<Lumen>();
        } else {
            notifier.failed();
        }
    }
}

delegate_compositor!(Lumen);
delegate_shm!(Lumen);
delegate_xdg_shell!(Lumen);
delegate_xdg_decoration!(Lumen);
delegate_seat!(Lumen);
delegate_output!(Lumen);
delegate_fractional_scale!(Lumen);
delegate_viewporter!(Lumen);
delegate_presentation!(Lumen);
delegate_xdg_activation!(Lumen);
delegate_cursor_shape!(Lumen);
delegate_single_pixel_buffer!(Lumen);
delegate_dmabuf!(Lumen);
