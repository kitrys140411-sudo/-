//! Lumen: Wayland compositor with a built-in desktop shell.
#![cfg_attr(test, allow(dead_code))]

mod anim;
mod apps;
mod backend;
mod background;
mod backlight;
mod cursor;
mod cursor_art;
mod focus;
mod gaming;
mod handlers;
mod icons;
mod icontheme;
mod input;
mod inputcfg;
mod ipc;
mod keybind;
mod layouts;
mod png;
mod power;
mod render;
mod selection;
mod shell;
mod state;
mod text;
mod theme;
mod vector;
mod volume;
mod wm;
mod xwayland;

use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::process::ExitCode;

use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{EventLoop, Interest, Mode, PostAction};
use smithay::reexports::wayland_server::Display;

use crate::backend::Backend;
use crate::state::Lumen;

const USAGE: &str = "usage: lumen [--backend auto|udev|winit|headless] [--size WxH] [--scale S] [-- command...]

  --backend   udev: DRM/KMS on the current seat (default without a parent compositor)
              winit: nested in a window (default when WAYLAND_DISPLAY or DISPLAY is set)
              headless: offscreen output for tests and profiling
  --size      headless output size in pixels (default 1280x800)
  --scale     output scale for headless (others use LUMEN_SCALE or the display's DPI)
  -- cmd      program to start once the compositor is running

environment: LUMEN_LOG (log filter), LUMEN_THEME, LUMEN_KEYBINDINGS, LUMEN_INPUT (input.toml), LUMEN_FAVORITES,
             LUMEN_SCALE, LUMEN_IPC, LUMEN_HUD, XKB_DEFAULT_LAYOUT/VARIANT/OPTIONS/MODEL (override input.toml),
             XCURSOR_THEME, XCURSOR_SIZE, LUMEN_INIT_CONTROL (init's socket), LUMEN_SYSFS (/sys for batteries
             and backlights), LUMEN_POWER_POLL_MS, LUMEN_OUTPUT_NAME (headless output name),
             LUMEN_XWAYLAND=0 (no X11 support), LUMEN_IDLE_BLANK (minutes until the screen blanks, 0 = never),
             LUMEN_XWAYLAND_SCALE (auto|native|scaled: X11 apps on scaled outputs), LUMEN_VRR (fullscreen|off|always),
             LUMEN_DRM_DEVICE (GPU to render on, default: the boot GPU)";

struct Args {
    backend: String,
    size: (i32, i32),
    scale: f64,
    autostart: Vec<String>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args { backend: "auto".into(), size: (1280, 800), scale: 1.0, autostart: Vec::new() };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--backend" => args.backend = it.next().ok_or("--backend needs a value")?,
            "--size" => {
                let v = it.next().ok_or("--size needs WxH")?;
                let (w, h) = v.split_once('x').ok_or("--size needs WxH")?;
                args.size = (w.parse().map_err(|_| "bad width")?, h.parse().map_err(|_| "bad height")?);
            }
            "--scale" => args.scale = it.next().ok_or("--scale needs a value")?.parse().map_err(|_| "bad scale")?,
            "--version" => {
                println!("lumen {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            "--" => {
                args.autostart = it.by_ref().collect();
            }
            other => return Err(format!("unknown argument '{other}'\n{USAGE}")),
        }
    }
    if !(0.5..=4.0).contains(&args.scale) {
        return Err("--scale must be between 0.5 and 4".into());
    }
    Ok(args)
}

/// Block termination signals and return a signalfd that reports them, so they
/// are handled in the event loop. Must run before any thread is started.
fn termination_signalfd() -> std::io::Result<OwnedFd> {
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        for sig in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
            libc::sigaddset(&mut set, sig);
        }
        if libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut()) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let fd = libc::signalfd(-1, &set, libc::SFD_CLOEXEC | libc::SFD_NONBLOCK);
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(OwnedFd::from_raw_fd(fd))
    }
}

fn main() -> ExitCode {
    let sigfd = match termination_signalfd() {
        Ok(fd) => fd,
        Err(e) => {
            eprintln!("lumen: signalfd: {e}");
            return ExitCode::FAILURE;
        }
    };
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("lumen: {e}");
            return ExitCode::from(2);
        }
    };
    let filter = std::env::var("LUMEN_LOG").unwrap_or_else(|_| "info,smithay=warn".into());
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
        .with_writer(std::io::stderr)
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .init();

    match run(args, sigfd) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Args, sigfd: OwnedFd) -> Result<(), String> {
    let mut event_loop: EventLoop<'static, Lumen> = EventLoop::try_new().map_err(|e| format!("event loop: {e}"))?;
    let display: Display<Lumen> = Display::new().map_err(|e| format!("wayland display: {e}"))?;
    let mut state = Lumen::new(display, event_loop.handle(), event_loop.get_signal(), Backend::None);

    let nested = std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some();
    let kind = match args.backend.as_str() {
        "auto" if nested => "winit",
        "auto" => "udev",
        k => k,
    };
    match kind {
        "udev" => {
            let (session, notifier) = smithay::backend::session::libseat::LibSeatSession::new()
                .map_err(|e| format!("cannot open a seat session (is seatd running?): {e}"))?;
            backend::udev::init(&mut state, session, notifier)?;
        }
        "winit" => backend::winit::init(&mut state)?,
        "headless" => backend::headless::init(&mut state, args.size.into(), args.scale)?,
        other => return Err(format!("unknown backend '{other}'")),
    }
    tracing::info!(backend = state.backend.name(), socket = ?state.socket_name, "compositor running");

    state.start_clock();
    state.start_power_monitor();
    let ipc_path = ipc::init(&mut state)?;
    tracing::info!("control socket at {}", ipc_path.display());

    event_loop
        .handle()
        .insert_source(Generic::new(sigfd, Interest::READ, Mode::Level), |_, fd, state| {
            let mut info: libc::signalfd_siginfo = unsafe { std::mem::zeroed() };
            let n = unsafe {
                libc::read(fd.as_fd().as_raw_fd(), &mut info as *mut _ as *mut libc::c_void, std::mem::size_of::<libc::signalfd_siginfo>())
            };
            if n == std::mem::size_of::<libc::signalfd_siginfo>() as isize {
                tracing::info!("signal {} received, exiting", info.ssi_signo);
                state.signal.stop();
            }
            Ok(PostAction::Continue)
        })
        .map_err(|e| format!("signal source: {e}"))?;

    if !args.autostart.is_empty() {
        state.spawn(&args.autostart);
    }
    state.queue_redraw_all();

    event_loop
        .run(None, &mut state, |state| {
            state.popups.cleanup();
            state.check_x11_keyboard_grab();
            state.check_xwayland_startup();
            let _ = state.dh.flush_clients();
        })
        .map_err(|e| format!("event loop: {e}"))?;

    let _ = std::fs::remove_file(&ipc_path);
    tracing::info!("bye");
    Ok(())
}
