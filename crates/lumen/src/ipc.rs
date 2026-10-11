//! Control socket: introspection, screenshots and synthetic input.
//!
//! Line protocol over a Unix stream socket at `$LUMEN_IPC` (default
//! `$XDG_RUNTIME_DIR/lumen-ipc.sock`): every request line gets exactly one
//! response line, either JSON or `ok ...` / `error ...`. Used by `lumenctl`,
//! the automated tests and the screenshot shortcut.
//!
//! Requests:
//!   ping | version | socket | windows | outputs | cursor | stats | stats-reset
//!   screenshot <path>
//!   pointer <x> <y>          absolute, logical coordinates
//!   button <code> press|release
//!   scroll <dy>
//!   key <evdev-code> press|release
//!   action <overview|apps|workspace N|switcher-next|switcher-commit|close|maximize|restore|hud|quit>
//!   spawn <program> [args...]
//!   notify <title>|<body>

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

use smithay::backend::allocator::Fourcc;
use smithay::backend::input::{ButtonState, KeyState};
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::gles::GlesTexture;
use smithay::backend::renderer::{Bind, ExportMem, Offscreen};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{Interest, Mode, PostAction};
use smithay::utils::{Rectangle, Transform};

use crate::backend::Backend;
use crate::input::Action;
use crate::state::Lumen;
use crate::wm::Layout;

pub fn socket_path() -> PathBuf {
    if let Some(p) = std::env::var_os("LUMEN_IPC") {
        return p.into();
    }
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    dir.join("lumen-ipc.sock")
}

pub fn init(state: &mut Lumen) -> Result<PathBuf, String> {
    let path = socket_path();
    // A stale socket from a crashed instance would make bind fail.
    if path.exists() && UnixStream::connect(&path).is_err() {
        let _ = std::fs::remove_file(&path);
    }
    let listener = UnixListener::bind(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    state
        .lh
        .insert_source(Generic::new(listener, Interest::READ, Mode::Level), |_, listener, state| {
            loop {
                match listener.accept() {
                    Ok((stream, _)) => state.ipc_add_client(stream),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(e) => {
                        tracing::warn!("ipc accept: {e}");
                        break;
                    }
                }
            }
            Ok(PostAction::Continue)
        })
        .map_err(|e| e.to_string())?;
    Ok(path)
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn percentile(v: &[f32], p: f64) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.total_cmp(b));
    s[((s.len() - 1) as f64 * p).round() as usize]
}

