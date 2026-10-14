//! Headless backend: renders into an offscreen texture on a software or
//! hardware EGL device, with a simulated 60 Hz vblank. Used by automated tests
//! (screenshots and synthetic input via the IPC socket) and for profiling frame
//! times on machines without a display.

use std::time::{Duration, Instant};

use smithay::backend::allocator::Fourcc;
use smithay::backend::egl::{EGLContext, EGLDevice, EGLDisplay};
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::{Bind, Offscreen};
use smithay::output::{Mode, Output, PhysicalProperties, Scale, Subpixel};
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::LoopHandle;
use smithay::utils::{Physical, Size, Transform};

use crate::state::Lumen;

pub struct HeadlessData {
    pub renderer: GlesRenderer,
    pub output: Output,
    damage_tracker: OutputDamageTracker,
    target: GlesTexture,
    refresh: Duration,
    redraw_queued: bool,
    timer_armed: bool,
    lh: LoopHandle<'static, Lumen>,
    epoch: Instant,
    pub frames: u64,
}

pub fn renderer_for_device(prefer_hardware: bool) -> Result<GlesRenderer, String> {
    let devices: Vec<EGLDevice> = EGLDevice::enumerate().map_err(|e| format!("EGL device enumeration: {e}"))?.collect();
    let pick = devices
        .iter()
        .position(|d| d.is_software() != prefer_hardware)
        .or(if devices.is_empty() { None } else { Some(0) })
        .ok_or("no EGL devices")?;
    let device = devices.into_iter().nth(pick).unwrap();
    let display = unsafe { EGLDisplay::new(device) }.map_err(|e| format!("EGL display: {e}"))?;
    let context = EGLContext::new(&display).map_err(|e| format!("EGL context: {e}"))?;
    unsafe { GlesRenderer::new(context) }.map_err(|e| format!("GLES renderer: {e}"))
}

impl HeadlessData {
    pub fn new(lh: LoopHandle<'static, Lumen>, size: Size<i32, Physical>, scale: f64) -> Result<HeadlessData, String> {
        let mut renderer = renderer_for_device(std::env::var("LUMEN_HEADLESS_HW").is_ok())?;
        let target: GlesTexture = renderer
            .create_buffer(Fourcc::Abgr8888, size.to_logical(1).to_buffer(1, Transform::Normal))
            .map_err(|e| format!("offscreen buffer: {e:?}"))?;
        // Tests name it like a laptop panel (eDP-1) to exercise the lid logic.
        let name = std::env::var("LUMEN_OUTPUT_NAME").ok().filter(|n| !n.is_empty()).unwrap_or_else(|| "HEADLESS-1".into());
        let output = Output::new(
            name,
            PhysicalProperties { size: (0, 0).into(), subpixel: Subpixel::Unknown, make: "Lumen".into(), model: "Headless".into() },
        );
        let mode = Mode { size, refresh: 60_000 };
        output.change_current_state(Some(mode), Some(Transform::Normal), Some(Scale::Fractional(scale)), Some((0, 0).into()));
        output.set_preferred(mode);
        let damage_tracker = OutputDamageTracker::from_output(&output);
        Ok(HeadlessData {
            renderer,
            output,
            damage_tracker,
            target,
            refresh: Duration::from_micros(16_667),
            redraw_queued: false,
            timer_armed: false,
            lh,
            epoch: Instant::now(),
            frames: 0,
        })
    }

    pub fn queue_redraw(&mut self) {
        self.redraw_queued = true;
        if self.timer_armed {
            return;
        }
        self.timer_armed = true;
        // Next simulated vblank on the refresh grid.
        let elapsed = self.epoch.elapsed();
        let phase = Duration::from_nanos((elapsed.as_nanos() % self.refresh.as_nanos()) as u64);
        let wait = self.refresh - phase;
        let _ = self.lh.insert_source(Timer::from_duration(wait), |_, _, state| {
            render(state);
            TimeoutAction::Drop
        });
    }
}

/// Install the headless backend with one virtual output.
pub fn init(state: &mut Lumen, size: Size<i32, Physical>, scale: f64) -> Result<(), String> {
    let data = HeadlessData::new(state.lh.clone(), size, scale)?;
    super::update_shm_formats(&mut state.shm_state, &data.renderer);
    data.output.create_global::<Lumen>(&state.dh);
    state.outputs.push(data.output.clone());
    state.pointer_pos = (size.w as f64 / scale / 2.0, size.h as f64 / scale / 2.0).into();
    state.backend = Backend::Headless(data);
    state.queue_redraw_all();
    Ok(())
}

fn render(state: &mut Lumen) {
    let Backend::Headless(d) = &mut state.backend else { return };
    d.timer_armed = false;
    if !d.redraw_queued {
        return;
    }
    d.redraw_queued = false;
    let output = d.output.clone();
    let refresh = d.refresh;
    let target_time = state.now() + refresh;
    state.advance_animations(target_time);

    let start = Instant::now();
    // Build the scene and render it into the offscreen target with damage
    // tracking (buffer age 1: the target keeps last frame's pixels).
    let result = {
        let (backend, mut ctx) = state.split_for_render();
        let Backend::Headless(d) = backend else { return };
        let (elements, clear) = crate::render::build_scene(&mut d.renderer, &mut ctx, &output);
        d.frames += 1;
        match d.renderer.bind(&mut d.target) {
            Ok(mut fb) => d
                .damage_tracker
                .render_output(&mut d.renderer, &mut fb, 1, &elements, clear)
                .map(|r| r.damage.is_some_and(|d| !d.is_empty()))
                .map_err(|e| format!("{e:?}")),
            Err(e) => Err(format!("{e:?}")),
        }
    };
    let render_ms = start.elapsed().as_secs_f32() * 1000.0;
    match result {
        Ok(damaged) => {
            if damaged {
                state.stats.push_render(render_ms);
            }
        }
        Err(e) => tracing::warn!("headless render failed: {e}"),
    }
    let now = state.now();
    let animating = state.animating();
    state.stats.push_present(now, refresh, animating);
    let feedback = state.presentation_feedback(&output);
    crate::backend::present(feedback, state.clock.now(), refresh, 0, false, false);
    state.post_repaint(&output, now);
    state.notify_ready();
    if animating {
        state.backend.queue_redraw(&output);
    }
}

use super::Backend;
