//! Diagnostic log to the serial port (if the machine has one).
//!
//! Text written to ConOut would be painted over the graphical menu by the
//! firmware's graphics console, so the log talks to the Serial I/O protocol
//! directly. Tests read these lines through QEMU's serial chardev.

use core::fmt::{self, Write};
use core::ptr::NonNull;
use uefi::boot::{self, OpenProtocolAttributes, OpenProtocolParams};
use uefi::proto::console::serial::Serial;

static mut SERIAL: Option<NonNull<Serial>> = None;

pub fn init() {
    let Ok(handles) = boot::find_handles::<Serial>() else {
        return;
    };
    let Some(&handle) = handles.first() else {
        return;
    };
    // SAFETY: GetProtocol does not take ownership; TerminalDxe keeps working.
    let opened = unsafe {
        boot::open_protocol::<Serial>(
            OpenProtocolParams {
                handle,
                agent: boot::image_handle(),
                controller: None,
            },
            OpenProtocolAttributes::GetProtocol,
        )
    };
    if let Ok(mut proto) = opened {
        if let Some(p) = proto.get_mut() {
            // SAFETY: single-threaded boot environment; set once before use.
            unsafe { SERIAL = Some(NonNull::from(p)) };
        }
        // Keep the interface open for the lifetime of the loader.
        core::mem::forget(proto);
    }
}

struct SerialWriter;

impl Write for SerialWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        // SAFETY: see init(); the pointer stays valid while boot services run.
        if let Some(mut p) = unsafe { SERIAL } {
            let serial = unsafe { p.as_mut() };
            for chunk in s.split_inclusive('\n') {
                if let Some(line) = chunk.strip_suffix('\n') {
                    let _ = serial.write(line.as_bytes());
                    let _ = serial.write(b"\r\n");
                } else {
                    let _ = serial.write(chunk.as_bytes());
                }
            }
        }
        Ok(())
    }
}

pub fn write(args: fmt::Arguments) {
    let _ = SerialWriter.write_fmt(args);
}

/// Forget the serial protocol before handing control to the kernel.
pub fn shutdown() {
    // SAFETY: single-threaded.
    unsafe { SERIAL = None };
}

#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => {
        $crate::log::write(format_args!("lumen-boot: {}\n", format_args!($($arg)*)))
    };
}
