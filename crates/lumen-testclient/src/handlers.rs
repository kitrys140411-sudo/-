//! Wayland event handlers: sctk handler traits, delegate macros and the Dispatch
//! implementations for the protocols sctk 0.19 does not wrap (fractional scale,
//! viewporter, presentation time).

use std::num::NonZeroU32;

use crate::sctk;
use sctk::compositor::CompositorHandler;
use sctk::data_device_manager::WritePipe;
use sctk::data_device_manager::data_device::DataDeviceHandler;
use sctk::data_device_manager::data_offer::{DataOfferHandler, DragOffer};
use sctk::data_device_manager::data_source::DataSourceHandler;
use sctk::output::{OutputHandler, OutputState};
use sctk::reexports::client::protocol::wl_data_device::WlDataDevice;
use sctk::reexports::client::protocol::wl_data_device_manager::DndAction;
use sctk::reexports::client::protocol::wl_data_source::WlDataSource;
use sctk::reexports::client::protocol::{
    wl_callback, wl_keyboard, wl_output, wl_pointer, wl_seat, wl_surface,
};
use sctk::reexports::client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};
use sctk::reexports::csd_frame::WindowState;
use sctk::reexports::protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1;
use sctk::reexports::protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::{
    self, WpFractionalScaleV1,
};
use sctk::reexports::protocols::wp::presentation_time::client::wp_presentation::WpPresentation;
use sctk::reexports::protocols::wp::presentation_time::client::wp_presentation_feedback::{
    self, WpPresentationFeedback,
};
use sctk::reexports::protocols::wp::viewporter::client::wp_viewport::WpViewport;
use sctk::reexports::protocols::wp::viewporter::client::wp_viewporter::WpViewporter;
use sctk::registry::{ProvidesRegistryState, RegistryState};
use sctk::seat::keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers};
use sctk::seat::pointer::{BTN_RIGHT, PointerEvent, PointerEventKind, PointerHandler};
use sctk::seat::{Capability, SeatHandler, SeatState};
use sctk::shell::WaylandSurface;
use sctk::shell::xdg::popup::{Popup, PopupConfigure, PopupHandler};
use sctk::shell::xdg::window::{Window, WindowConfigure, WindowHandler};
use sctk::shm::{Shm, ShmHandler};
use sctk::{
    delegate_compositor, delegate_data_device, delegate_keyboard, delegate_output,
    delegate_pointer, delegate_registry, delegate_seat, delegate_shm, delegate_xdg_popup,
    delegate_xdg_shell, delegate_xdg_window, registry_handlers,
};

use crate::App;
use crate::out::{self, emit, escape};

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        if surface == self.window.wl_surface() {
            // With wl_surface v6 sctk only reports wl_surface.preferred_buffer_scale here.
            if surface.version() >= 6 {
                self.preferred_scale_seen = true;
            }
            self.set_integer_scale(new_factor);
        }
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _: u32,
    ) {
        if surface == self.window.wl_surface() {
            self.frame_done();
        }
    }

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
        if surface == self.window.wl_surface() {
            self.update_output_scale();
        }
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
        if surface == self.window.wl_surface() {
            self.update_output_scale();
        }
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        self.update_output_scale();
    }
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl WindowHandler for App {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Window) {
        emit("closed");
        self.exit = Some(0);
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &Window,
        configure: WindowConfigure,
        serial: u32,
    ) {
        // sctk has already acked the configure; the commit in draw_main applies it.
        let req = (
            configure.new_size.0.map_or(0, NonZeroU32::get),
            configure.new_size.1.map_or(0, NonZeroU32::get),
        );
        let s = configure.state;
        let constrained =
            s.intersects(WindowState::MAXIMIZED | WindowState::FULLSCREEN | WindowState::TILED);
        let width = if req.0 > 0 {
            req.0
        } else {
            self.floating_size.0
        };
        let height = if req.1 > 0 {
            req.1
        } else {
            self.floating_size.1
        };
        if req.0 > 0 && req.1 > 0 && !constrained {
            self.floating_size = req;
        }
        let states = out::states_list(&[
            (s.contains(WindowState::MAXIMIZED), "maximized"),
            (s.contains(WindowState::FULLSCREEN), "fullscreen"),
            (s.contains(WindowState::ACTIVATED), "activated"),
            (s.contains(WindowState::RESIZING), "resizing"),
            (s.contains(WindowState::TILED_LEFT), "tiled_left"),
            (s.contains(WindowState::TILED_RIGHT), "tiled_right"),
            (s.contains(WindowState::TILED_TOP), "tiled_top"),
            (s.contains(WindowState::TILED_BOTTOM), "tiled_bottom"),
            (s.contains(WindowState::SUSPENDED), "suspended"),
        ]);
        emit(&format!(
            "configure w={} h={} states={states} size={width}x{height} serial={serial}",
            req.0, req.1
        ));
        self.size = (width, height);
        let first = !self.configured;
        self.configured = true;
        if first {
            self.report_initial_scale();
            self.request_initial_state();
        }
        self.draw_main();
    }
}

