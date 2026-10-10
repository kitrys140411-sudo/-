//! lumen-init: the Lumen PID 1.
//!
//! The same binary runs twice during boot:
//! * stage 1 from the initramfs (marker file `/.lumen-initramfs`): find and mount
//!   the root filesystem, then switch to it and exec `/sbin/init`;
//! * stage 2 as `/sbin/init` on the real root: mount API filesystems, run and
//!   supervise services, shut down cleanly.
//!
//! `lumen-init --check DIR` validates service files (used by the image build).

mod cmdline;
mod log;
mod service;
mod stage1;
mod supervisor;
mod sys;
mod users;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--check") {
        let dir = args.get(2).map(String::as_str).unwrap_or("/etc/lumen/services");
        let (defs, errors) = service::load_dir(dir);
        for d in &defs {
            println!("ok   {} ({})", d.name, d.exec.join(" "));
        }
        for e in &errors {
            eprintln!("FAIL {e}");
        }
        std::process::exit(if errors.is_empty() && !defs.is_empty() { 0 } else { 1 });
    }
    if std::process::id() != 1 {
        eprintln!("lumen-init must run as PID 1 (or use --check DIR)");
        std::process::exit(2);
    }
    if std::path::Path::new("/.lumen-initramfs").exists() {
        stage1::run();
    }
    supervisor::run();
}
