//! Output backends: DRM/KMS on a real seat, a winit window for development,
//! and a headless offscreen backend for tests and profiling.

pub mod headless;
pub mod udev;
pub mod winit;

use std::time::Duration;

use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::ImportDma;
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::Monotonic;
use smithay::wayland::presentation::Refresh;
use smithay::wayland::shm::ShmState;

use crate::render::SceneCtx;
use crate::state::Lumen;

pub enum Backend {
    Udev(udev::UdevData),
    Winit(winit::WinitData),
    Headless(headless::HeadlessData),
    /// Placeholder while the real backend is being constructed.
    None,
}

impl Backend {
    pub fn queue_redraw(&mut self, output: &Output) {
        match self {
            Backend::Udev(d) => d.queue_redraw(output),
            Backend::Winit(d) => d.queue_redraw(),
            Backend::Headless(d) => d.queue_redraw(),
            Backend::None => {}
        }
    }

    pub fn with_renderer<T>(&mut self, f: impl FnOnce(&mut GlesRenderer) -> T) -> Option<T> {
        match self {
            Backend::Udev(d) => d.renderer.as_mut().map(f),
            Backend::Winit(d) => Some(f(d.backend.renderer())),
            Backend::Headless(d) => Some(f(&mut d.renderer)),
            Backend::None => None,
        }
    }

    pub fn import_dmabuf(&mut self, dmabuf: &Dmabuf) -> bool {
        self.with_renderer(|r| r.import_dmabuf(dmabuf, None).is_ok()).unwrap_or(false)
    }

    pub fn change_vt(&mut self, vt: i32) {
        if let Backend::Udev(d) = self {
            d.change_vt(vt);
        }
    }

    /// Apply pointer settings to the real input devices (udev only);
    /// returns how many were configured.
    pub fn configure_input_devices(&mut self, cfg: &crate::inputcfg::InputConfig) -> usize {
        match self {
            Backend::Udev(d) => d.configure_input_devices(cfg),
            _ => 0,
        }
    }

    pub fn update_leds(&mut self, leds: smithay::input::keyboard::LedState) {
        if let Backend::Udev(d) = self {
            d.update_leds(leds);
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Backend::Udev(_) => "udev",
            Backend::Winit(_) => "winit",
            Backend::Headless(_) => "headless",
            Backend::None => "none",
        }
    }
}

/// Update shm formats from the renderer once it exists.
pub fn update_shm_formats(shm: &mut ShmState, r: &GlesRenderer) {
    use smithay::backend::renderer::ImportMemWl;
    shm.update_formats(r.shm_formats());
}

impl Lumen {
    /// Step all animations to `target` (the predicted presentation time).
    pub fn advance_animations(&mut self, target: Duration) {
        // After an idle period the last frame may be seconds old: animations
        // started since then (by input, a new window) begin now, so stepping
        // them by the whole gap would skip their first frames.
        let from = if self.shell.anim_running { self.shell.last_anim_time } else { self.now().min(target) };
        self.shell.last_anim_time = target;
        // A stalled frame must not make springs jump either.
        let dt = target.saturating_sub(from).min(Duration::from_millis(100));
        let moving = self.wm.any_animating();
        self.wm.advance(dt);
        let out = self.output_rect();
        self.shell.advance(dt, target, &self.wm, &self.theme, out);
        if moving {
            // Windows slide under a resting pointer.
            self.refresh_pointer_focus();
        }
        self.shell.anim_running = self.animating();
    }

    pub fn animating(&self) -> bool {
        self.wm.any_animating() || self.shell.animating(self.now()) || self.cursor_animating()
    }

    fn cursor_animating(&self) -> bool {
        if let smithay::input::pointer::CursorImageStatus::Named(icon) = &self.cursor_status {
            return self.cursor_theme.is_animated(icon.name());
        }
        false
    }

    /// Split the state into the backend (which owns the renderer) and the
    /// scene context, so a frame can be built while the renderer is borrowed.
    pub fn split_for_render(&mut self) -> (&mut Backend, SceneCtx<'_>) {
        let now = self.now();
        let Lumen { backend, wm, shell, theme, cursor_theme, cursor_status, dnd_icon, pointer_pos, render, stats, outputs, .. } = self;
        let ctx = SceneCtx {
            wm,
            stats,
            scale: 1.0,
            shell,
            theme,
            cursor_theme,
            cursor_status,
            dnd_icon: dnd_icon.as_ref(),
            primary: outputs.first().map(|o| o.name()),
            pointer: *pointer_pos,
            rs: render,
            now,
        };
        (backend, ctx)
    }

