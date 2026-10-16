//! Global compositor state.

use std::ffi::OsString;
use std::sync::Arc;
use std::time::Duration;

use smithay::desktop::PopupManager;
use smithay::input::keyboard::{Keycode, XkbConfig};
use smithay::input::pointer::CursorImageStatus;
use smithay::input::{Seat, SeatState};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::output::Output;
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{Interest, LoopHandle, LoopSignal, Mode, PostAction, RegistrationToken};
use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::{Display, DisplayHandle};
use smithay::utils::{Clock, Logical, Monotonic, Point};
use smithay::wayland::compositor::{CompositorClientState, CompositorState};
use smithay::wayland::cursor_shape::CursorShapeManagerState;
use smithay::wayland::dmabuf::{DmabufGlobal, DmabufState};
use smithay::wayland::fractional_scale::FractionalScaleManagerState;
use smithay::wayland::output::OutputManagerState;
use smithay::wayland::presentation::PresentationState;
use smithay::wayland::selection::data_device::DataDeviceState;
use smithay::wayland::selection::primary_selection::PrimarySelectionState;
use smithay::wayland::shell::xdg::decoration::XdgDecorationState;
use smithay::wayland::shell::xdg::XdgShellState;
use smithay::wayland::shm::ShmState;
use smithay::wayland::single_pixel_buffer::SinglePixelBufferState;
use smithay::wayland::socket::ListeningSocketSource;
use smithay::wayland::viewporter::ViewporterState;
use smithay::wayland::xdg_activation::XdgActivationState;

use crate::backend::Backend;
use crate::background::{Background, JobQueue};
use crate::backlight::BrightnessCmd;
use crate::cursor::CursorTheme;
use crate::input::Action;
use crate::inputcfg::{InputConfig, KeyboardConfig};
use crate::keybind::{KeyBindings, Mods};
use crate::layouts::{Layouts, XkbNames};
use crate::volume::VolumeCmd;
use crate::render::RenderState;

/// How long after the first frame the renderer prepares what that frame
/// skipped (clients and Xwayland get to start first).
const WARM_UP_DELAY_MS: u64 = 1500;
use crate::shell::Shell;
use crate::theme::Theme;
use crate::wm::Wm;

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

/// Render timing statistics (frame-time HUD, tests, profiling).
#[derive(Default, Debug, Clone)]
pub struct FrameStats {
    pub frames: u64,
    /// Rendering CPU time of recent frames (ms), ring buffer.
    pub render_ms: Vec<f32>,
    /// Intervals between presented frames during animations (ms).
    pub interval_ms: Vec<f32>,
    pub last_present: Option<Duration>,
    pub missed: u64,
    /// Frames whose primary plane showed a client buffer directly (KMS).
    pub scanout_frames: u64,
    /// Frames that were composited (KMS).
    pub composited_frames: u64,
}

impl FrameStats {
    pub fn push_render(&mut self, ms: f32) {
        self.frames += 1;
        if self.render_ms.len() >= 240 {
            self.render_ms.remove(0);
        }
        self.render_ms.push(ms);
    }

    pub fn note_scanout(&mut self, direct: bool) {
        if direct {
            self.scanout_frames += 1;
        } else {
            self.composited_frames += 1;
        }
    }

    /// Record a presentation; `animating` intervals count toward missed frames.
    pub fn push_present(&mut self, now: Duration, refresh: Duration, animating: bool) {
        if let Some(last) = self.last_present {
            let dt = now.saturating_sub(last);
            if animating {
                if self.interval_ms.len() >= 240 {
                    self.interval_ms.remove(0);
                }
                self.interval_ms.push(dt.as_secs_f32() * 1000.0);
                if dt > refresh + refresh / 2 {
                    self.missed += (dt.as_secs_f64() / refresh.as_secs_f64()).round() as u64 - 1;
                }
            }
        }
        self.last_present = Some(now);
    }
}

// Several protocol states are only kept alive here: they own their globals.
#[allow(dead_code)]
pub struct Lumen {
    pub dh: DisplayHandle,
    pub lh: LoopHandle<'static, Lumen>,
    pub signal: LoopSignal,
    pub clock: Clock<Monotonic>,
    pub socket_name: OsString,
    pub backend: Backend,
    pub theme: Theme,

    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub xdg_decoration_state: XdgDecorationState,
    pub shm_state: ShmState,
    pub dmabuf_state: DmabufState,
    pub dmabuf_global: Option<DmabufGlobal>,
    pub seat_state: SeatState<Lumen>,
    pub data_device_state: DataDeviceState,
    pub primary_selection_state: PrimarySelectionState,
    pub output_manager_state: OutputManagerState,
    pub fractional_scale_state: FractionalScaleManagerState,
    pub viewporter_state: ViewporterState,
    pub presentation_state: PresentationState,
    pub xdg_activation_state: XdgActivationState,
    pub cursor_shape_state: CursorShapeManagerState,
    pub single_pixel_buffer_state: SinglePixelBufferState,

