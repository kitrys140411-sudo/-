//! Laptop power: the battery shown in the panel (from
//! `/sys/class/power_supply`), low-battery warnings, the lid switch and
//! suspend.
//!
//! Battery files are read on a background thread every few seconds: reading
//! them runs ACPI methods in the embedded controller, which takes tens of
//! milliseconds on some laptops. Suspend (like restart and power off) is a
//! request to lumen-init over its control socket; the compositor never
//! touches /sys/power itself.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use smithay::reexports::calloop::timer::{TimeoutAction, Timer};

use crate::background::Done;
use crate::state::Lumen;

/// How often the battery is read.
pub const POLL: Duration = Duration::from_secs(10);
/// Warnings below these levels (percent) while discharging.
pub const LOW: f64 = 10.0;
pub const CRITICAL: f64 = 5.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Charge {
    Charging,
    Discharging,
    Full,
    /// Plugged in but not charging (charge thresholds, a full battery).
    NotCharging,
    Unknown,
}

/// The system battery (all internal batteries together).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Battery {
    /// 0..100.
    pub percent: f64,
    pub charge: Charge,
    pub on_ac: bool,
    /// Until empty while discharging, until full while charging.
    pub time_left: Option<Duration>,
}

impl Battery {
    /// Charging, full or otherwise on external power: no warnings.
    pub fn plugged(&self) -> bool {
        self.on_ac || matches!(self.charge, Charge::Charging | Charge::Full | Charge::NotCharging)
    }

    /// Panel icon level: 0..1.
    pub fn fraction(&self) -> f64 {
        (self.percent / 100.0).clamp(0.0, 1.0)
    }
}

fn read(dir: &Path, name: &str) -> Option<String> {
    std::fs::read_to_string(dir.join(name)).ok().map(|s| s.trim().to_string())
}

fn read_f(dir: &Path, name: &str) -> Option<f64> {
    read(dir, name)?.parse().ok()
}

/// One battery's counters: energy (µWh, µW) or charge (µAh, µA) units.
struct Cell {
    now: Option<f64>,
    full: Option<f64>,
    rate: Option<f64>,
    capacity: Option<f64>,
    charge: Charge,
}

/// Read the power supplies under `sysfs`. None: no system battery (a desktop,
/// or a laptop with its battery removed).
pub fn read_battery(sysfs: &Path) -> Option<Battery> {
    let mut on_ac = false;
    let mut cells: Vec<Cell> = Vec::new();
    let mut entries: Vec<PathBuf> = std::fs::read_dir(sysfs.join("class/power_supply")).ok()?.flatten().map(|e| e.path()).collect();
    entries.sort();
    for dir in entries {
        match read(&dir, "type").as_deref() {
            Some("Mains") | Some("USB") | Some("USB_C") | Some("USB_PD") | Some("USB_PD_DRP") => {
                on_ac |= read(&dir, "online").as_deref() == Some("1");
            }
            Some("Battery") => {
                // Mice, keyboards and gamepads report their batteries with scope Device.
                if read(&dir, "scope").as_deref() == Some("Device") || read(&dir, "present").as_deref() == Some("0") {
                    continue;
                }
                let charge = match read(&dir, "status").as_deref() {
                    Some("Charging") => Charge::Charging,
                    Some("Discharging") => Charge::Discharging,
                    Some("Full") => Charge::Full,
                    Some("Not charging") => Charge::NotCharging,
                    _ => Charge::Unknown,
                };
                let (now, full, rate) = match read_f(&dir, "energy_now") {
                    Some(now) => (Some(now), read_f(&dir, "energy_full"), read_f(&dir, "power_now")),
                    None => (read_f(&dir, "charge_now"), read_f(&dir, "charge_full"), read_f(&dir, "current_now")),
                };
                cells.push(Cell { now, full, rate: rate.map(f64::abs), capacity: read_f(&dir, "capacity"), charge });
            }
            _ => {}
        }
    }
    if cells.is_empty() {
        return None;
    }
    let any = |c: Charge| cells.iter().any(|x| x.charge == c);
    let charge = if any(Charge::Charging) {
        Charge::Charging
    } else if any(Charge::Discharging) {
        Charge::Discharging
    } else if cells.iter().all(|x| x.charge == Charge::Full) {
        Charge::Full
    } else if any(Charge::NotCharging) {
        Charge::NotCharging
    } else {
        Charge::Unknown
    };
    let sums = cells.iter().try_fold((0.0, 0.0, 0.0), |(n, f, r), c| Some((n + c.now?, f + c.full?, r + c.rate.unwrap_or(0.0))));
    let (percent, time_left) = match sums {
        Some((now, full, rate)) if full > 0.0 => {
            let hours = match charge {
                Charge::Discharging if rate > 0.0 => Some(now / rate),
                Charge::Charging if rate > 0.0 => Some((full - now).max(0.0) / rate),
                _ => None,
            };
            let left = hours.filter(|h| h.is_finite() && *h < 100.0).map(|h| Duration::from_secs((h * 3600.0) as u64));
            ((now / full * 100.0).clamp(0.0, 100.0), left)
        }
        // No energy/charge counters: average the capacities.
        _ => {
            let caps: Vec<f64> = cells.iter().filter_map(|c| c.capacity).collect();
            if caps.is_empty() {
                return None;
            }
            (caps.iter().sum::<f64>() / caps.len() as f64, None)
        }
    };
    Some(Battery { percent, charge, on_ac, time_left })
}

