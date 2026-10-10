//! Stage 1: runs from the initramfs. Finds and mounts the root filesystem,
//! moves the API filesystems over and executes the real init. Any failure ends
//! in a rescue shell with an explanation instead of a kernel panic.

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use crate::{cmdline, log, sys};

const NEWROOT: &str = "/newroot";

pub fn run() -> ! {
    let _ = sys::mount_once("devtmpfs", "/dev", libc::MS_NOSUID, Some("mode=0755"));
    let _ = sys::mount_once("proc", "/proc", libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC, None);
    let _ = sys::mount_once("sysfs", "/sys", libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC, None);
    log::open();
    log::notice(&format!("stage 1 (initramfs) at {:.3}s", sys::boottime().as_secs_f64()));

    let args = cmdline::parse(&fs::read_to_string("/proc/cmdline").unwrap_or_default());
    match mount_root(&args) {
        Ok(init) => {
            let err = switch_root(&init);
            rescue(&format!("Cannot switch to the root filesystem: {err}"));
        }
        Err(e) => rescue(&e),
    }
}

fn mount_root(args: &[(String, String)]) -> Result<String, String> {
    let spec = cmdline::get(args, "root").ok_or("No root= parameter on the kernel command line.")?;
    let fstype = cmdline::get(args, "rootfstype").unwrap_or("ext4");
    let flags = if cmdline::has(args, "ro") { libc::MS_RDONLY } else { 0 };
    let data = cmdline::get(args, "rootflags");
    let wait = if cmdline::has(args, "rootwait") { Duration::from_secs(30) } else { Duration::from_secs(10) };

    let dev = find_device(spec, wait)?;
    log::notice(&format!("root {spec} is {dev}"));
    let _ = fs::create_dir_all(NEWROOT);
    sys::mount(&dev, NEWROOT, Some(fstype), flags | libc::MS_NOATIME, data)
        .map_err(|e| format!("Mounting {dev} ({fstype}) on {NEWROOT} failed: {e}."))?;

    let init = cmdline::get(args, "init").unwrap_or("/sbin/init").to_string();
    let target = format!("{NEWROOT}{init}");
    match fs::metadata(&target) {
        Ok(m) if m.is_file() && m.permissions().mode() & 0o111 != 0 => Ok(init),
        _ => {
            let _ = sys::umount(NEWROOT, 0);
            Err(format!("The root filesystem on {dev} has no executable {init}."))
        }
    }
}

struct BlockDev {
    name: String,
    partuuid: Option<String>,
    partname: Option<String>,
}