impl PopupHandler for App {
    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        popup: &Popup,
        config: PopupConfigure,
    ) {
        let Some(p) = self.popup.as_mut().filter(|p| &p.popup == popup) else {
            return;
        };
        let w = u32::try_from(config.width)
            .ok()
            .filter(|&w| w > 0)
            .unwrap_or(p.size.0);
        let h = u32::try_from(config.height)
            .ok()
            .filter(|&h| h > 0)
            .unwrap_or(p.size.1);
        p.size = (w, h);
        p.configured = true;
        emit(&format!(
            "popup event=configure x={} y={} w={w} h={h}",
            config.position.0, config.position.1
        ));
        self.draw_popup();
    }

    fn done(&mut self, _: &Connection, _: &QueueHandle<Self>, popup: &Popup) {
        if self.popup.as_ref().is_some_and(|p| &p.popup == popup) {
            self.close_popup("done");
        }
    }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        // Only the first seat is used.
        if self.seat.as_ref().is_some_and(|s| s != &seat) {
            return;
        }
        if self.seat.is_none() {
            self.seat = Some(seat.clone());
            if let Some(manager) = &self.data_device_manager {
                self.data_device = Some(manager.get_data_device(qh, &seat));
            }
        }
        match capability {
            Capability::Keyboard if self.keyboard.is_none() => {
                match self.seat_state.get_keyboard::<App, App>(qh, &seat, None) {
                    Ok(keyboard) => self.keyboard = Some(keyboard),
                    Err(e) => eprintln!("lumen-testclient: wl_keyboard: {e}"),
                }
            }
            Capability::Pointer if self.pointer.is_none() => {
                match self.seat_state.get_pointer(qh, &seat) {
                    Ok(pointer) => {
                        if let Some(manager) = &self.cursor_shape_manager {
                            self.cursor_device = Some(manager.get_shape_device(&pointer, qh));
                        }
                        self.pointer = Some(pointer);
                    }
                    Err(e) => eprintln!("lumen-testclient: wl_pointer: {e}"),
                }
            }
            _ => {}
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if self.seat.as_ref() != Some(&seat) {
            return;
        }
        match capability {
            Capability::Keyboard => {
                if let Some(keyboard) = self.keyboard.take()
                    && keyboard.version() >= 3
                {
                    keyboard.release();
                }
                self.main_focused = false;
                self.paste_waiting = false;
            }
            Capability::Pointer => {
                if let Some(device) = self.cursor_device.take() {
                    device.destroy();
                }
                if let Some(pointer) = self.pointer.take()
                    && pointer.version() >= 3
                {
                    pointer.release();
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
            self.main_focused = false;
            self.paste_waiting = false;
        }
    }
}

impl App {
    /// `main` or `popup` for one of our surfaces, None for anything else.
    fn surface_name(&self, surface: &wl_surface::WlSurface) -> Option<&'static str> {
        if surface == self.window.wl_surface() {
            Some("main")
        } else if self
            .popup
            .as_ref()
            .is_some_and(|p| p.popup.wl_surface() == surface)
        {
            Some("popup")
        } else {
            None
        }
    }
}

impl KeyboardHandler for App {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        surface: &wl_surface::WlSurface,
        serial: u32,
        _: &[u32],
        _: &[Keysym],
    ) {
        // A popup grab moves keyboard focus from the toplevel to the popup and back.
        let Some(name) = self.surface_name(surface) else {
            return;
        };
        emit(&format!("focus in surface={name}"));
        if name != "main" {
            return;
        }
        self.main_focused = true;
        if self.opts.copy.is_some() {
            self.offer_selection(serial);
        }
        if self.opts.paste {
            self.request_paste();
        }
    }

    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        surface: &wl_surface::WlSurface,
        _: u32,
    ) {
        if surface == self.window.wl_surface() {
            self.main_focused = false;
            self.paste_waiting = false;
            emit("focus out surface=main");
        } else {
            // The popup (possibly already destroyed by us).
            emit("focus out surface=popup");
        }
    }

    fn press_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        emit_key(&event, "pressed");
        if self.opts.close_on_key && event.keysym == Keysym::Escape {
            emit("exit reason=key");
            self.exit = Some(0);
        }
    }

    fn release_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        emit_key(&event, "released");
    }

    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: Modifiers,
        _: u32,
    ) {
    }
}

fn emit_key(event: &KeyEvent, state: &str) {
    emit(&format!(
        "key sym={} utf8={} state={state} code={}",
        out::keysym_name(event.keysym.name(), event.keysym.raw()),
        escape(event.utf8.as_deref().unwrap_or("")),
        event.raw_code
    ));
}