/// "1 h 20 min", "35 min".
pub fn format_duration(d: Duration) -> String {
    let mins = (d.as_secs() + 30) / 60;
    match (mins / 60, mins % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

/// Warning state across readings, so each warning shows once per discharge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Warned {
    #[default]
    None,
    Low,
    Critical,
}

/// The warning to show for a new reading, if any (title, body).
pub fn warning(b: &Battery, warned: &mut Warned) -> Option<(String, String)> {
    if b.plugged() {
        *warned = Warned::None;
        return None;
    }
    let left = b.time_left.map(|t| format!(" (about {})", format_duration(t))).unwrap_or_default();
    let pct = b.percent.round() as i64;
    if b.percent <= CRITICAL && *warned != Warned::Critical {
        *warned = Warned::Critical;
        return Some(("Battery critically low".into(), format!("{pct}% left{left}. Connect the charger now.")));
    }
    if b.percent <= LOW && *warned == Warned::None {
        *warned = Warned::Low;
        return Some(("Battery low".into(), format!("{pct}% left{left}.")));
    }
    None
}

/// Connector types of built-in panels (a laptop's own screen).
pub fn is_internal_panel(output_name: &str) -> bool {
    ["eDP", "LVDS", "DSI", "DPI"].iter().any(|p| output_name.starts_with(p))
}

/// Path of lumen-init's control socket.
pub fn init_socket() -> PathBuf {
    std::env::var_os("LUMEN_INIT_CONTROL").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/run/lumen/control"))
}

/// Send `request` to lumen-init and return its reply line (blocking: call
/// off the main loop). Init answers "ok" or "error: ..."; for suspend the
/// answer may come only after resuming, so a missing reply is no error.
pub fn init_request(socket: &Path, request: &str) -> Result<(), String> {
    let mut s = std::os::unix::net::UnixStream::connect(socket).map_err(|e| format!("{}: {e}", socket.display()))?;
    s.set_write_timeout(Some(Duration::from_secs(2))).ok();
    s.write_all(format!("{request}\n").as_bytes()).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(Duration::from_secs(2))).ok();
    let mut reply = String::new();
    let _ = s.take(4096).read_to_string(&mut reply);
    match reply.trim() {
        r if r.starts_with("error") => Err(r.trim_start_matches("error").trim_start_matches(':').trim().to_string()),
        _ => Ok(()),
    }
}