fn block_devices() -> Vec<BlockDev> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir("/sys/class/block") else { return out };
    for e in rd.flatten() {
        let Ok(uevent) = fs::read_to_string(e.path().join("uevent")) else { continue };
        let mut dev = BlockDev { name: String::new(), partuuid: None, partname: None };
        for line in uevent.lines() {
            match line.split_once('=') {
                Some(("DEVNAME", v)) => dev.name = v.to_string(),
                Some(("PARTUUID", v)) => dev.partuuid = Some(v.to_ascii_lowercase()),
                Some(("PARTNAME", v)) => dev.partname = Some(v.to_string()),
                _ => {}
            }
        }
        if !dev.name.is_empty() {
            out.push(dev);
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Resolve `PARTUUID=`, `PARTLABEL=` or `/dev/...` to a device node, waiting
/// for the device to appear.
fn find_device(spec: &str, wait: Duration) -> Result<String, String> {
    let start = Instant::now();
    let mut announced = false;
    loop {
        let found = if let Some(uuid) = spec.strip_prefix("PARTUUID=") {
            let uuid = uuid.split('/').next().unwrap_or(uuid).to_ascii_lowercase();
            block_devices().into_iter().find(|d| d.partuuid.as_deref() == Some(&uuid)).map(|d| format!("/dev/{}", d.name))
        } else if let Some(label) = spec.strip_prefix("PARTLABEL=") {
            block_devices().into_iter().find(|d| d.partname.as_deref() == Some(label)).map(|d| format!("/dev/{}", d.name))
        } else if spec.starts_with("/dev/") {
            Some(spec.to_string())
        } else {
            return Err(format!("Unsupported root={spec} (use PARTUUID=, PARTLABEL= or /dev/...)."));
        };
        if let Some(dev) = found {
            if Path::new(&dev).exists() {
                return Ok(dev);
            }
        }
        if start.elapsed() > wait {
            let list: Vec<String> = block_devices()
                .iter()
                .map(|d| {
                    let mut s = d.name.clone();
                    if let Some(u) = &d.partuuid {
                        s += &format!(" PARTUUID={u}");
                    }
                    if let Some(n) = &d.partname {
                        s += &format!(" PARTLABEL={n}");
                    }
                    s
                })
                .collect();
            let list = if list.is_empty() { "  (none)".to_string() } else { format!("  {}", list.join("\n  ")) };
            return Err(format!(
                "The root device root={spec} did not appear within {} s.\nBlock devices found:\n{list}",
                wait.as_secs()
            ));
        }
        if !announced && start.elapsed() > Duration::from_secs(3) {
            log::console(&format!("lumen-init: waiting for root device {spec}...\n"));
            announced = true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Remove the initramfs contents (they are in RAM) without crossing into other
/// filesystems, like switch_root(8) does.
fn wipe(dir: &Path, root_dev: u64) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let path = e.path();
        let Ok(meta) = fs::symlink_metadata(&path) else { continue };
        if meta.dev() != root_dev {
            continue;
        }
        if meta.is_dir() {
            wipe(&path, root_dev);
            let _ = fs::remove_dir(&path);
        } else {
            let _ = fs::remove_file(&path);
        }
    }
}

fn switch_root(init: &str) -> std::io::Error {
    for api in ["/dev", "/proc", "/sys"] {
        let target = format!("{NEWROOT}{api}");
        let _ = fs::create_dir_all(&target);
        if let Err(e) = sys::mount(api, &target, None, libc::MS_MOVE, None) {
            return e;
        }
    }
    if let Ok(meta) = fs::metadata("/") {
        wipe(Path::new("/"), meta.dev());
    }
    if let Err(e) = std::env::set_current_dir(NEWROOT) {
        return e;
    }
    if let Err(e) = sys::mount(".", "/", None, libc::MS_MOVE, None) {
        return e;
    }
    if let Err(e) = sys::chroot(".") {
        return e;
    }
    if let Err(e) = std::env::set_current_dir("/") {
        return e;
    }
    log::notice(&format!("switching to {init} at {:.3}s", sys::boottime().as_secs_f64()));
    Command::new(init).arg0(init).env_clear().env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin").exec()
}

fn rescue(reason: &str) -> ! {
    log::error(reason);
    let banner = format!(
        "\n\
         ==================== Lumen: boot failed ====================\n\
         {reason}\n\
         \n\
         Check the root= parameter (press E in the boot menu to edit it).\n\
         A rescue shell follows; exit it to reboot.\n\
         ============================================================\n\n"
    );
    log::console(&banner);
    for shell in ["/bin/lsh", "/bin/sh"] {
        if Path::new(shell).exists() {
            let status = Command::new(shell)
                .env("PATH", "/bin:/sbin")
                .env("PS1", "(rescue) # ")
                .stdin(console())
                .stdout(console())
                .stderr(console())
                .status();
            if status.is_ok() {
                break;
            }
        }
    }
    log::console("lumen-init: rebooting in 5 s\n");
    std::thread::sleep(Duration::from_secs(5));
    sys::sync();
    let err = sys::reboot(libc::RB_AUTOBOOT);
    log::error(&format!("reboot failed: {err}"));
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

fn console() -> std::process::Stdio {
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/console")
        .map(std::process::Stdio::from)
        .unwrap_or_else(|_| std::process::Stdio::null())
}
