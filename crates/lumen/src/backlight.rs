//! Screen brightness keys: the laptop panel's backlight in
//! `/sys/class/backlight/*/brightness` (made group-writable for the video
//! group by the device daemon), with the new level shown in the OSD.
//!
//! Sysfs writes go through the driver (ACPI methods, sometimes I2C) and can
//! take a while, so they run off the main loop like the volume commands.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrightnessCmd {
    Up,
    Down,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// No backlight device (desktop monitors are adjusted on the monitor).
    NoDevice,
    /// The brightness file is not writable for us.
    Permission(String),
    Failed(String),
}

/// One backlight device.
#[derive(Debug, Clone, PartialEq)]
pub struct Device {
    pub dir: PathBuf,
    pub max: u32,
}

/// Root of sysfs (`$LUMEN_SYSFS` in tests).
pub fn sysfs_root() -> PathBuf {
    std::env::var_os("LUMEN_SYSFS").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/sys"))
}

fn read_u32(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// The device the keys control: firmware (ACPI) interfaces first, then
/// platform, then raw GPU ones, as they reflect what the firmware expects;
/// names break ties so the choice is stable.
pub fn find(sysfs: &Path) -> Option<Device> {
    let dir = sysfs.join("class/backlight");
    let mut found: Vec<(u8, String, Device)> = Vec::new();
    for e in std::fs::read_dir(&dir).ok()?.flatten() {
        let path = e.path();
        let Some(max) = read_u32(&path.join("max_brightness")).filter(|m| *m > 0) else { continue };
        let rank = match std::fs::read_to_string(path.join("type")).unwrap_or_default().trim() {
            "firmware" => 0,
            "platform" => 1,
            "raw" => 2,
            _ => 3,
        };
        let name = e.file_name().to_string_lossy().into_owned();
        found.push((rank, name, Device { dir: path, max }));
    }
    found.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    found.into_iter().next().map(|(_, _, d)| d)
}

/// The next raw value: 5 % steps on a grid of 5 %, never down to 0 (a black
/// panel looks like a dead one) and always at least one raw step, so coarse
/// devices (max 7 or 15) still move.
pub fn step(cur: u32, max: u32, cmd: BrightnessCmd) -> u32 {
    let max = max.max(1);
    let min = (max / 100).max(1);
    let pct = cur.min(max) as f64 * 100.0 / max as f64;
    let target_pct = match cmd {
        BrightnessCmd::Up => ((pct + 0.5) / 5.0).floor() * 5.0 + 5.0,
        BrightnessCmd::Down => ((pct - 0.5) / 5.0).ceil() * 5.0 - 5.0,
    };
    let mut v = (target_pct.clamp(0.0, 100.0) * max as f64 / 100.0).round() as u32;
    match cmd {
        BrightnessCmd::Up if v <= cur => v = (cur + 1).min(max),
        BrightnessCmd::Down if v >= cur => v = cur.saturating_sub(1),
        _ => {}
    }
    v.clamp(min.min(cur.max(min)), max)
}

/// Apply `cmd` and return the new level (0..1). Blocking: off the main loop.
pub fn run(sysfs: &Path, cmd: BrightnessCmd) -> Result<f64, Error> {
    let dev = find(sysfs).ok_or(Error::NoDevice)?;
    let file = dev.dir.join("brightness");
    let cur = read_u32(&file).ok_or_else(|| Error::Failed(format!("cannot read {}", file.display())))?;
    let next = step(cur, dev.max, cmd);
    if next != cur {
        std::fs::write(&file, format!("{next}\n")).map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                Error::Permission(format!("{}: {e}", file.display()))
            } else {
                Error::Failed(format!("{}: {e}", file.display()))
            }
        })?;
    }
    let now = read_u32(&file).unwrap_or(next);
    Ok(now as f64 / dev.max as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake(root: &Path, name: &str, kind: &str, cur: u32, max: u32) {
        let d = root.join("class/backlight").join(name);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("type"), format!("{kind}\n")).unwrap();
        std::fs::write(d.join("max_brightness"), format!("{max}\n")).unwrap();
        std::fs::write(d.join("brightness"), format!("{cur}\n")).unwrap();
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lumen-backlight-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn steps_are_five_percent_and_never_black() {
        use BrightnessCmd::*;
        assert_eq!(step(50, 100, Up), 55);
        assert_eq!(step(52, 100, Up), 55);
        assert_eq!(step(52, 100, Down), 50);
        assert_eq!(step(100, 100, Up), 100);
        assert_eq!(step(5, 100, Down), 1);
        assert_eq!(step(1, 100, Down), 1);
        assert_eq!(step(1, 100, Up), 5);
        // Intel-style large ranges and coarse firmware ranges.
        assert_eq!(step(96000, 120000, Up), 102000);
        assert_eq!(step(6000, 120000, Down), 1200);
        assert_eq!(step(7, 15, Up), 8);
        assert_eq!(step(8, 15, Down), 7);
        assert_eq!(step(1, 15, Down), 1);
        assert_eq!(step(0, 15, Up), 1);
    }

    #[test]
    fn prefers_firmware_devices() {
        let root = tmp("pick");
        fake(&root, "intel_backlight", "raw", 400, 1000);
        fake(&root, "acpi_video0", "firmware", 5, 10);
        assert_eq!(find(&root).unwrap().dir.file_name().unwrap(), "acpi_video0");
        assert!(find(&root.join("none")).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn run_writes_and_reads_back() {
        let root = tmp("run");
        assert_eq!(run(&root, BrightnessCmd::Up), Err(Error::NoDevice));
        fake(&root, "intel_backlight", "raw", 500, 1000);
        assert_eq!(run(&root, BrightnessCmd::Up), Ok(0.55));
        assert_eq!(std::fs::read_to_string(root.join("class/backlight/intel_backlight/brightness")).unwrap().trim(), "550");
        assert_eq!(run(&root, BrightnessCmd::Down), Ok(0.5));
        let _ = std::fs::remove_dir_all(&root);
    }
}