/// Battery poll interval (`$LUMEN_POWER_POLL_MS` in tests).
fn poll_interval() -> Duration {
    std::env::var("LUMEN_POWER_POLL_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(|ms| Duration::from_millis(ms.max(50)))
        .unwrap_or(POLL)
}

/// Two suspend requests closer than this are one (lid switches bounce, and
/// the lid may be closed right after choosing Suspend in the menu).
const SUSPEND_DEBOUNCE: Duration = Duration::from_secs(5);

/// Time since boot including time spent suspended (the monotonic clock
/// stops while suspended, which would make a request right after resuming
/// look like a bounce).
fn boottime() -> Duration {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: clock_gettime writes into the provided struct.
    unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut ts) };
    Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32)
}

impl Lumen {
    /// Read the battery now and every few seconds from now on.
    pub fn start_power_monitor(&mut self) {
        let interval = poll_interval();
        let res = self.lh.insert_source(Timer::immediate(), move |_, _, state| {
            let sysfs = crate::backlight::sysfs_root();
            state.bg.run(move || Done::Battery(read_battery(&sysfs)));
            TimeoutAction::ToDuration(interval)
        });
        if let Err(e) = res {
            tracing::warn!("battery monitor: {e}");
        }
    }

    /// A new battery reading: update the panel, warn when it runs low.
    pub fn on_battery(&mut self, b: Option<Battery>) {
        if let Some(b) = &b {
            if let Some((title, body)) = warning(b, &mut self.battery_warned) {
                tracing::warn!("{title}: {body}");
                self.notify(&title, &body);
            }
        }
        if self.shell.battery != b {
            self.shell.battery = b;
            self.queue_redraw_all();
        }
    }

    /// The laptop lid was closed or opened (libinput switch event).
    pub fn on_lid(&mut self, closed: bool) {
        tracing::info!(closed, "lid switch");
        self.lid_closed = closed;
        self.check_lid();
    }

    /// With the lid closed and no external monitor to work on, suspend.
    /// Also called when outputs go away (unplugging the monitor of a closed
    /// laptop).
    pub fn check_lid(&mut self) {
        if !self.lid_closed {
            return;
        }
        if let Some(o) = self.outputs.iter().find(|o| !is_internal_panel(&o.name())) {
            tracing::info!("lid closed, external output {} connected: staying awake", o.name());
            return;
        }
        self.suspend();
    }

