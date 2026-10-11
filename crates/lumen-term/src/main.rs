//! lumen-term: a GPU-accelerated terminal for the Lumen desktop.
//!
//!   lumen-term [--title T] [--app-id ID] [--config FILE] [-e PROGRAM ARGS...]

mod app;
mod chrome;
mod gfx;
mod handlers;
mod pane;
mod view;

use std::process::ExitCode;

use smithay_client_toolkit as sctk;
use sctk::reexports::calloop::EventLoop;
use sctk::reexports::calloop_wayland_source::WaylandSource;
use sctk::reexports::client::globals::registry_queue_init;
use sctk::reexports::client::Connection;
use sctk::shell::WaylandSurface;

use lumen_term::config::Config;

const USAGE: &str = "usage: lumen-term [--title TITLE] [--app-id ID] [--config FILE] [-e PROGRAM [ARGS...]]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut title = None;
    let mut app_id = "lumen-term".to_string();
    let mut program = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            "--title" if i + 1 < args.len() => {
                title = Some(args[i + 1].clone());
                i += 1;
            }
            "--app-id" | "--class" if i + 1 < args.len() => {
                app_id = args[i + 1].clone();
                i += 1;
            }
            "--config" if i + 1 < args.len() => {
                // SAFETY: single-threaded at this point.
                unsafe { std::env::set_var("LUMEN_TERM_CONFIG", &args[i + 1]) };
                i += 1;
            }
            "-e" | "--" => {
                if i + 1 >= args.len() {
                    eprintln!("lumen-term: -e needs a program\n{USAGE}");
                    return ExitCode::from(2);
                }
                program = Some(args[i + 1..].to_vec());
                break;
            }
            a => {
                eprintln!("lumen-term: unknown argument {a}\n{USAGE}");
                return ExitCode::from(2);
            }
        }
        i += 1;
    }
    let (cfg, err) = Config::load();
    if let Some(e) = &err {
        eprintln!("lumen-term: config: {e} (using defaults)");
    }
    match run(cfg, err, program, title, &app_id) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("lumen-term: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cfg: Config, err: Option<String>, program: Option<Vec<String>>, title: Option<String>, app_id: &str) -> Result<(), String> {
    // Children must not inherit SIGPIPE ignored etc.; we ignore SIGPIPE
    // ourselves (writes to a dead PTY or clipboard reader return EPIPE).
    // SAFETY: setting a signal disposition.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
    let conn = Connection::connect_to_env().map_err(|e| format!("cannot connect to the Wayland compositor: {e}"))?;
    let (globals, mut queue) = registry_queue_init::<app::App>(&conn).map_err(|e| format!("wl_registry: {e}"))?;
    let qh = queue.handle();
    let mut event_loop: EventLoop<'static, app::App> = EventLoop::try_new().map_err(|e| format!("event loop: {e}"))?;
    let lh = event_loop.handle();
    let mut app = app::App::new(conn.clone(), qh, lh.clone(), &globals, cfg, err, program, title, app_id)?;
    // Seat capabilities (keyboard, data devices) before mapping.
    queue.roundtrip(&mut app).map_err(|e| format!("roundtrip: {e}"))?;
    app.window.commit();
    WaylandSource::new(conn.clone(), queue).insert(lh).map_err(|e| format!("event loop: {}", e.error))?;
    // Reap exited children (their PTY reports EOF/EIO and closes the pane).
    while !app.exit {
        if let Err(e) = event_loop.dispatch(None, &mut app) {
            return Err(format!("connection lost: {e}"));
        }
    }
    app.teardown();
    drop(app);
    let _ = conn.flush();
    Ok(())
}
