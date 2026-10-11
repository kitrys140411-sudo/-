//! DRM/KMS backend on a real seat (libseat), with libinput for input.
//!
//! Frame scheduling per output:
//!   Idle --queue_redraw--> Queued --render--> WaitingForVBlank --vblank-->
//!   (redraw needed or animating ? render after an adaptive delay : Idle)
//! The delay after vblank leaves clients time to submit their next buffer while
//! still finishing well before the following vblank (input latency < 1 frame).

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use smithay::backend::allocator::gbm::{GbmAllocator, GbmBufferFlags, GbmDevice};
use smithay::backend::allocator::Fourcc;
use smithay::backend::drm::compositor::{FrameFlags, PrimaryPlaneElement};
use smithay::backend::drm::exporter::gbm::GbmFramebufferExporter;
use smithay::backend::drm::output::{DrmOutput, DrmOutputManager, DrmOutputRenderElements};
use smithay::backend::drm::{DrmDevice, DrmDeviceFd, DrmEvent, DrmEventMetadata, DrmEventTime, DrmNode, NodeType};
use smithay::backend::egl::context::ContextPriority;
use smithay::backend::egl::{EGLContext, EGLDevice, EGLDisplay};
use smithay::backend::libinput::{LibinputInputBackend, LibinputSessionInterface};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::ImportDma;
use smithay::backend::session::libseat::LibSeatSession;
use smithay::backend::session::{Event as SessionEvent, Session};
use smithay::backend::udev::{self, UdevBackend, UdevEvent};
use smithay::desktop::utils::OutputPresentationFeedback;
use smithay::output::{Mode, Output, PhysicalProperties, Scale, Subpixel};
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{LoopHandle, RegistrationToken};
use smithay::reexports::drm::control::{connector, crtc, Device as ControlDevice, ModeTypeFlags};
use smithay::reexports::input::Libinput;
use smithay::reexports::rustix::fs::OFlags;
use smithay::utils::{DeviceFd, Transform};
use smithay::wayland::dmabuf::DmabufFeedbackBuilder;

use super::Backend;
use crate::render::LumenElement;
use crate::state::Lumen;

type Allocator = GbmAllocator<DrmDeviceFd>;
type Exporter = GbmFramebufferExporter<DrmDeviceFd>;
type Feedback = Option<OutputPresentationFeedback>;
type Out = DrmOutput<Allocator, Exporter, Feedback, DrmDeviceFd>;

#[derive(Debug, PartialEq)]
enum Redraw {
    Idle,
    Queued,
    WaitingForVBlank { redraw_needed: bool },
    /// Nothing was damaged; a timer stands in for the vblank.
    WaitingForEstimatedVBlank { token: RegistrationToken, redraw_needed: bool },
}

struct Surface {
    output: Output,
    out: Out,
    redraw: Redraw,
    refresh: Duration,
    last_render: Duration,
    frame_seq: u64,
    /// Time of the last vblank (presentation clock), for predicting the next ones.
    last_vblank: Option<Duration>,
}

impl Surface {
    /// When a frame rendered now will reach the screen: the first vblank on
    /// the refresh grid that is still ahead. Animations are stepped to this
    /// time, so motion advances by exactly one refresh period per frame.
    fn predicted_presentation(&self, now: Duration) -> Duration {
        let Some(v) = self.last_vblank else { return now + self.refresh };
        let refresh = self.refresh.max(Duration::from_millis(1));
        let mut t = v + refresh;
        if t < now + Duration::from_millis(1) {
            let behind = (now + Duration::from_millis(1)).saturating_sub(t);
            let periods = behind.as_nanos().div_ceil(refresh.as_nanos()) as u32;
            t += refresh * periods;
        }
        t
    }
}

struct Device {
    manager: DrmOutputManager<Allocator, Exporter, Feedback, DrmDeviceFd>,
    surfaces: HashMap<crtc::Handle, Surface>,
    _token: RegistrationToken,
}

pub struct UdevData {
    pub session: LibSeatSession,
    pub renderer: Option<GlesRenderer>,
    device: Option<Device>,
    node: DrmNode,
    lh: LoopHandle<'static, Lumen>,
}

