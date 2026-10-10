//! Lumen boot loader: a graphical UEFI menu that starts Linux through its EFI stub.
//!
//! Flow: read `\loader\lumen.conf` from the ESP, show the menu (fade in,
//! countdown), and run the chosen entry. Every failure ends on an error dialog
//! with an explanation and a way back to the menu — the loader never hangs.
#![no_std]
#![no_main]

extern crate alloc;

mod assets {
    use crate::font::{Glyph, Strike};
    include!(concat!(env!("OUT_DIR"), "/assets.rs"));
}
mod clock;
mod font;
mod gfx;
mod input;
mod linux;
#[macro_use]
mod log;
mod power;
mod ui;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use bootcfg::{Action, Config, Entry};
use uefi::boot;
use uefi::prelude::*;
use uefi::proto::console::gop::GraphicsOutput;

const CONFIG_PATH: &str = "\\loader\\lumen.conf";

/// Entries offered when the configuration cannot be used.
fn rescue_entries() -> Vec<Entry> {
    let mut v = Vec::new();
    if power::firmware_ui_supported() {
        v.push(Entry { id: "firmware".into(), title: "Firmware settings".into(), action: Action::Firmware });
    }
    v.push(Entry { id: "reboot".into(), title: "Reboot".into(), action: Action::Reboot });
    v.push(Entry { id: "poweroff".into(), title: "Power off".into(), action: Action::Poweroff });
    v
}

fn load_config() -> Result<Config, String> {
    let bytes = linux::read_esp_file(CONFIG_PATH)?;
    let text = core::str::from_utf8(&bytes).map_err(|_| format!("{CONFIG_PATH} is not valid UTF-8"))?;
    bootcfg::parse(text).map_err(|e| format!("{CONFIG_PATH}: {e}"))
}

#[entry]
fn main() -> Status {
    log::init();
    // The firmware arms a 5 minute watchdog for boot applications; a menu that
    // waits for the user must not be reset by it.
    let _ = boot::set_watchdog_timer(0, 0x10000, None);
    let clock = clock::Clock::calibrate();
    log!("start");

    let (config, banner) = match load_config() {
        Ok(c) => (c, None),
        Err(e) => {
            log!("config error: {e}");
            (Config { timeout: None, default: 0, entries: rescue_entries() }, Some(e))
        }
    };
    let items: Vec<ui::Item> = config
        .entries
        .iter()
        .map(|e| ui::Item {
            title: e.title.clone(),
            editable_cmdline: match &e.action {
                Action::Linux { cmdline, .. } => Some(cmdline.clone()),
                _ => None,
            },
        })
        .collect();

    let gop = boot::get_handle_for_protocol::<GraphicsOutput>()
        .and_then(boot::open_protocol_exclusive::<GraphicsOutput>);
    let Ok(gop) = gop else {
        log!("no GOP, using the text console");
        return text_mode(&config);
    };
    let (w, h) = gop.current_mode_info().resolution();
    log!("gop {}x{}", w, h);

    // timeout = 0 starts the default entry immediately (unless a problem shows up).
    if config.timeout == Some(0) && banner.is_none() {
        let entry = &config.entries[config.default];
        let mut menu = ui::Menu::new(gop, &clock, items, config.default, None, None);
        if let Err((t, d)) = run_entry(&mut menu, entry, None) {
            menu.show_error(&t, &d);
            menu.fade_in();
        } else {
            return Status::SUCCESS;
        }
        return menu_loop(menu, &config);
    }

    let mut menu = ui::Menu::new(gop, &clock, items, config.default, config.timeout, banner);
    menu.fade_in();
    log!("menu shown with {} entries", config.entries.len());
    menu_loop(menu, &config)
}

fn menu_loop(mut menu: ui::Menu, config: &Config) -> Status {
    loop {
        let ui::Choice::Run { index, cmdline } = menu.run();
        let entry = &config.entries[index];
        log!("selected '{}'", entry.id);
        if let Err((title, detail)) = run_entry(&mut menu, entry, cmdline) {
            log!("error: {title}: {detail}");
            menu.acquire_display();
            menu.show_error(&title, &detail);
            menu.fade_in();
        }
    }
}

/// Run an entry. Returns only on failure, with a title and explanation.
fn run_entry(menu: &mut ui::Menu, entry: &Entry, cmdline_override: Option<String>) -> Result<(), (String, String)> {
    match &entry.action {
        Action::Linux { kernel, initrd, cmdline } => {
            let cmdline = cmdline_override.unwrap_or_else(|| cmdline.clone());
            let prepared = linux::prepare(kernel, initrd.as_deref(), &cmdline).map_err(|e| (e.title, e.detail))?;
            menu.fade_out();
            log!("starting kernel {kernel} cmdline '{cmdline}'");
            menu.release_display();
            log::shutdown();
            let err = linux::start(prepared);
            log::init();
            Err((err.title, err.detail))
        }
        Action::Firmware => {
            if !power::firmware_ui_supported() {
                return Err((
                    "Firmware settings unavailable".to_string(),
                    "This firmware does not support rebooting into its setup screen. Use the key shown by the firmware at power-on instead.".to_string(),
                ));
            }
            menu.fade_out();
            let status = power::reboot_to_firmware();
            Err(("Cannot open firmware settings".to_string(), format!("Setting OsIndications failed: {status:?}")))
        }
        Action::Reboot => {
            menu.fade_out();
            power::reboot()
        }
        Action::Poweroff => {
            menu.fade_out();
            power::poweroff()
        }
    }
}

/// Minimal fallback for machines without a graphics output protocol.
fn text_mode(config: &Config) -> Status {
    use uefi::println;
    use uefi::proto::console::text::Key;
    loop {
        println!("Lumen boot menu");
        for (i, e) in config.entries.iter().enumerate() {
            println!("  {} {}{}", i + 1, e.title, if i == config.default { "  (default)" } else { "" });
        }
        println!("Press a number, or Enter for the default.");
        let index = loop {
            if let Some(ev) = input::key_event() {
                let _ = boot::wait_for_event(&[ev]);
            }
            match uefi::system::with_stdin(|s| s.read_key()) {
                Ok(Some(Key::Printable(c))) => {
                    let c = char::from(c);
                    if c == '\r' {
                        break config.default;
                    }
                    if let Some(d) = c.to_digit(10) {
                        if d >= 1 && (d as usize) <= config.entries.len() {
                            break d as usize - 1;
                        }
                    }
                }
                _ => {}
            }
        };
        let e = &config.entries[index];
        match &e.action {
            Action::Linux { kernel, initrd, cmdline } => match linux::prepare(kernel, initrd.as_deref(), cmdline) {
                Ok(p) => {
                    let err = linux::start(p);
                    println!("{}: {}", err.title, err.detail);
                }
                Err(err) => println!("{}: {}", err.title, err.detail),
            },
            Action::Firmware => {
                let s = power::reboot_to_firmware();
                println!("Cannot open firmware settings: {s:?}");
            }
            Action::Reboot => power::reboot(),
            Action::Poweroff => power::poweroff(),
        }
    }
}