    pub seat: Seat<Lumen>,
    pub popups: PopupManager,
    pub cursor_status: CursorImageStatus,
    /// Client surface under the pointer and its origin (None: shell/desktop).
    pub pointer_target: Option<(WlSurface, smithay::utils::Point<f64, smithay::utils::Logical>)>,
    /// Icon of the drag-and-drop in progress (client drags).
    pub dnd_icon: Option<WlSurface>,
    pub cursor_theme: CursorTheme,
    pub pointer_pos: Point<f64, Logical>,
    /// Keys whose release must not reach clients (their press was consumed
    /// by a shortcut or the shell). Keycodes, not symbols: a layout switch
    /// between press and release changes the symbol.
    pub suppressed_keys: Vec<Keycode>,
    /// A modifier-only chord (Alt+Shift) held down and not yet spoiled by
    /// another key or button: it fires on release.
    pub mod_chord: Option<Mods>,
    /// An action triggered by a key release, run after the release was
    /// forwarded.
    pub release_action: Option<Action>,
    /// The held key whose action repeats (volume, brightness) and its timer.
    pub key_repeat: Option<(Keycode, RegistrationToken)>,
    /// Keyboard layouts and pointer settings (input.toml).
    pub input_cfg: InputConfig,
    pub layouts: Layouts,
    pub outputs: Vec<Output>,

    pub wm: Wm,
    pub shell: Shell,
    pub render: RenderState,
    pub stats: FrameStats,
    /// Notify init (READY=1) after the first presented frame.
    pub ready_notified: bool,
    /// Keyboard shortcuts.
    pub keys: KeyBindings,
    pub bg: Background,
    /// Problems found while starting (e.g. a broken keybindings file), shown
    /// as notifications once the desktop is on screen.
    pub startup_notices: Vec<(String, String)>,
    /// wpctl was found missing (logged once).
    pub wpctl_missing: bool,
    pub volume_jobs: JobQueue<VolumeCmd>,
    pub brightness_jobs: JobQueue<BrightnessCmd>,
    /// A missing or read-only backlight was reported (once).
    pub backlight_reported: bool,
    pub battery_warned: crate::power::Warned,
    pub lid_closed: bool,
    /// When suspend was last requested (boot clock, includes time asleep).
    pub last_suspend: Option<Duration>,
    pub xwayland: crate::xwayland::XwaylandState,
    pub pointer_constraints_state: smithay::wayland::pointer_constraints::PointerConstraintsState,
    pub relative_pointer_state: smithay::wayland::relative_pointer::RelativePointerManagerState,
    pub shortcuts_inhibit_state: smithay::wayland::keyboard_shortcuts_inhibit::KeyboardShortcutsInhibitState,
    pub idle_inhibit_state: smithay::wayland::idle_inhibit::IdleInhibitManagerState,
    /// The surface whose pointer constraint is active.
    pub constrained: Option<WlSurface>,
    pub idle: crate::gaming::Idle,
    /// `lumen -- command`: started with the desktop (see [`Lumen::notify_ready`]).
    pub autostart: Vec<String>,
}