impl UdevData {
    pub fn queue_redraw(&mut self, output: &Output) {
        let Some(dev) = self.device.as_mut() else { return };
        let Some((crtc, s)) = dev.surfaces.iter_mut().find(|(_, s)| &s.output == output) else { return };
        match &mut s.redraw {
            Redraw::Idle => {
                s.redraw = Redraw::Queued;
                let crtc = *crtc;
                self.lh.insert_idle(move |state| render_surface(state, crtc));
            }
            Redraw::WaitingForVBlank { redraw_needed } | Redraw::WaitingForEstimatedVBlank { redraw_needed, .. } => {
                *redraw_needed = true
            }
            Redraw::Queued => {}
        }
    }

    pub fn change_vt(&mut self, vt: i32) {
        if let Err(e) = self.session.change_vt(vt) {
            tracing::warn!("cannot switch to VT {vt}: {e}");
        }
    }
}

fn pick_scale(info: &connector::Info, mode: &smithay::reexports::drm::control::Mode) -> f64 {
    if let Some(s) = std::env::var("LUMEN_SCALE").ok().and_then(|v| v.parse::<f64>().ok()) {
        return s.clamp(0.5, 4.0);
    }
    let (w_mm, _) = info.size().unwrap_or((0, 0));
    if w_mm == 0 {
        return 1.0;
    }
    let dpi = mode.size().0 as f64 / (w_mm as f64 / 25.4);
    if dpi >= 200.0 {
        2.0
    } else if dpi >= 160.0 {
        1.5
    } else if dpi >= 135.0 {
        1.25
    } else {
        1.0
    }
}

