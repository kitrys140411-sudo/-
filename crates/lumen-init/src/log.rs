//! Logging to the kernel ring buffer (/dev/kmsg), so messages get kernel
//! timestamps and show up on the serial console and in `dmesg`.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::Mutex;

static KMSG: Mutex<Option<File>> = Mutex::new(None);

pub fn open() {
    if let Ok(f) = OpenOptions::new().write(true).open("/dev/kmsg") {
        *KMSG.lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
    }
}

fn emit(level: u8, msg: &str) {
    let mut guard = KMSG.lock().unwrap_or_else(|e| e.into_inner());
    for line in msg.lines() {
        let text = format!("<{level}>lumen-init: {line}\n");
        let ok = guard.as_mut().map(|f| f.write_all(text.as_bytes()).is_ok()).unwrap_or(false);
        if !ok {
            let _ = std::io::stderr().write_all(&text.as_bytes()[3..]);
        }
    }
}

pub fn info(msg: &str) {
    emit(6, msg);
}

pub fn notice(msg: &str) {
    emit(5, msg);
}

pub fn warn(msg: &str) {
    emit(4, msg);
}

pub fn error(msg: &str) {
    emit(3, msg);
}

/// Write directly to the console (for the user, independent of loglevel).
pub fn console(msg: &str) {
    if let Ok(mut f) = OpenOptions::new().write(true).open("/dev/console") {
        let _ = f.write_all(msg.replace('\n', "\r\n").as_bytes());
    }
}