impl Lumen {
    pub fn new(display: Display<Lumen>, lh: LoopHandle<'static, Lumen>, signal: LoopSignal, backend: Backend) -> Lumen {
        let dh = display.handle();
        let clock = Clock::<Monotonic>::new();

        let source = ListeningSocketSource::new_auto().expect("cannot create the Wayland socket");
        let socket_name = source.socket_name().to_os_string();
        lh.insert_source(source, |stream, _, state| {
            if let Err(e) = state.dh.insert_client(stream, Arc::new(ClientState::default())) {
                tracing::warn!("cannot add Wayland client: {e}");
            }
        })
        .expect("cannot listen on the Wayland socket");
        lh.insert_source(Generic::new(display, Interest::READ, Mode::Level), |_, display, state| {
            // SAFETY: the display is never dropped while the source exists.
            unsafe { display.get_mut().dispatch_clients(state).expect("dispatching clients failed") };
            Ok(PostAction::Continue)
        })
        .expect("cannot register the Wayland display");

        let compositor_state = CompositorState::new_v6::<Lumen>(&dh);
        let xdg_shell_state = XdgShellState::new_with_capabilities::<Lumen>(
            &dh,
            [xdg_toplevel::WmCapabilities::Fullscreen, xdg_toplevel::WmCapabilities::Maximize],
        );
        let xdg_decoration_state = XdgDecorationState::new::<Lumen>(&dh);
        let shm_state = ShmState::new::<Lumen>(&dh, vec![]);
        let dmabuf_state = DmabufState::new();
        let mut seat_state = SeatState::new();
        let data_device_state = DataDeviceState::new::<Lumen>(&dh);
        let primary_selection_state = PrimarySelectionState::new::<Lumen>(&dh);
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Lumen>(&dh);
        let fractional_scale_state = FractionalScaleManagerState::new::<Lumen>(&dh);
        let viewporter_state = ViewporterState::new::<Lumen>(&dh);
        let presentation_state = PresentationState::new::<Lumen>(&dh, clock.id() as u32);
        let xdg_activation_state = XdgActivationState::new::<Lumen>(&dh);
        let cursor_shape_state = CursorShapeManagerState::new::<Lumen>(&dh);
        let single_pixel_buffer_state = SinglePixelBufferState::new::<Lumen>(&dh);
        let pointer_constraints_state = smithay::wayland::pointer_constraints::PointerConstraintsState::new::<Lumen>(&dh);
        let relative_pointer_state = smithay::wayland::relative_pointer::RelativePointerManagerState::new::<Lumen>(&dh);
        let shortcuts_inhibit_state = smithay::wayland::keyboard_shortcuts_inhibit::KeyboardShortcutsInhibitState::new::<Lumen>(&dh);
        let idle_inhibit_state = smithay::wayland::idle_inhibit::IdleInhibitManagerState::new::<Lumen>(&dh);
        let xwayland = crate::xwayland::XwaylandState::new(&dh);
        crate::startup::mark("wayland globals");

        let mut seat = seat_state.new_wl_seat(&dh, "seat0");
        seat.add_pointer();
        let mut startup_notices: Vec<(String, String)> = Vec::new();
        let (mut input_cfg, input_errors) = InputConfig::load();
        for e in input_errors {
            tracing::error!("input settings: {e}");
            startup_notices.push(("Input settings not loaded".into(), format!("{e}\nThe file was ignored.")));
        }
        let k = &input_cfg.keyboard;
        let names = XkbNames::new(k);
        if seat.add_keyboard(names.config(), k.repeat_delay, k.repeat_rate).is_err() {
            // An unknown layout or option: type with the plain US layout rather than not at all.
            let msg = format!("xkb cannot build a keymap for {}", names.describe());
            tracing::error!("{msg}");
            startup_notices.push(("Keyboard layout not loaded".into(), format!("{msg}.\nThe US layout is in effect.")));
            input_cfg.keyboard = KeyboardConfig::default();
            // Named explicitly: xkbcommon's defaults come from XKB_DEFAULT_* and may be what failed.
            let us = XkbConfig { layout: "us", ..XkbConfig::default() };
            seat.add_keyboard(us, 400, 30).expect("cannot initialize the keyboard (xkbcommon)");
        }
        let layouts = Layouts::new(&input_cfg.keyboard);
        tracing::info!(layouts = %input_cfg.keyboard.xkb_layout(), "keyboard");
        crate::startup::mark("keyboard (xkb keymap)");

        let theme = Theme::load();
        let (keys, key_errors) = KeyBindings::load();
        for e in key_errors {
            tracing::error!("keybindings: {e}");
            startup_notices.push(("Keyboard shortcuts not loaded".to_string(), format!("{e}\nThe default shortcuts are in effect.")));
        }
        tracing::info!("{} keyboard shortcuts", keys.bindings.len());
        crate::startup::mark("theme, keybindings");
        let bg = Background::new(&lh);
        let wm = Wm::new(&theme);
        let mut shell = Shell::new(&theme);
        shell.layout_label = layouts.indicator().map(String::from);
        crate::startup::mark("shell");
        let render = RenderState::new(&theme);
        crate::startup::mark("fonts");
        let cursor_theme = CursorTheme::load();
        crate::startup::mark("cursor theme");

        Lumen {
            dh,
            lh,
            signal,
            clock,
            socket_name,
            backend,
            theme,
            compositor_state,
            xdg_shell_state,
            xdg_decoration_state,
            shm_state,
            dmabuf_state,
            dmabuf_global: None,
            seat_state,
            data_device_state,
            primary_selection_state,
            output_manager_state,
            fractional_scale_state,
            viewporter_state,
            presentation_state,
            xdg_activation_state,
            cursor_shape_state,
            single_pixel_buffer_state,
            seat,
            popups: PopupManager::default(),
            cursor_status: CursorImageStatus::default_named(),
            dnd_icon: None,
            pointer_target: None,
            cursor_theme,
            pointer_pos: (0.0, 0.0).into(),
            suppressed_keys: Vec::new(),
            mod_chord: None,
            release_action: None,
            key_repeat: None,
            input_cfg,
            layouts,
            outputs: Vec::new(),
            wm,
            shell,
            render,
            stats: FrameStats::default(),
            ready_notified: false,
            keys,
            bg,
            startup_notices,
            wpctl_missing: false,
            volume_jobs: JobQueue::default(),
            brightness_jobs: JobQueue::default(),
            backlight_reported: false,
            battery_warned: Default::default(),
            lid_closed: false,
            last_suspend: None,
            xwayland,
            pointer_constraints_state,
            relative_pointer_state,
            shortcuts_inhibit_state,
            idle_inhibit_state,
            constrained: None,
            idle: crate::gaming::Idle::new(),
            autostart: Vec::new(),
        }
    }