impl Lumen {
    fn ipc_add_client(&mut self, stream: UnixStream) {
        if stream.set_nonblocking(true).is_err() {
            return;
        }
        let mut pending: Vec<u8> = Vec::new();
        let res = self.lh.insert_source(Generic::new(stream, Interest::READ, Mode::Level), move |_, stream, state| {
            // SAFETY: the stream is not dropped while registered.
            let stream = unsafe { stream.get_mut() };
            let mut buf = [0u8; 4096];
            let eof = loop {
                match stream.read(&mut buf) {
                    Ok(0) => break true,
                    Ok(n) => pending.extend_from_slice(&buf[..n]),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break false,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => return Ok(PostAction::Remove),
                }
                if pending.len() > 1 << 20 {
                    return Ok(PostAction::Remove);
                }
            };
            while let Some(nl) = pending.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = pending.drain(..=nl).collect();
                let line = String::from_utf8_lossy(&line);
                let reply = state.ipc_request(line.trim());
                if write_reply(stream, &reply).is_err() {
                    return Ok(PostAction::Remove);
                }
            }
            if eof {
                // A last request without a trailing newline still gets an answer.
                if !pending.is_empty() {
                    let line = String::from_utf8_lossy(&pending).into_owned();
                    pending.clear();
                    let reply = state.ipc_request(line.trim());
                    let _ = write_reply(stream, &reply);
                }
                return Ok(PostAction::Remove);
            }
            Ok(PostAction::Continue)
        });
        if let Err(e) = res {
            tracing::warn!("ipc client: {e}");
        }
    }

    pub fn ipc_request(&mut self, line: &str) -> String {
        let mut parts = line.split_whitespace();
        let cmd = parts.next().unwrap_or("");
        let args: Vec<&str> = parts.collect();
        let time = self.now().as_millis() as u32;
        let reply = match (cmd, args.as_slice()) {
            ("ping", []) => "pong".to_string(),
            ("version", []) => format!("lumen {}", env!("CARGO_PKG_VERSION")),
            ("windows", []) => self.ipc_windows(),
            ("outputs", []) => self.ipc_outputs(),
            ("cursor", []) => {
                use smithay::input::pointer::CursorImageStatus;
                let shape = match &self.cursor_status {
                    CursorImageStatus::Hidden => "hidden".to_string(),
                    CursorImageStatus::Named(icon) => icon.name().to_string(),
                    CursorImageStatus::Surface(_) => "surface".to_string(),
                };
                format!("{:.1} {:.1} {shape}", self.pointer_pos.x, self.pointer_pos.y)
            }
            ("socket", []) => self.socket_name.to_string_lossy().into_owned(),
            ("stats", []) => self.ipc_stats(),
            ("stats-reset", []) => {
                self.stats = Default::default();
                "ok".into()
            }
            ("screenshot", [path]) => match self.capture() {
                Ok((w, h, px)) => match crate::png::write_file(std::path::Path::new(path), w, h, &px) {
                    Ok(()) => format!("ok {path} {w}x{h}"),
                    Err(e) => format!("error {e}"),
                },
                Err(e) => format!("error {e}"),
            },
            ("pointer", [x, y]) => match (x.parse::<f64>(), y.parse::<f64>()) {
                (Ok(x), Ok(y)) => {
                    self.on_pointer_moved((x, y).into(), time, None);
                    "ok".into()
                }
                _ => "error bad coordinates".into(),
            },
            ("button", [code, st]) => match (code.parse::<u32>(), *st) {
                (Ok(code), "press" | "release") => {
                    let st = if *st == "press" { ButtonState::Pressed } else { ButtonState::Released };
                    self.handle_button(code, st, time);
                    "ok".into()
                }
                _ => "error usage: button <code> press|release".into(),
            },
            ("scroll", [dy]) => match dy.parse::<f64>() {
                Ok(dy) => {
                    self.synthetic_scroll(dy, time);
                    "ok".into()
                }
                Err(_) => "error bad amount".into(),
            },
            ("key", [code, st]) => match (code.parse::<u32>(), *st) {
                (Ok(code), "press" | "release") => {
                    let st = if *st == "press" { KeyState::Pressed } else { KeyState::Released };
                    self.handle_key((code + 8).into(), st, time);
                    "ok".into()
                }
                _ => "error usage: key <evdev-code> press|release".into(),
            },
            ("action", a) => match self.ipc_action(a) {
                Some(action) => {
                    self.run_action(action);
                    "ok".into()
                }
                None => "error unknown action".into(),
            },
            ("spawn", prog) if !prog.is_empty() => {
                let cmd: Vec<String> = prog.iter().map(|s| s.to_string()).collect();
                self.spawn(&cmd);
                "ok".into()
            }
            ("notify", _) => {
                let rest = line.splitn(2, ' ').nth(1).unwrap_or("");
                let (title, body) = rest.split_once('|').unwrap_or((rest, ""));
                let now = self.now();
                self.shell.notify(&self.theme, title.trim(), body.trim(), now);
                "ok".into()
            }
            _ => format!("error unknown request: {line}"),
        };
        self.queue_redraw_all();
        reply
    }

    fn ipc_action(&self, a: &[&str]) -> Option<Action> {
        Some(match a {
            ["overview"] => Action::ToggleOverview,
            ["apps"] => Action::ShowApps,
            ["workspace", n] => Action::Workspace(n.parse::<usize>().ok()?),
            ["switcher-next"] => Action::SwitcherNext { reverse: false },
            ["switcher-prev"] => Action::SwitcherNext { reverse: true },
            ["close"] => Action::CloseWindow,
            ["maximize"] => Action::Maximize,
            ["restore"] => Action::Restore,
            ["tile-left"] => Action::Tile(crate::wm::Tile::Left),
            ["tile-right"] => Action::Tile(crate::wm::Tile::Right),
            ["hud"] => Action::ToggleHud,
            ["quit"] => Action::Quit,
            _ => return None,
        })
    }

    fn ipc_windows(&self) -> String {
        let items: Vec<String> = self
            .wm
            .windows
            .iter()
            .map(|w| {
                let g = w.target_geo();
                let layout = match w.layout {
                    Layout::Floating => "floating",
                    Layout::Maximized { .. } => "maximized",
                    Layout::Tiled { side: crate::wm::Tile::Left, .. } => "tiled-left",
                    Layout::Tiled { .. } => "tiled-right",
                    Layout::Fullscreen { .. } => "fullscreen",
                };
                format!(
                    "{{\"id\":{},\"title\":{},\"app_id\":{},\"x\":{},\"y\":{},\"w\":{},\"h\":{},\"ws\":{},\"mapped\":{},\"focused\":{},\"layout\":\"{}\",\"animating\":{}}}",
                    w.id,
                    json_str(&w.title()),
                    json_str(&w.app_id()),
                    g.loc.x,
                    g.loc.y,
                    g.size.w,
                    g.size.h,
                    w.ws,
                    w.mapped,
                    self.wm.focused == Some(w.id),
                    layout,
                    !w.pos.is_settled() || !w.open.is_settled(),
                )
            })
            .collect();
        format!("[{}]", items.join(","))
    }

    fn ipc_outputs(&self) -> String {
        let items: Vec<String> = self
            .outputs
            .iter()
            .map(|o| {
                let mode = o.current_mode();
                format!(
                    "{{\"name\":{},\"width\":{},\"height\":{},\"refresh_mhz\":{},\"scale\":{}}}",
                    json_str(&o.name()),
                    mode.map(|m| m.size.w).unwrap_or(0),
                    mode.map(|m| m.size.h).unwrap_or(0),
                    mode.map(|m| m.refresh).unwrap_or(0),
                    o.current_scale().fractional_scale()
                )
            })
            .collect();
        format!("[{}]", items.join(","))
    }

    fn ipc_stats(&self) -> String {
        let s = &self.stats;
        let avg = |v: &[f32]| if v.is_empty() { 0.0 } else { v.iter().sum::<f32>() / v.len() as f32 };
        format!(
            "{{\"backend\":\"{}\",\"frames\":{},\"render_ms_avg\":{:.3},\"render_ms_p99\":{:.3},\"render_ms_max\":{:.3},\"interval_ms_avg\":{:.3},\"interval_ms_max\":{:.3},\"interval_samples\":{},\"missed\":{},\"animating\":{},\"overview\":{},\"workspace\":{}}}",
            self.backend.name(),
            s.frames,
            avg(&s.render_ms),
            percentile(&s.render_ms, 0.99),
            s.render_ms.iter().copied().fold(0.0, f32::max),
            avg(&s.interval_ms),
            s.interval_ms.iter().copied().fold(0.0, f32::max),
            s.interval_ms.len(),
            s.missed,
            self.animating(),
            self.shell.overview_open,
            self.wm.current,
        )
    }

    fn synthetic_scroll(&mut self, dy: f64, time: u32) {
        use smithay::backend::input::{Axis, AxisSource};
        use smithay::input::pointer::AxisFrame;
        if self.shell_scroll(dy) {
            return;
        }
        let frame = AxisFrame::new(time).source(AxisSource::Wheel).value(Axis::Vertical, dy).v120(Axis::Vertical, (dy / 15.0 * 120.0) as i32);
        if let Some(pointer) = self.seat.get_pointer() {
            pointer.axis(self, frame);
            pointer.frame(self);
        }
    }

    /// Render the primary output's current scene into memory: (w, h, RGBA8
    /// straight alpha, top-down rows).
    pub fn capture(&mut self) -> Result<(u32, u32, Vec<u8>), String> {
        let output = self.primary_output().cloned().ok_or("no output")?;
        let size = crate::render::output_physical_size(&output);
        let scale = output.current_scale().fractional_scale();
        let (backend, mut ctx) = self.split_for_render();
        let r = match backend {
            Backend::Udev(d) => d.renderer.as_mut().ok_or("no renderer")?,
            Backend::Winit(d) => d.backend.renderer(),
            Backend::Headless(d) => &mut d.renderer,
            Backend::None => return Err("no backend".into()),
        };
        let (elements, clear) = crate::render::build_scene(r, &mut ctx, &output);
        let buf_size = size.to_logical(1).to_buffer(1, Transform::Normal);
        let mut tex: GlesTexture = r.create_buffer(Fourcc::Abgr8888, buf_size).map_err(|e| format!("{e:?}"))?;
        {
            let mut fb = r.bind(&mut tex).map_err(|e| format!("{e:?}"))?;
            let mut dt = OutputDamageTracker::new(size, scale, Transform::Normal);
            dt.render_output(r, &mut fb, 0, &elements, clear).map_err(|e| format!("{e:?}"))?;
        }
        let mapping = r.copy_texture(&tex, Rectangle::from_size(buf_size), Fourcc::Abgr8888).map_err(|e| format!("{e:?}"))?;
        let data = r.map_texture(&mapping).map_err(|e| format!("{e:?}"))?;
        let (w, h) = (size.w as u32, size.h as u32);
        let mut px = data.to_vec();
        // The scene is opaque; force alpha so viewers show it as on screen.
        for p in px.chunks_exact_mut(4) {
            p[3] = 255;
        }
        Ok((w, h, px))
    }

    /// Print key: save a screenshot to ~/Pictures (or $XDG_PICTURES_DIR).
    pub fn screenshot_to_file(&mut self) {
        let dir = std::env::var_os("XDG_PICTURES_DIR")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Pictures")))
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let _ = std::fs::create_dir_all(&dir);
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let path = dir.join(format!("screenshot-{secs}.png"));
        let now = self.now();
        let result = self.capture().and_then(|(w, h, px)| crate::png::write_file(&path, w, h, &px).map_err(|e| e.to_string()));
        match result {
            Ok(()) => {
                tracing::info!("screenshot saved to {}", path.display());
                self.shell.notify(&self.theme, "Screenshot saved", &path.display().to_string(), now);
            }
            Err(e) => {
                tracing::warn!("screenshot failed: {e}");
                self.shell.notify(&self.theme, "Screenshot failed", &e, now);
            }
        }
    }
}

fn write_reply(stream: &mut UnixStream, reply: &str) -> std::io::Result<()> {
    // Replies are small; switch to blocking for the write so it is never cut short.
    stream.set_nonblocking(false)?;
    let r = stream.write_all(reply.as_bytes()).and_then(|_| stream.write_all(b"\n"));
    stream.set_nonblocking(true)?;
    r
}
