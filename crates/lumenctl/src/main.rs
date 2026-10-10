//! lumenctl — talk to lumen-init (and later the desktop) over unix sockets.
//!
//!   lumenctl poweroff | reboot | halt
//!   lumenctl status
//!   lumenctl start|stop|restart SERVICE
//!   lumenctl log TEXT...          append a line to the kernel log (via init)
//!   lumenctl notify-ready         send READY=1 to init (for shell services)

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
         \n  notify-ready                 report readiness to init"
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
        _ => usage(),
    }
}
