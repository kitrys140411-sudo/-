//! Development backend: the compositor runs inside a window of the host
//! compositor (Wayland or X11). Input comes from that window.

use std::time::{Duration, Instant};

use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::winit::{self, WinitEvent, WinitGraphicsBackend};
use smithay::output::{Mode, Output, PhysicalProperties, Scale, Subpixel};
use smithay::reexports::winit::dpi::LogicalSize;
use smithay::reexports::winit::window::Window as WinitWindow;
use smithay::utils::Transform;

use super::Backend;
use crate::state::Lumen;

pub struct WinitData {
    pub backend: WinitGraphicsBackend<GlesRenderer>,
    pub output: Output,
    damage_tracker: OutputDamageTracker,
    /// Frames to render without damage tracking (after resizes).
    full_redraw: u8,
    redraw_queued: bool,
}

impl WinitData {
    pub fn queue_redraw(&mut self) {
        if !self.redraw_queued {
            self.redraw_queued = true;
            self.backend.window().request_redraw();
        }
    }
}

fn output_scale(backend: &WinitGraphicsBackend<GlesRenderer>) -> f64 {
    std::env::var("LUMEN_SCALE")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or_else(|| backend.scale_factor())
        .clamp(0.5, 4.0)
}

pub fn init(state: &mut Lumen) -> Result<(), String> {
    let attributes = WinitWindow::default_attributes()
        .with_inner_size(LogicalSize::new(1280.0, 800.0))
        .with_title("Lumen")
        .with_visible(true);
    let (mut backend, event_loop) = winit::init_from_attributes::<GlesRenderer>(attributes).map_err(|e| format!("winit: {e}"))?;
    super::update_shm_formats(&mut state.shm_state, backend.renderer());
    let size = backend.window_size();
    let scale = output_scale(&backend);
    let output = Output::new(
        "WINIT-1".into(),
        PhysicalProperties { size: (0, 0).into(), subpixel: Subpixel::Unknown, make: "Lumen".into(), model: "Winit".into() },
    );
    let mode = Mode { size, refresh: 60_000 };
    output.change_current_state(Some(mode), Some(Transform::Flipped180), Some(Scale::Fractional(scale)), Some((0, 0).into()));
    output.set_preferred(mode);
    output.create_global::<Lumen>(&state.dh);
    let damage_tracker = OutputDamageTracker::from_output(&output);
    state.outputs.push(output.clone());
    state.pointer_pos = (size.w as f64 / scale / 2.0, size.h as f64 / scale / 2.0).into();
    state.backend = Backend::Winit(WinitData { backend, output, damage_tracker, full_redraw: 2, redraw_queued: false });

    state
        .lh
        .insert_source(event_loop, |event, _, state| match event {
            WinitEvent::Resized { size, .. } => {
                let Backend::Winit(d) = &mut state.backend else { return };
                let scale = output_scale(&d.backend);
                let mode = Mode { size, refresh: 60_000 };
                d.output.change_current_state(Some(mode), None, Some(Scale::Fractional(scale)), None);
                d.output.set_preferred(mode);
                d.full_redraw = 2;
                state.render.wallpaper = None;
                state.render.wallpaper_blurred = None;
                state.render.blur.clear();
                state.on_output_resized();
                state.queue_redraw_all();
            }
            WinitEvent::Input(event) => state.process_input_event(event),
            WinitEvent::Redraw => render(state),
            WinitEvent::CloseRequested => state.signal.stop(),
            WinitEvent::Focus(_) => {}
        })
        .map_err(|e| format!("winit source: {e}"))?;
    state.queue_redraw_all();
    Ok(())
}

fn render(state: &mut Lumen) {
    let Backend::Winit(d) = &mut state.backend else { return };
    d.redraw_queued = false;
    let output = d.output.clone();
    let refresh = Duration::from_micros(16_667);
    state.advance_animations(state.now() + refresh);
    let start = Instant::now();
    let result = {
        let (backend, mut ctx) = state.split_for_render();
        let Backend::Winit(d) = backend else { return };
        let (elements, clear) = crate::render::build_scene(d.backend.renderer(), &mut ctx, &output);
        let age = if d.full_redraw > 0 {
            d.full_redraw -= 1;
            0
        } else {
            d.backend.buffer_age().unwrap_or(0)
        };
        let rendered = match d.backend.bind() {
            Ok((renderer, mut fb)) => d
                .damage_tracker
                .render_output(renderer, &mut fb, age, &elements, clear)
                .map(|r| r.damage.cloned())
                .map_err(|e| format!("{e:?}")),
            Err(e) => Err(format!("{e:?}")),
        };
        match rendered {
            Ok(Some(damage)) => d.backend.submit(Some(&damage)).map(|_| true).map_err(|e| format!("{e:?}")),
            Ok(None) => Ok(false),
            Err(e) => Err(e),
        }
    };
    match result {
        Ok(true) => state.stats.push_render(start.elapsed().as_secs_f32() * 1000.0),
        Ok(false) => {}
        Err(e) => tracing::warn!("winit render failed: {e}"),
    }
    let now = state.now();
    let animating = state.animating();
    state.stats.push_present(now, refresh, animating);
    let feedback = state.presentation_feedback(&output);
    super::present(feedback, state.clock.now(), refresh, 0, false);
    state.post_repaint(&output, now);
    state.notify_ready();
    if animating {
        state.backend.queue_redraw(&output);
    }
}