pub fn init(state: &mut Lumen, session: LibSeatSession, notifier: smithay::backend::session::libseat::LibSeatSessionNotifier) -> Result<(), String> {
    let lh = state.lh.clone();
    let seat = session.seat();
    let path = udev::primary_gpu(&seat)
        .ok()
        .flatten()
        .or_else(|| udev::all_gpus(&seat).ok()?.into_iter().next())
        .ok_or("no GPU found")?;
    let node = DrmNode::from_path(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    tracing::info!("using GPU {}", path.display());
    state.backend = Backend::Udev(UdevData { session, renderer: None, device: None, node, lh: lh.clone() });
    add_device(state, &path, node)?;

    // Hotplug of connectors (device changed events).
    let udev_backend = UdevBackend::new(&seat).map_err(|e| format!("udev: {e}"))?;
    lh.insert_source(udev_backend, move |event, _, state| match event {
        UdevEvent::Changed { device_id } => {
            if let Backend::Udev(d) = &state.backend {
                if d.node.dev_id() == device_id {
                    scan_connectors(state);
                }
            }
        }
        UdevEvent::Added { .. } | UdevEvent::Removed { .. } => {}
    })
    .map_err(|e| format!("udev source: {e}"))?;

    // Input.
    let Backend::Udev(d) = &state.backend else { unreachable!() };
    let mut libinput = Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(d.session.clone().into());
    libinput.udev_assign_seat(&seat).map_err(|_| "libinput: cannot assign seat")?;
    let input_backend = LibinputInputBackend::new(libinput.clone());
    lh.insert_source(input_backend, |event, _, state| state.process_input_event(event))
        .map_err(|e| format!("libinput source: {e}"))?;

    // Session (VT switch) events.
    lh.insert_source(notifier, move |event, _, state| match event {
        SessionEvent::PauseSession => {
            tracing::info!("session paused");
            libinput.suspend();
            if let Backend::Udev(d) = &mut state.backend {
                if let Some(dev) = d.device.as_mut() {
                    dev.manager.pause();
                }
            }
        }
        SessionEvent::ActivateSession => {
            tracing::info!("session resumed");
            if libinput.resume().is_err() {
                tracing::warn!("libinput resume failed");
            }
            if let Backend::Udev(d) = &mut state.backend {
                if let Some(dev) = d.device.as_mut() {
                    if let Err(e) = dev.manager.activate(false) {
                        tracing::error!("cannot reactivate DRM: {e}");
                    }
                    for s in dev.surfaces.values_mut() {
                        s.redraw = Redraw::Idle;
                    }
                }
            }
            state.queue_redraw_all();
        }
    })
    .map_err(|e| format!("session source: {e}"))?;
    Ok(())
}

fn add_device(state: &mut Lumen, path: &Path, node: DrmNode) -> Result<(), String> {
    let Backend::Udev(d) = &mut state.backend else { return Err("not udev".into()) };
    let fd = d
        .session
        .open(path, OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK)
        .map_err(|e| format!("open {}: {e}", path.display()))?;
    let fd = DrmDeviceFd::new(DeviceFd::from(fd));
    let (drm, notifier) = DrmDevice::new(fd.clone(), true).map_err(|e| format!("DRM device: {e}"))?;
    let gbm = GbmDevice::new(fd).map_err(|e| format!("GBM device: {e}"))?;
    let display = unsafe { EGLDisplay::new(gbm.clone()) }.map_err(|e| format!("EGL display: {e}"))?;
    let egl_device = EGLDevice::device_for_display(&display).ok();
    let software = egl_device.as_ref().is_some_and(|d| d.is_software());
    let render_node = egl_device.and_then(|d| d.try_get_render_node().ok().flatten());
    tracing::info!(software, render_node = ?render_node, "EGL initialized");
    let context = EGLContext::new_with_priority(&display, ContextPriority::High).map_err(|e| format!("EGL context: {e}"))?;
    let renderer = unsafe { GlesRenderer::new(context) }.map_err(|e| format!("GLES renderer: {e}"))?;
    let render_formats = renderer.egl_context().dmabuf_render_formats().clone();
    super::update_shm_formats(&mut state.shm_state, &renderer);

    // linux-dmabuf for clients, if there is a render node to share buffers with.
    if let Some(rn) = render_node.or_else(|| node.node_with_type(NodeType::Render).and_then(|n| n.ok())) {
        if !software {
            let formats = renderer.dmabuf_formats();
            match DmabufFeedbackBuilder::new(rn.dev_id(), formats).build() {
                Ok(feedback) => {
                    let global = state.dmabuf_state.create_global_with_default_feedback::<Lumen>(&state.dh, &feedback);
                    state.dmabuf_global = Some(global);
                }
                Err(e) => tracing::warn!("dmabuf feedback: {e}"),
            }
        }
    }

    let allocator = GbmAllocator::new(gbm.clone(), GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT);
    let exporter = GbmFramebufferExporter::new(gbm.clone(), render_node);
    let manager = DrmOutputManager::new(
        drm,
        allocator,
        exporter,
        Some(gbm),
        [Fourcc::Argb8888, Fourcc::Xrgb8888],
        render_formats,
    );
    let token = state
        .lh
        .insert_source(notifier, move |event, metadata, state| match event {
            DrmEvent::VBlank(crtc) => on_vblank(state, crtc, metadata),
            DrmEvent::Error(e) => tracing::error!("DRM error: {e}"),
        })
        .map_err(|e| format!("DRM source: {e}"))?;
    let Backend::Udev(d) = &mut state.backend else { unreachable!() };
    d.renderer = Some(renderer);
    d.device = Some(Device { manager, surfaces: HashMap::new(), _token: token });
    scan_connectors(state);
    Ok(())
}

/// Light up connected connectors (one CRTC each) and drop disconnected ones.
fn scan_connectors(state: &mut Lumen) {
    let Backend::Udev(d) = &mut state.backend else { return };
    let Some(dev) = d.device.as_mut() else { return };
    // Pass 1 (device borrowed immutably): decide what to light up.
    struct Plan {
        conn: connector::Handle,
        crtc: crtc::Handle,
        mode: smithay::reexports::drm::control::Mode,
        name: String,
        size_mm: (u32, u32),
        scale: f64,
    }
    let mut plans = Vec::new();
    let mut connected = Vec::new();
    {
        let drm = dev.manager.device();
        let Ok(res) = drm.resource_handles() else { return };
        let mut used: Vec<crtc::Handle> = dev.surfaces.keys().copied().collect();
        for &conn in res.connectors() {
            let Ok(info) = drm.get_connector(conn, true) else { continue };
            if info.state() != connector::State::Connected || info.modes().is_empty() {
                continue;
            }
            connected.push(conn);
            if dev.surfaces.values().any(|s| s.out.with_compositor(|c| c.surface().current_connectors().into_iter().any(|c| c == conn))) {
                continue;
            }
            // First free CRTC that one of the connector's encoders can drive.
            let crtc = info.encoders().iter().filter_map(|e| drm.get_encoder(*e).ok()).find_map(|enc| {
                res.filter_crtcs(enc.possible_crtcs()).into_iter().find(|c| !used.contains(c))
            });
            let Some(crtc) = crtc else {
                tracing::warn!("no free CRTC for connector {:?}", conn);
                continue;
            };
            used.push(crtc);
            let mode = *info
                .modes()
                .iter()
                .find(|m| m.mode_type().contains(ModeTypeFlags::PREFERRED))
                .unwrap_or(&info.modes()[0]);
            let name = format!("{}-{}", info.interface().as_str(), info.interface_id());
            let scale = pick_scale(&info, &mode);
            plans.push(Plan { conn, crtc, mode, name, size_mm: info.size().unwrap_or((0, 0)), scale });
        }
    }

    // Pass 2: create outputs, placed left to right in connector order.
    for plan in plans {
        let Plan { conn, crtc, mode, name, size_mm, scale } = plan;
        let output = Output::new(
            name.clone(),
            PhysicalProperties {
                size: (size_mm.0 as i32, size_mm.1 as i32).into(),
                subpixel: Subpixel::Unknown,
                make: "Lumen".into(),
                model: name.clone(),
            },
        );
        let wl_mode = Mode::from(mode);
        let x: i32 = state
            .outputs
            .iter()
            .map(|o| {
                let w = o.current_mode().map(|m| m.size.w).unwrap_or(0) as f64;
                (w / o.current_scale().fractional_scale()).round() as i32
            })
            .sum();
        output.set_preferred(wl_mode);
        output.change_current_state(Some(wl_mode), Some(Transform::Normal), Some(Scale::Fractional(scale)), Some((x, 0).into()));
        let Some(renderer) = d.renderer.as_mut() else { return };
        let out = dev.manager.initialize_output::<_, LumenElement>(
            crtc,
            mode,
            &[conn],
            &output,
            None,
            renderer,
            &DrmOutputRenderElements::default(),
        );
        match out {
            Ok(out) => {
                output.create_global::<Lumen>(&state.dh);
                let refresh = Duration::from_secs_f64(1000.0 / wl_mode.refresh.max(1) as f64);
                tracing::info!("output {name}: {}x{}@{:.2} scale {scale}", mode.size().0, mode.size().1, wl_mode.refresh as f64 / 1000.0);
                dev.surfaces.insert(
                    crtc,
                    Surface {
                        output: output.clone(),
                        out,
                        redraw: Redraw::Idle,
                        refresh,
                        last_render: Duration::from_millis(4),
                        frame_seq: 0,
                        last_vblank: None,
                    },
                );
                if state.outputs.is_empty() {
                    state.pointer_pos = (wl_mode.size.w as f64 / scale / 2.0, wl_mode.size.h as f64 / scale / 2.0).into();
                }
                state.outputs.push(output.clone());
            }
            Err(e) => tracing::error!("cannot initialize output {name}: {e:?}"),
        }
    }
    // Remove outputs whose connector went away.
    let Backend::Udev(d) = &mut state.backend else { return };
    let Some(dev) = d.device.as_mut() else { return };
    let gone: Vec<crtc::Handle> = dev
        .surfaces
        .iter()
        .filter(|(_, s)| !s.out.with_compositor(|c| c.surface().current_connectors().into_iter().any(|c| connected.contains(&c))))
        .map(|(c, _)| *c)
        .collect();
    for crtc in gone {
        if let Some(s) = dev.surfaces.remove(&crtc) {
            state.outputs.retain(|o| o != &s.output);
        }
    }
    state.queue_redraw_all();
}

fn render_surface(state: &mut Lumen, crtc: crtc::Handle) {
    let now = state.now();
    let (output, target) = {
        let Backend::Udev(d) = &mut state.backend else { return };
        let Some(s) = d.device.as_mut().and_then(|dev| dev.surfaces.get_mut(&crtc)) else { return };
        if s.redraw != Redraw::Queued {
            return;
        }
        (s.output.clone(), s.predicted_presentation(now))
    };
    state.advance_animations(target);
    let start = Instant::now();
    let result = {
        let (backend, mut ctx) = state.split_for_render();
        let Backend::Udev(d) = backend else { return };
        let Some(renderer) = d.renderer.as_mut() else { return };
        let (elements, clear) = crate::render::build_scene(renderer, &mut ctx, &output);
        let Some(s) = d.device.as_mut().and_then(|dev| dev.surfaces.get_mut(&crtc)) else { return };
        s.out
            .render_frame(renderer, &elements, clear, FrameFlags::DEFAULT)
            .map(|r| {
                // Without an exportable fence (software rendering, some drivers)
                // KMS cannot wait for the GPU: wait here, or a half-rendered
                // buffer would be scanned out.
                if r.needs_sync() {
                    if let PrimaryPlaneElement::Swapchain(element) = &r.primary_element {
                        let _ = element.sync.wait();
                    }
                }
                !r.is_empty
            })
            .map_err(|e| format!("{e:?}"))
    };
    let took = start.elapsed();
    match result {
        Ok(true) => {
            state.stats.push_render(took.as_secs_f32() * 1000.0);
            let feedback = state.presentation_feedback(&output);
            let Backend::Udev(d) = &mut state.backend else { return };
            let Some(s) = d.device.as_mut().and_then(|dev| dev.surfaces.get_mut(&crtc)) else { return };
            s.last_render = took;
            match s.out.queue_frame(Some(feedback)) {
                Ok(()) => s.redraw = Redraw::WaitingForVBlank { redraw_needed: false },
                Err(e) => {
                    tracing::warn!("queue_frame failed: {e:?}");
                    s.redraw = Redraw::Idle;
                }
            }
            state.post_repaint(&output, now);
        }
        Ok(false) => {
            // No damage: wait for an estimated vblank so frame callbacks keep their pace.
            let Backend::Udev(d) = &mut state.backend else { return };
            let Some(s) = d.device.as_mut().and_then(|dev| dev.surfaces.get_mut(&crtc)) else { return };
            let wait = target.saturating_sub(now).max(Duration::from_millis(1));
            let token = d.lh.insert_source(Timer::from_duration(wait), move |_, _, state| {
                on_estimated_vblank(state, crtc);
                TimeoutAction::Drop
            });
            s.redraw = match token {
                Ok(token) => Redraw::WaitingForEstimatedVBlank { token, redraw_needed: false },
                Err(_) => Redraw::Idle,
            };
            state.post_repaint(&output, now);
        }
        Err(e) => {
            tracing::warn!("render failed: {e}");
            let Backend::Udev(d) = &mut state.backend else { return };
            if let Some(s) = d.device.as_mut().and_then(|dev| dev.surfaces.get_mut(&crtc)) {
                s.redraw = Redraw::Idle;
            }
        }
    }
}

fn on_estimated_vblank(state: &mut Lumen, crtc: crtc::Handle) {
    let animating = state.animating();
    let Backend::Udev(d) = &mut state.backend else { return };
    let Some(s) = d.device.as_mut().and_then(|dev| dev.surfaces.get_mut(&crtc)) else { return };
    let redraw_needed = matches!(s.redraw, Redraw::WaitingForEstimatedVBlank { redraw_needed: true, .. });
    s.redraw = Redraw::Idle;
    if redraw_needed || animating {
        let o = s.output.clone();
        d.queue_redraw(&o);
    }
}

fn on_vblank(state: &mut Lumen, crtc: crtc::Handle, metadata: &mut Option<DrmEventMetadata>) {
    let animating = state.animating();
    let now = state.now();
    let Backend::Udev(d) = &mut state.backend else { return };
    let lh = d.lh.clone();
    let Some(s) = d.device.as_mut().and_then(|dev| dev.surfaces.get_mut(&crtc)) else { return };
    let time = metadata
        .as_ref()
        .and_then(|m| match m.time {
            DrmEventTime::Monotonic(t) if !t.is_zero() => Some(t),
            _ => None,
        })
        .unwrap_or(now);
    s.frame_seq = metadata.as_ref().map(|m| m.sequence as u64).unwrap_or(s.frame_seq + 1);
    s.last_vblank = Some(time);
    let refresh = s.refresh;
    match s.out.frame_submitted() {
        Ok(Some(Some(feedback))) => super::present(feedback, time.into(), refresh, s.frame_seq, true),
        Ok(_) => {}
        Err(e) => tracing::warn!("frame_submitted: {e:?}"),
    }
    let redraw_needed = matches!(s.redraw, Redraw::WaitingForVBlank { redraw_needed: true });
    s.redraw = Redraw::Idle;
    // Render late in the frame when rendering is fast (fresher client buffers,
    // lower latency), immediately when it is slow (software rendering).
    let budget = refresh.saturating_sub(s.last_render * 2 + Duration::from_millis(2));
    let delay = budget.min(refresh * 6 / 10);
    state.stats.push_present(time, refresh, animating);
    if redraw_needed || animating {
        let Backend::Udev(d) = &mut state.backend else { return };
        let Some(s) = d.device.as_mut().and_then(|dev| dev.surfaces.get_mut(&crtc)) else { return };
        s.redraw = Redraw::Queued;
        let _ = lh.insert_source(Timer::from_duration(delay), move |_, _, state| {
            render_surface(state, crtc);
            TimeoutAction::Drop
        });
    }
    state.notify_ready();
}