impl PointerHandler for App {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            let Some(surface) = self.surface_name(&event.surface) else {
                continue;
            };
            let (x, y) = event.position;
            match event.kind {
                PointerEventKind::Enter { serial } => {
                    emit(&format!("enter kind=pointer x={x} y={y} surface={surface}"));
                    if let (Some(device), Some(name)) =
                        (&self.cursor_device, self.opts.cursor_shape)
                    {
                        device.set_shape(serial, crate::cursor_shape(name));
                        emit(&format!("cursor shape={}", name.name()));
                    }
                }
                PointerEventKind::Leave { .. } => {
                    emit(&format!("leave kind=pointer surface={surface}"));
                }
                PointerEventKind::Motion { .. } => {
                    emit(&format!("motion x={x} y={y} surface={surface}"));
                }
                PointerEventKind::Press { button, serial, .. } => {
                    emit(&format!(
                        "button code={button} state=pressed surface={surface}"
                    ));
                    if self.opts.popup {
                        if self.popup.is_some() {
                            self.close_popup("click");
                        } else if button == BTN_RIGHT && surface == "main" {
                            self.open_popup(x, y, serial);
                        }
                    }
                }
                PointerEventKind::Release { button, .. } => {
                    emit(&format!(
                        "button code={button} state=released surface={surface}"
                    ));
                }
                PointerEventKind::Axis {
                    horizontal,
                    vertical,
                    ..
                } => {
                    if horizontal.absolute != 0.0
                        || vertical.absolute != 0.0
                        || horizontal.discrete != 0
                        || vertical.discrete != 0
                    {
                        emit(&format!(
                            "axis vertical={} horizontal={} surface={surface}",
                            vertical.absolute, horizontal.absolute
                        ));
                    }
                }
            }
        }
    }
}

impl DataDeviceHandler for App {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlDataDevice,
        _: f64,
        _: f64,
        _: &wl_surface::WlSurface,
    ) {
    }
    fn leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {}
    fn motion(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice, _: f64, _: f64) {}
    // `--paste` reads the selection when the sync sent on focus-in is done (see
    // App::request_paste), by which time the selection event has been processed.
    fn selection(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {}
    fn drop_performed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {}
}

impl DataOfferHandler for App {
    fn source_actions(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &mut DragOffer,
        _: DndAction,
    ) {
    }
    fn selected_action(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &mut DragOffer,
        _: DndAction,
    ) {
    }
}

impl DataSourceHandler for App {
    fn accept_mime(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlDataSource,
        _: Option<String>,
    ) {
    }

    fn send_request(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlDataSource,
        mime: String,
        pipe: WritePipe,
    ) {
        self.send_selection(mime, pipe);
    }

    fn cancelled(&mut self, _: &Connection, _: &QueueHandle<Self>, source: &WlDataSource) {
        if self
            .copy_source
            .as_ref()
            .is_some_and(|s| s.inner() == source)
        {
            self.copy_source = None;
            emit("copy event=cancelled");
        }
    }

    fn dnd_dropped(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource) {}
    fn dnd_finished(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource) {}
    fn action(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource, _: DndAction) {}
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

// Protocols without sctk helpers. Managers and viewports have no events.

impl Dispatch<WpViewporter, ()> for App {
    fn event(
        _: &mut Self,
        _: &WpViewporter,
        _: <WpViewporter as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WpViewport, ()> for App {
    fn event(
        _: &mut Self,
        _: &WpViewport,
        _: <WpViewport as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WpFractionalScaleManagerV1, ()> for App {
    fn event(
        _: &mut Self,
        _: &WpFractionalScaleManagerV1,
        _: <WpFractionalScaleManagerV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WpFractionalScaleV1, ()> for App {
    fn event(
        app: &mut Self,
        _: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            app.set_fractional_scale(scale);
        }
    }
}

impl Dispatch<WpPresentation, ()> for App {
    fn event(
        _: &mut Self,
        _: &WpPresentation,
        _: <WpPresentation as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // clock_id: timestamps are printed raw, the clock does not matter here.
    }
}

/// User data of the wl_display.sync sent on keyboard focus-in for `--paste`.
pub(crate) struct PasteSync;

impl Dispatch<wl_callback::WlCallback, PasteSync> for App {
    fn event(
        app: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        _: &PasteSync,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            app.paste();
        }
    }
}

/// User data: the commit sequence number the feedback belongs to.
impl Dispatch<WpPresentationFeedback, u64> for App {
    fn event(
        _: &mut Self,
        _: &WpPresentationFeedback,
        event: wp_presentation_feedback::Event,
        seq: &u64,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wp_presentation_feedback::Event::Presented {
                tv_sec_hi,
                tv_sec_lo,
                tv_nsec,
                refresh,
                flags,
                ..
            } => {
                let flags = match flags {
                    WEnum::Value(kind) => kind.bits(),
                    WEnum::Unknown(raw) => raw,
                };
                emit(&format!(
                    "frame seq={seq} presented_us={} refresh_ns={refresh} flags={flags}",
                    out::presented_us(tv_sec_hi, tv_sec_lo, tv_nsec)
                ));
            }
            wp_presentation_feedback::Event::Discarded => {
                emit(&format!(
                    "frame seq={seq} presented_us=0 refresh_ns=0 flags=0 discarded=1"
                ));
            }
            _ => {}
        }
    }
}

delegate_compositor!(App);
delegate_output!(App);
delegate_shm!(App);
delegate_seat!(App);
delegate_keyboard!(App);
delegate_pointer!(App);
delegate_xdg_shell!(App);
delegate_xdg_window!(App);
delegate_xdg_popup!(App);
delegate_data_device!(App);
delegate_registry!(App);