    /// The output showing most of window `id`: its frame callbacks,
    /// presentation feedback and preferred scale follow that output.
    pub fn window_output(&self, id: u32) -> Option<Output> {
        let r = self.wm.get(id)?.target_geo();
        let mut best: Option<(f64, &Output)> = None;
        for o in &self.outputs {
            let area = crate::render::output_logical_rect(o).intersection(r).map(|i| i.size.w * i.size.h).unwrap_or(0.0);
            if area > 0.0 && best.is_none_or(|(a, _)| area > a) {
                best = Some((area, o));
            }
        }
        best.map(|(_, o)| o).or_else(|| self.primary_output()).cloned()
    }

    /// After a frame was rendered: frame callbacks, output enter/leave and
    /// preferred buffer scale/transform for every window.
    pub fn post_repaint(&mut self, output: &Output, time: Duration) {
        use smithay::desktop::space::SpaceElement;
        let throttle = Some(Duration::from_secs(1));
        let current = self.wm.current;
        let out_rect = crate::render::output_logical_rect(output).to_i32_round();
        let scale = output.current_scale();
        let transform = output.current_transform();
        let homes: Vec<Option<Output>> = self.wm.windows.iter().map(|w| self.window_output(w.id)).collect();
        for (w, home) in self.wm.windows.iter().zip(homes) {
            let primary = home.as_ref() == Some(output);
            let visible = w.mapped && (w.ws == current || self.wm.ws_visible(w.ws)) && primary;
            let out = output.clone();
            w.window.send_frame(output, time, throttle, |_, _| if visible { Some(out.clone()) } else { None });
            // wl_surface.enter/leave follow where the window is on its workspace.
            let render_loc = (w.target - w.window.geometry().loc.to_f64()).to_i32_round();
            let mut bbox = w.window.bbox();
            bbox.loc += render_loc;
            match bbox.intersection(out_rect).filter(|_| w.mapped && w.ws == current) {
                Some(mut overlap) => {
                    overlap.loc -= render_loc;
                    w.window.output_enter(output, overlap);
                }
                None => w.window.output_leave(output),
            }
            if primary {
                w.window.with_surfaces(|surface, states| {
                    smithay::wayland::compositor::send_surface_state(surface, states, scale.integer_scale(), transform);
                    smithay::wayland::fractional_scale::with_fractional_scale(states, |fs| fs.set_preferred_scale(scale.fractional_scale()));
                });
            }
        }
        if let smithay::input::pointer::CursorImageStatus::Surface(surface) = &self.cursor_status {
            send_frames(surface, output, time);
        }
        if let Some(icon) = &self.dnd_icon {
            send_frames(icon, output, time);
        }
        // Icons the frame wanted from the icon theme load in the background.
        self.load_requested_icons();
    }

    /// Presentation feedback for everything visible on `output`.
    pub fn presentation_feedback(&self, output: &Output) -> smithay::desktop::utils::OutputPresentationFeedback {
        let mut feedback = smithay::desktop::utils::OutputPresentationFeedback::new(output);
        for w in &self.wm.windows {
            if w.mapped && w.ws == self.wm.current && self.window_output(w.id).as_ref() == Some(output) {
                w.window.take_presentation_feedback(&mut feedback, |_, _| Some(output.clone()), |_, _| {
                    smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind::Vsync
                });
            }
        }
        feedback
    }
}

fn send_frames(surface: &WlSurface, output: &Output, time: Duration) {
    smithay::desktop::utils::send_frames_surface_tree(surface, output, time, Some(Duration::ZERO), |_, _| Some(output.clone()));
}

/// Mark feedback as presented at `time`.
pub fn present(
    mut feedback: smithay::desktop::utils::OutputPresentationFeedback,
    time: smithay::utils::Time<Monotonic>,
    refresh: Duration,
    seq: u64,
    hw: bool,
) {
    use smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind;
    let flags = if hw { Kind::Vsync | Kind::HwClock | Kind::HwCompletion } else { Kind::Vsync };
    feedback.presented(time, Refresh::fixed(refresh), seq, flags);
}
