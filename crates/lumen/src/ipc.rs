//! Control socket: introspection, screenshots and synthetic input.
//!
//! Line protocol over a Unix stream socket at `$LUMEN_IPC` (default
//! `$XDG_RUNTIME_DIR/lumen-ipc.sock`): every request line gets exactly one
//! response line, either JSON or `ok ...` / `error ...`. Used by `lumenctl`,
//! the automated tests and the screenshot shortcut.
//!
//! Requests:
//!   ping | version | socket | windows | outputs | cursor | stats | stats-reset | shell
//!   screenshot <path>
//!   pointer <x> <y>          absolute, logical coordinates
//!   button <code> press|release
//!   scroll <dy>
//!   key <evdev-code> press|release
//!   action <overview|apps|workspace N|switcher-next|switcher-prev|close|maximize|restore|hud|quit
//!           |any keybinding action name: shortcuts, fullscreen, volume-up, workspace-2, ...>
//!   spawn <program> [args...]
//!   notify <title>|<body>
//!   keybindings              the live shortcut table (JSON)
//!   reload-keybindings       re-read the keybinding files
//!   input                    keyboard layouts and pointer settings (JSON)
//!   reload-input             re-read input.toml (layouts, touchpad, mouse)
//!   layout <n>               make keyboard layout n (0-based) active
//!   switch lid closed|open   a laptop lid switch event (as from libinput)
//!   xwayland                 X11 support: display, readiness, cost, override-redirect windows (JSON)
//!   motion <dx> <dy>         relative pointer motion (a mouse, not a tablet)
//!   idle                     idle state: inhibitors, blanking, timeout (JSON)
//!   idle-timeout <seconds>   blank the screen after this long without input (0: never)
//!   scene                    whether the frame could be scanned out without composition (JSON)
//!   startup                  startup timeline, programs deferred past the first frame, glass panes (JSON)

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
                    _ if self.pointer_locked() => "hidden".to_string(),
                    CursorImageStatus::Hidden => "hidden".to_string(),
                    CursorImageStatus::Named(icon) => icon.name().to_string(),
                    CursorImageStatus::Surface(_) => "surface".to_string(),
                };
                format!("{:.1} {:.1} {shape}", self.pointer_pos.x, self.pointer_pos.y)
            }
            ("socket", []) => self.socket_name.to_string_lossy().into_owned(),
            ("stats", []) => self.ipc_stats(),
            ("startup", []) => self.ipc_startup(),
            ("shell", []) => self.ipc_shell(),
            ("keybindings", []) => self.ipc_keybindings(),
            ("reload-keybindings", []) => match self.reload_keybindings() {
                Ok(n) => format!("ok {n} shortcuts"),
                Err(e) => format!("error {}", e.replace('\n', "; ")),
            },
            ("input", []) => self.ipc_input(),
            ("xwayland", []) => self.ipc_xwayland(),
            ("idle", []) => self.ipc_idle(),
            ("idle-timeout", [secs]) => match secs.parse::<f64>() {
                Ok(v) if v >= 0.0 => {
                    self.set_idle_timeout((v > 0.0).then(|| std::time::Duration::from_secs_f64(v)));
                    "ok".into()
                }
                _ => "error bad timeout".into(),
            },
            ("motion", [dx, dy]) => match (dx.parse::<f64>(), dy.parse::<f64>()) {
                (Ok(dx), Ok(dy)) => {
                    let d = smithay::utils::Point::from((dx, dy));
                    let utime = self.now().as_micros() as u64;
                    self.on_pointer_moved(self.pointer_pos + d, time, Some((d, d, utime)));
                    "ok".into()
                }
                _ => "error bad motion".into(),
            },
            ("scene", []) => match self.ipc_scene() {
                Ok(s) => s,
                Err(e) => format!("error {e}"),
            },
            ("reload-input", []) => match self.reload_input() {
                Ok(summary) => format!("ok {summary}"),
                Err(e) => format!("error {}", e.replace('\n', "; ")),
            },
            ("layout", [n]) => match n.parse::<u32>() {
                Ok(n) if n < self.layouts.count() => {
                    self.set_layout(n);
                    "ok".into()
                }
                _ => format!("error no layout {n} (there are {})", self.layouts.count()),
            },
            ("switch", ["lid", st @ ("closed" | "open")]) => {
                self.on_lid(*st == "closed");
                "ok".into()
            }
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
                self.notify(title.trim(), body.trim());
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
            [name] => crate::keybind::parse_action(name, None).ok()??,
            _ => return None,
        })
    }

    /// One JSON object per binding: chord, action, command, label, group.
    fn bindings_json(&self) -> String {
        let items: Vec<String> = self
            .keys
            .bindings
            .iter()
            .map(|b| {
                let command = match &b.action {
                    Action::Spawn(cmd) => format!("[{}]", cmd.iter().map(|w| json_str(w)).collect::<Vec<_>>().join(",")),
                    _ => "null".into(),
                };
                format!(
                    "{{\"keys\":{},\"action\":{},\"command\":{},\"label\":{},\"group\":{}}}",
                    json_str(&b.chord.spec()),
                    json_str(&crate::keybind::action_name(&b.action)),
                    command,
                    json_str(&b.label),
                    json_str(&b.group)
                )
            })
            .collect();
        format!("[{}]", items.join(","))
    }

    fn ipc_keybindings(&self) -> String {
        self.bindings_json()
    }

    fn ipc_input(&self) -> String {
        let list = |v: &[String]| v.iter().map(|s| json_str(s)).collect::<Vec<_>>().join(",");
        let c = &self.input_cfg;
        let k = &c.keyboard;
        format!(
            "{{\"layouts\":[{}],\"labels\":[{}],\"active\":{},\"indicator\":{},\"xkb\":{{\"layout\":{},\"variant\":{},\"options\":{},\"model\":{}}},\"per_window\":{},\"repeat_delay\":{},\"repeat_rate\":{},\"touchpad\":{{\"tap_to_click\":{},\"natural_scroll\":{},\"speed\":{},\"disable_while_typing\":{},\"click_method\":{}}},\"mouse\":{{\"natural_scroll\":{},\"speed\":{},\"accel_profile\":{}}}}}",
            list(&self.layouts.names),
            list(&self.layouts.labels),
            self.layouts.active,
            self.layouts.indicator().map(json_str).unwrap_or_else(|| "null".into()),
            json_str(&k.xkb_layout()),
            json_str(&k.xkb_variant()),
            json_str(&k.xkb_options().unwrap_or_default()),
            json_str(&k.model),
            k.per_window,
            k.repeat_delay,
            k.repeat_rate,
            c.touchpad.tap_to_click,
            c.touchpad.natural_scroll,
            c.touchpad.speed,
            c.touchpad.disable_while_typing,
            json_str(&format!("{:?}", c.touchpad.click_method).to_lowercase()),
            c.mouse.natural_scroll,
            c.mouse.speed,
            json_str(&format!("{:?}", c.mouse.accel_profile).to_lowercase()),
        )
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
                    "{{\"id\":{},\"title\":{},\"app_id\":{},\"x\":{},\"y\":{},\"w\":{},\"h\":{},\"ws\":{},\"mapped\":{},\"focused\":{},\"layout\":\"{}\",\"animating\":{},\"x11\":{},\"parent\":{}}}",
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
                    !w.pos.is_settled() || !w.open.is_settled() || w.morph.is_some() || w.pending_morph.is_some(),
                    w.x11().is_some(),
                    self.wm.parent_of(w.id).map(|p| p.to_string()).unwrap_or_else(|| "null".into()),
                )
            })
            .collect();
        format!("[{}]", items.join(","))
    }

    /// Shell state for tests: overview, search, dock, menus, notifications
    /// and the clickable regions of the last frame.
    fn ipc_shell(&self) -> String {
        let sh = &self.shell;
        let ids = |v: &[usize]| v.iter().filter_map(|&i| sh.apps.get(i)).map(|a| json_str(&a.id)).collect::<Vec<_>>().join(",");
        let notes: Vec<String> = sh
            .notifications
            .iter()
            .map(|n| format!("{{\"id\":{},\"title\":{},\"body\":{}}}", n.id, json_str(&n.title), json_str(&n.body)))
            .collect();
        let dock: Vec<String> = sh
            .dock_items(&self.wm)
            .iter()
            .map(|d| {
                format!(
                    "{{\"id\":{},\"name\":{},\"icon\":{},\"windows\":{}}}",
                    json_str(&d.id),
                    json_str(&d.name),
                    json_str(&d.icon),
                    d.windows.len()
                )
            })
            .collect();
        let hits: Vec<String> = sh
            .hits
            .iter()
            .map(|(r, h)| {
                format!(
                    "{{\"hit\":{},\"x\":{:.1},\"y\":{:.1},\"w\":{:.1},\"h\":{:.1}}}",
                    json_str(&format!("{h:?}")),
                    r.loc.x,
                    r.loc.y,
                    r.size.w,
                    r.size.h
                )
            })
            .collect();
        let sheet = sh.sheet.as_ref();
        let sheet_rows: Vec<String> = sheet
            .map(|s| {
                s.visible
                    .iter()
                    .filter_map(|&i| s.rows.get(i))
                    .map(|r| {
                        let chords: Vec<String> = r.chords.iter().map(|c| json_str(&c.join("+"))).collect();
                        format!("{{\"label\":{},\"group\":{},\"keys\":[{}]}}", json_str(&r.label), json_str(&r.group), chords.join(","))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let selected_row = sheet.and_then(|s| s.selected_row()).and_then(|i| sheet.and_then(|s| s.rows.get(i))).map(|r| json_str(&r.label));
        let osd = sh
            .osd
            .as_ref()
            .filter(|o| o.anim.target > 0.0)
            .map(|o| {
                format!("{{\"kind\":\"{}\",\"level\":{:.3},\"muted\":{},\"icon\":{}}}", o.kind.name(), o.level, o.muted, json_str(o.icon()))
            })
            .unwrap_or_else(|| "null".into());
        let battery = sh
            .battery
            .map(|b| {
                format!(
                    "{{\"percent\":{:.1},\"charge\":\"{:?}\",\"on_ac\":{},\"time_left_s\":{}}}",
                    b.percent,
                    b.charge,
                    b.on_ac,
                    b.time_left.map(|t| t.as_secs().to_string()).unwrap_or_else(|| "null".into())
                )
            })
            .unwrap_or_else(|| "null".into());
        let layout = sh.layout_label.as_deref().map(json_str).unwrap_or_else(|| "null".into());
        format!(
            "{{\"overview\":{},\"apps_mode\":{},\"query\":{},\"results\":[{}],\"visible_apps\":[{}],\"selected\":{},\"popover\":{},\"switcher\":{},\"dock_shown\":{:.3},\"notifications\":[{}],\"dock\":[{}],\"hits\":[{}],\"shortcuts\":{},\"shortcuts_shown\":{:.3},\"shortcuts_query\":{},\"shortcuts_rows\":[{}],\"shortcuts_selected\":{},\"shortcuts_scroll\":{:.1},\"shortcuts_max_scroll\":{:.1},\"osd\":{},\"layout\":{},\"battery\":{},\"lid_closed\":{},\"icons_pending\":{},\"bindings\":{}}}",
            sh.overview_open,
            sh.apps_mode,
            json_str(&sh.query),
            ids(&sh.results),
            ids(&sh.visible_apps()),
            sh.selected,
            sh.popover.as_ref().is_some_and(|p| p.open),
            sh.switcher.is_some(),
            sh.dock.value,
            notes.join(","),
            dock.join(","),
            hits.join(","),
            sheet.is_some_and(|s| s.open),
            sheet.map(|s| s.anim.value).unwrap_or(0.0),
            json_str(sheet.map(|s| s.query.as_str()).unwrap_or("")),
            sheet_rows.join(","),
            selected_row.unwrap_or_else(|| "null".into()),
            sheet.map(|s| s.scroll.value).unwrap_or(0.0),
            sheet.map(|s| s.max_scroll()).unwrap_or(0.0),
            osd,
            layout,
            battery,
            self.lid_closed,
            self.render.icons.pending(),
            self.bindings_json(),
        )
    }

    fn ipc_xwayland(&self) -> String {
        let x = &self.xwayland;
        let opt = |v: Option<String>| v.unwrap_or_else(|| "null".into());
        let overrides: Vec<String> = self
            .wm
            .overrides
            .iter()
            .map(|o| {
                let g = o.x.geometry();
                format!(
                    "{{\"title\":{},\"class\":{},\"x\":{},\"y\":{},\"w\":{},\"h\":{},\"has_surface\":{},\"ws\":{}}}",
                    json_str(&o.x.title()),
                    json_str(&o.x.class()),
                    g.loc.x,
                    g.loc.y,
                    g.size.w,
                    g.size.h,
                    o.x.wl_surface().is_some(),
                    o.ws
                )
            })
            .collect();
        format!(
            "{{\"enabled\":{},\"ready\":{},\"display\":{},\"pid\":{},\"ready_ms\":{},\"rss_kib\":{},\"pss_kib\":{},\"private_kib\":{},\"failures\":{},\"error\":{},\"x11_focused\":{},\"keyboard_grab\":{},\"scale_mode\":\"{}\",\"client_scale\":{},\"overrides\":[{}]}}",
            x.enabled,
            x.ready,
            opt(x.display_env().map(|d| json_str(&d))),
            opt(x.pid.map(|p| p.to_string())),
            opt(x.ready_after.map(|d| d.as_millis().to_string())),
            opt(x.rss_kib().map(|k| k.to_string())),
            opt(x.pss_private_kib().map(|m| m.0.to_string())),
            opt(x.pss_private_kib().map(|m| m.1.to_string())),
            x.failures,
            opt(x.error.as_deref().map(json_str)),
            self.x11_focused(),
            match &x.keyboard_grab {
                Some(g) => format!("{{\"active\":{}}}", g.active),
                None => "null".into(),
            },
            x.scale_mode.name(),
            x.client_scale,
            overrides.join(","),
        )
    }

    fn ipc_idle(&self) -> String {
        let i = &self.idle;
        let constraint = match self.active_constraint() {
            Some((_, _, crate::gaming::Constraint::Locked)) => "\"locked\"",
            Some((_, _, crate::gaming::Constraint::Confined(_))) => "\"confined\"",
            None => "null",
        };
        format!(
            "{{\"idle_ms\":{},\"timeout_s\":{},\"blanked\":{},\"inhibited\":{},\"inhibitors\":{},\"shortcuts_inhibited\":{},\"pointer_constraint\":{constraint}}}",
            self.now().saturating_sub(i.last_input).as_millis(),
            i.timeout.map(|t| format!("{:.3}", t.as_secs_f64())).unwrap_or_else(|| "null".into()),
            i.blanked,
            self.idle_inhibited(),
            i.inhibitors.iter().filter(|s| smithay::utils::IsAlive::alive(*s)).count(),
            self.shortcuts_inhibited(),
        )
    }

    fn ipc_scene(&mut self) -> Result<String, String> {
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
        let (elements, _) = crate::render::build_scene(r, &mut ctx, &output);
        Ok(crate::render::scanout_check(r, &elements, size, scale))
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

    /// The startup timeline (see crate::startup) and what the renderer
    /// deferred past the first frame.
    fn ipc_startup(&self) -> String {
        let steps: Vec<String> = crate::startup::steps()
            .iter()
            .map(|(n, at, cpu)| format!("{{\"step\":{},\"at_ms\":{at},\"cpu_ms\":{cpu}}}", json_str(n)))
            .collect();
        let names = |v: &[&str]| v.iter().map(|n| json_str(n)).collect::<Vec<_>>().join(",");
        let rs = &self.render;
        let compiled = rs.shaders.as_ref().map(|s| s.compiled_lazy()).unwrap_or_default();
        let (blurred, sampled) = rs.glass.counts();
        format!(
            "{{\"steps\":[{}],\"lazy_at_first_frame\":{},\"lazy_compiled\":[{}],\"warmed_up\":{},\"glass_blurred\":{blurred},\"glass_wallpaper\":{sampled}}}",
            steps.join(","),
            rs.lazy_at_first_frame.as_ref().map(|v| format!("[{}]", names(v))).unwrap_or_else(|| "null".into()),
            names(&compiled),
            rs.warmed_up,
        )
    }

    fn ipc_stats(&self) -> String {
        let s = &self.stats;
        let avg = |v: &[f32]| if v.is_empty() { 0.0 } else { v.iter().sum::<f32>() / v.len() as f32 };
        format!(
            "{{\"backend\":\"{}\",\"scanout_frames\":{},\"composited_frames\":{},\"frames\":{},\"render_ms_avg\":{:.3},\"render_ms_p99\":{:.3},\"render_ms_max\":{:.3},\"interval_ms_avg\":{:.3},\"interval_ms_max\":{:.3},\"interval_samples\":{},\"missed\":{},\"animating\":{},\"overview\":{},\"workspace\":{}}}",
            self.backend.name(),
            s.scanout_frames,
            s.composited_frames,
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

    /// Print key: save a screenshot as ~/Pictures/Screenshot-YYYY-MM-DD-HHMMSS.png
    /// (or in $XDG_PICTURES_DIR). The frame is captured now; encoding and
    /// writing happen off the main loop, and a notification reports the result.
    pub fn screenshot_to_file(&mut self) {
        let dir = std::env::var_os("XDG_PICTURES_DIR")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Pictures")))
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let stamp = local_timestamp();
        match self.capture() {
            Ok((w, h, px)) => self.bg.run(move || {
                let saved = std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display())).and_then(|()| {
                    let path = unique_path(&dir, &format!("Screenshot-{stamp}"), "png");
                    crate::png::write_file(&path, w, h, &px).map(|()| path).map_err(|e| e.to_string())
                });
                crate::background::Done::Screenshot(saved)
            }),
            Err(e) => {
                tracing::warn!("screenshot failed: {e}");
                self.notify("Screenshot failed", &e);
            }
        }
    }
}

/// Local time as "YYYY-MM-DD-HHMMSS".
fn local_timestamp() -> String {
    // SAFETY: localtime_r writes into the provided struct.
    let tm = unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        tm
    };
    format!("{:04}-{:02}-{:02}-{:02}{:02}{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec)
}

/// `dir/stem.ext`, or `dir/stem-2.ext`, ... if that exists already.
fn unique_path(dir: &std::path::Path, stem: &str, ext: &str) -> PathBuf {
    let mut path = dir.join(format!("{stem}.{ext}"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{stem}-{n}.{ext}"));
        n += 1;
    }
    path
}

fn write_reply(stream: &mut UnixStream, reply: &str) -> std::io::Result<()> {
    // Replies are small; switch to blocking for the write so it is never cut short.
    stream.set_nonblocking(false)?;
    let r = stream.write_all(reply.as_bytes()).and_then(|_| stream.write_all(b"\n"));
    stream.set_nonblocking(true)?;
    r
}
