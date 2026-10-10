//! Lumen boot loader — milestone 0 smoke version: paints the screen through GOP
//! and echoes key presses to the console so the QEMU/QMP harness can be tested.
#![no_std]
#![no_main]

use uefi::prelude::*;
use uefi::proto::console::gop::{BltOp, BltPixel, GraphicsOutput};
use uefi::proto::console::text::Key;
use uefi::{boot, println, system};

#[entry]
fn main() -> Status {
    println!("lumen-boot: hello");
    if let Ok(handle) = boot::get_handle_for_protocol::<GraphicsOutput>() {
        if let Ok(mut gop) = boot::open_protocol_exclusive::<GraphicsOutput>(handle) {
            let (w, h) = gop.current_mode_info().resolution();
            println!("lumen-boot: gop {}x{}", w, h);
            let _ = gop.blt(BltOp::VideoFill {
                color: BltPixel::new(0x24, 0x1f, 0x31),
                dest: (0, 0),
                dims: (w, h),
            });
            let _ = gop.blt(BltOp::VideoFill {
                color: BltPixel::new(0x62, 0xa0, 0xea),
                dest: (w / 2 - 50, h / 2 - 50),
                dims: (100, 100),
            });
        }
    }
    loop {
        let event = system::with_stdin(|stdin| stdin.wait_for_key_event().unwrap());
        let _ = boot::wait_for_event(&[event]);
        let key = system::with_stdin(|stdin| stdin.read_key());
        if let Ok(Some(key)) = key {
            match key {
                Key::Printable(c) => {
                    println!("lumen-boot: key {}", char::from(c));
                    if char::from(c) == 'q' {
                        return Status::SUCCESS;
                    }
                }
                Key::Special(s) => println!("lumen-boot: special {:?}", s),
            }
        }
    }
}