    /// Ask init to suspend (menu, sleep key, lid).
    pub fn suspend(&mut self) {
        let now = boottime();
        if self.last_suspend.is_some_and(|t| now.saturating_sub(t) < SUSPEND_DEBOUNCE) {
            tracing::info!("suspend already requested");
            return;
        }
        self.last_suspend = Some(now);
        self.init_request("suspend");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lumen-power-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn supply(root: &Path, name: &str, files: &[(&str, &str)]) {
        let d = root.join("class/power_supply").join(name);
        std::fs::create_dir_all(&d).unwrap();
        for (k, v) in files {
            std::fs::write(d.join(k), format!("{v}\n")).unwrap();
        }
    }

    #[test]
    fn reads_a_laptop_battery() {
        let root = tmp("one");
        assert_eq!(read_battery(&root), None);
        supply(&root, "AC", &[("type", "Mains"), ("online", "0")]);
        assert_eq!(read_battery(&root), None, "no battery: nothing to show");
        supply(&root, "BAT0", &[
            ("type", "Battery"), ("present", "1"), ("status", "Discharging"), ("capacity", "41"),
            ("energy_now", "20000000"), ("energy_full", "50000000"), ("power_now", "10000000"),
        ]);
        // A wireless mouse's battery does not count.
        supply(&root, "hidpp_battery_0", &[("type", "Battery"), ("scope", "Device"), ("status", "Discharging"), ("capacity", "5")]);
        let b = read_battery(&root).unwrap();
        assert_eq!((b.percent, b.charge, b.on_ac), (40.0, Charge::Discharging, false));
        assert_eq!(b.time_left, Some(Duration::from_secs(7200)));
        assert!(!b.plugged());
        supply(&root, "AC", &[("online", "1")]);
        supply(&root, "BAT0", &[("status", "Charging")]);
        let b = read_battery(&root).unwrap();
        assert!(b.plugged() && b.on_ac);
        assert_eq!(b.time_left, Some(Duration::from_secs(3 * 3600)));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn two_batteries_and_charge_counters() {
        let root = tmp("two");
        supply(&root, "BAT0", &[("type", "Battery"), ("status", "Full"), ("charge_now", "4000000"), ("charge_full", "4000000")]);
        supply(&root, "BAT1", &[("type", "Battery"), ("status", "Discharging"), ("charge_now", "1000000"), ("charge_full", "4000000"),
            ("current_now", "-2000000")]);
        let b = read_battery(&root).unwrap();
        assert_eq!((b.percent, b.charge), (62.5, Charge::Discharging));
        assert_eq!(b.time_left, Some(Duration::from_secs(9000)));
        let _ = std::fs::remove_dir_all(&root);
        // Only capacities: their average.
        let root = tmp("cap");
        supply(&root, "BAT0", &[("type", "Battery"), ("status", "Unknown"), ("capacity", "80")]);
        let b = read_battery(&root).unwrap();
        assert_eq!((b.percent, b.charge, b.time_left), (80.0, Charge::Unknown, None));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn warnings_once_per_discharge() {
        let bat = |percent: f64, charge: Charge| Battery { percent, charge, on_ac: false, time_left: Some(Duration::from_secs(25 * 60)) };
        let mut w = Warned::None;
        assert_eq!(warning(&bat(30.0, Charge::Discharging), &mut w), None);
        let (title, body) = warning(&bat(9.6, Charge::Discharging), &mut w).unwrap();
        assert_eq!((title.as_str(), body.as_str()), ("Battery low", "10% left (about 25 min)."));
        assert_eq!(warning(&bat(8.0, Charge::Discharging), &mut w), None);
        assert_eq!(warning(&bat(5.0, Charge::Discharging), &mut w).unwrap().0, "Battery critically low");
        assert_eq!(warning(&bat(3.0, Charge::Discharging), &mut w), None);
        // Plugging in resets the warnings.
        assert_eq!(warning(&bat(3.0, Charge::Charging), &mut w), None);
        assert_eq!(w, Warned::None);
        assert_eq!(warning(&bat(4.0, Charge::Discharging), &mut w).unwrap().0, "Battery critically low");
    }

    #[test]
    fn durations_and_panels() {
        assert_eq!(format_duration(Duration::from_secs(35 * 60)), "35 min");
        assert_eq!(format_duration(Duration::from_secs(80 * 60 + 10)), "1 h 20 min");
        assert_eq!(format_duration(Duration::from_secs(7200)), "2 h");
        assert!(is_internal_panel("eDP-1") && is_internal_panel("LVDS-1"));
        assert!(!is_internal_panel("HDMI-A-1") && !is_internal_panel("DP-2") && !is_internal_panel("HEADLESS-1"));
    }

    #[test]
    fn init_requests() {
        let dir = tmp("sock");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("control");
        assert!(init_request(&path, "suspend").is_err(), "no init listening");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let mut got = Vec::new();
            for reply in ["ok\n", "error: suspend is not supported\n"] {
                let (mut s, _) = listener.accept().unwrap();
                let mut line = String::new();
                std::io::BufRead::read_line(&mut std::io::BufReader::new(&s), &mut line).unwrap();
                got.push(line);
                s.write_all(reply.as_bytes()).unwrap();
            }
            got
        });
        assert_eq!(init_request(&path, "suspend"), Ok(()));
        assert_eq!(init_request(&path, "suspend"), Err("suspend is not supported".into()));
        assert_eq!(server.join().unwrap(), vec!["suspend\n", "suspend\n"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