    /// Current time on the presentation clock.
    pub fn now(&self) -> Duration {
        self.clock.now().into()
    }

    pub fn primary_output(&self) -> Option<&Output> {
        self.outputs.first()
    }

    /// The output changed size or scale: re-apply layouts that depend on it.
    pub fn on_output_resized(&mut self) {
        let (work, out) = (self.work_area(), self.output_rect());
        let ids: Vec<(u32, crate::wm::Layout)> = self.wm.windows.iter().map(|w| (w.id, w.layout)).collect();
        for (id, layout) in ids {
            if !matches!(layout, crate::wm::Layout::Floating) {
                self.wm.set_layout(id, layout, work, out);
            }
        }
        let scale = self.primary_output().map(|o| o.current_scale().fractional_scale()).unwrap_or(1.0);
        for w in &self.wm.windows {
            w.window.with_surfaces(|surface, states| {
                let _ = surface;
                smithay::wayland::fractional_scale::with_fractional_scale(states, |fs| fs.set_preferred_scale(scale));
            });
        }
        let p = self.pointer_pos;
        self.pointer_pos = (p.x.clamp(out.loc.x, out.loc.x + out.size.w - 1.0), p.y.clamp(out.loc.y, out.loc.y + out.size.h - 1.0)).into();
        self.update_xwayland_scale();
    }

    /// Ask every output to redraw at the next opportunity.
    pub fn queue_redraw_all(&mut self) {
        for o in self.outputs.clone() {
            self.backend.queue_redraw(&o);
        }
    }

    /// Tell lumen-init we are ready (first frame on screen).
    pub fn notify_ready(&mut self) {
        if self.ready_notified {
            return;
        }
        self.ready_notified = true;
        if let Ok(path) = std::env::var("LUMEN_NOTIFY_SOCKET") {
            if let Ok(sock) = std::os::unix::net::UnixDatagram::unbound() {
                let _ = sock.send_to(b"READY=1\nSTATUS=first frame presented", path);
            }
        }
        tracing::info!("first frame presented");
        crate::startup::finish();
        self.render.lazy_at_first_frame = Some(self.render.shaders.as_ref().map(|s| s.compiled_lazy()).unwrap_or_default());
        // X11 support starts once the desktop is on screen (off the boot
        // path), and the autostart program right after it, so it gets
        // DISPLAY and does not compete with the first frame for the CPU.
        self.lh.insert_idle(|state| {
            state.start_xwayland();
            let cmd = std::mem::take(&mut state.autostart);
            if !cmd.is_empty() {
                state.spawn(&cmd);
            }
        });
        // What the first frame left out (programs, the overview's blurred
        // wallpaper) is prepared a moment later, see render::warm_up.
        let warm = Timer::from_duration(Duration::from_millis(WARM_UP_DELAY_MS));
        let _ = self.lh.insert_source(warm, |_, _, state| {
            state.render.warm_up = true;
            state.queue_redraw_all();
            TimeoutAction::Drop
        });
        // The idle clock starts with the desktop.
        self.note_input();
        for (title, body) in std::mem::take(&mut self.startup_notices) {
            self.notify(&title, &body);
        }
    }

    /// Re-read the keybinding files. On an error the current table stays
    /// and the problem is shown as a notification.
    pub fn reload_keybindings(&mut self) -> Result<usize, String> {
        match KeyBindings::reload() {
            Ok(keys) => {
                tracing::info!("keybindings reloaded: {} shortcuts", keys.bindings.len());
                self.keys = keys;
                self.shell_sheet_bindings_changed();
                Ok(self.keys.bindings.len())
            }
            Err(e) => {
                tracing::error!("keybindings: {e}");
                self.notify("Keyboard shortcuts not updated", &format!("{e}\nThe previous shortcuts stay in effect."));
                Err(e)
            }
        }
    }
}
