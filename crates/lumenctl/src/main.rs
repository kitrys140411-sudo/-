//! lumenctl — talk to lumen-init (and later the desktop) over unix sockets.
//!
//!   lumenctl poweroff | reboot | halt
//!   lumenctl status
//!   lumenctl start|stop|restart SERVICE
//!   lumenctl log TEXT...          append a line to the kernel log (via init)
//!   lumenctl notify-ready         send READY=1 to init (for shell services)
//!   lumenctl desktop REQUEST...   send a request to the compositor's control
//!                                 socket (see `lumen` ipc.rs), print the reply

use std::io::{Read, Write};
use std::os::unix::net::{UnixDatagram, UnixStream};
use std::process::ExitCode;

const CONTROL: &str = "/run/lumen/control";
const NOTIFY: &str = "/run/lumen/notify";

fn usage() -> ExitCode {
    eprintln!(
        "usage: lumenctl COMMAND\n\
         \n  poweroff | reboot | halt     shut the machine down\
         \n  status                       list services\
         \n  start|stop|restart SERVICE   control a service\
         \n  log TEXT...                  write a line to the kernel log\
         \n  notify-ready                 report readiness to init\
         \n  desktop REQUEST...           talk to the compositor (ping, windows, stats,\
         \n                               screenshot PATH, action NAME, ...)"
    );
    ExitCode::from(2)
}

fn control(request: &str) -> ExitCode {
    let mut stream = match UnixStream::connect(CONTROL) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("lumenctl: cannot connect to {CONTROL}: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = stream.write_all(format!("{request}\n").as_bytes()) {
        eprintln!("lumenctl: {e}");
        return ExitCode::FAILURE;
    }
    let mut reply = String::new();
    let _ = stream.read_to_string(&mut reply);
    print!("{reply}");
    if reply.starts_with("error") { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}

/// Path of the compositor's control socket: $LUMEN_IPC, else the desktop
/// user's runtime directory (so root on the serial console reaches it too).
fn desktop_socket() -> String {
    if let Ok(p) = std::env::var("LUMEN_IPC") {
        return p;
    }
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        let p = format!("{dir}/lumen-ipc.sock");
        if std::path::Path::new(&p).exists() {
            return p;
        }
    }
    "/run/user/1000/lumen-ipc.sock".into()
}

fn desktop(request: &str) -> ExitCode {
    let path = desktop_socket();
    let mut stream = match UnixStream::connect(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("lumenctl: cannot connect to {path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = stream.write_all(format!("{request}\n").as_bytes()) {
        eprintln!("lumenctl: {e}");
        return ExitCode::FAILURE;
    }
    // One reply line per request; stop reading at the newline.
    let mut reply = Vec::new();
    let mut byte = [0u8; 1];
    while let Ok(1) = stream.read(&mut byte) {
        if byte[0] == b'\n' {
            break;
        }
        reply.push(byte[0]);
    }
    let reply = String::from_utf8_lossy(&reply);
    println!("{reply}");
    if reply.starts_with("error") || reply.is_empty() { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first() else { return usage() };
    match (cmd.as_str(), args.len()) {
        ("poweroff" | "reboot" | "halt" | "status", 1) => control(cmd),
        ("start" | "stop" | "restart", 2) => control(&format!("{cmd} {}", args[1])),
        ("log", n) if n >= 2 => control(&format!("log {}", args[1..].join(" "))),
        ("notify-ready", 1) => {
            let path = std::env::var("LUMEN_NOTIFY_SOCKET").unwrap_or_else(|_| NOTIFY.into());
            match UnixDatagram::unbound().and_then(|s| s.send_to(b"READY=1", &path)) {
                Ok(_) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("lumenctl: {path}: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        ("desktop", n) if n >= 2 => desktop(&args[1..].join(" ")),
        _ => usage(),
    }
}
