//! Thin safe wrappers over the libc calls PID 1 needs.

use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::time::Duration;

fn cstr(s: &str) -> io::Result<CString> {
    CString::new(s).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))
}

fn check(ret: libc::c_int) -> io::Result<()> {
    if ret < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
}

pub fn mount(src: &str, target: &str, fstype: Option<&str>, flags: libc::c_ulong, data: Option<&str>) -> io::Result<()> {
    let src = cstr(src)?;
    let target = cstr(target)?;
    let fstype = fstype.map(cstr).transpose()?;
    let data = data.map(cstr).transpose()?;
    // SAFETY: all pointers are valid NUL-terminated strings or null.
    check(unsafe {
        libc::mount(
            src.as_ptr(),
            target.as_ptr(),
            fstype.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
            flags,
            data.as_ref().map_or(std::ptr::null(), |s| s.as_ptr().cast()),
        )
    })
}

pub fn umount(target: &str, flags: libc::c_int) -> io::Result<()> {
    let target = cstr(target)?;
    // SAFETY: valid C string.
    check(unsafe { libc::umount2(target.as_ptr(), flags) })
}

/// Mount `fstype` on `target` unless something is already mounted there.
pub fn mount_once(fstype: &str, target: &str, flags: libc::c_ulong, data: Option<&str>) -> io::Result<()> {
    let _ = std::fs::create_dir_all(target);
    if is_mountpoint(target) {
        return Ok(());
    }
    mount(fstype, target, Some(fstype), flags, data)
}

pub fn is_mountpoint(path: &str) -> bool {
    let Ok(mounts) = std::fs::read_to_string("/proc/self/mounts") else {
        return false;
    };
    mounts.lines().any(|l| l.split_whitespace().nth(1) == Some(path))
}

pub fn chown(path: &str, uid: u32, gid: u32) -> io::Result<()> {
    std::os::unix::fs::chown(path, Some(uid), Some(gid))
}

pub fn chmod(path: &str, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

pub fn chroot(path: &str) -> io::Result<()> {
    let p = cstr(path)?;
    // SAFETY: valid C string.
    check(unsafe { libc::chroot(p.as_ptr()) })
}

pub fn sync() {
    // SAFETY: no arguments.
    unsafe { libc::sync() };
}

/// reboot(2); only returns on failure.
pub fn reboot(cmd: libc::c_int) -> io::Error {
    // SAFETY: plain syscall.
    unsafe { libc::reboot(cmd) };
    io::Error::last_os_error()
}

pub fn disable_ctrl_alt_del() {
    // SAFETY: with CAD_OFF the kernel sends SIGINT to PID 1 instead of rebooting.
    unsafe { libc::reboot(libc::LINUX_REBOOT_CMD_CAD_OFF) };
}

pub fn kill(pid: i32, sig: libc::c_int) -> io::Result<()> {
    // SAFETY: plain syscall.
    check(unsafe { libc::kill(pid, sig) })
}

pub fn sethostname(name: &str) -> io::Result<()> {
    // SAFETY: pointer/length pair of a live buffer.
    check(unsafe { libc::sethostname(name.as_ptr().cast(), name.len()) })
}

/// Time since boot, including suspend.
pub fn boottime() -> Duration {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: valid out pointer.
    unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut ts) };
    Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32)
}

pub struct SigSet(libc::sigset_t);

impl SigSet {
    pub fn of(signals: &[libc::c_int]) -> Self {
        // SAFETY: sigset_t is plain data initialized by sigemptyset.
        unsafe {
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            for &s in signals {
                libc::sigaddset(&mut set, s);
            }
            SigSet(set)
        }
    }

    pub fn block(&self) {
        // SAFETY: valid set.
        unsafe { libc::sigprocmask(libc::SIG_BLOCK, &self.0, std::ptr::null_mut()) };
    }

    pub fn signalfd(&self) -> io::Result<OwnedFd> {
        // SAFETY: valid set; returns a new fd we own.
        let fd = unsafe { libc::signalfd(-1, &self.0, libc::SFD_NONBLOCK | libc::SFD_CLOEXEC) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fresh fd.
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }
}

/// Restore default dispositions and an empty mask (in a freshly forked child).
pub fn reset_signals_in_child() {
    // SAFETY: async-signal-safe calls only.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigprocmask(libc::SIG_SETMASK, &set, std::ptr::null_mut());
        for sig in 1..32 {
            if sig != libc::SIGKILL && sig != libc::SIGSTOP {
                libc::signal(sig, libc::SIG_DFL);
            }
        }
    }
}

/// Drain a signalfd; returns the signal numbers read.
pub fn read_signals(fd: &OwnedFd) -> Vec<u32> {
    let mut out = Vec::new();
    loop {
        // SAFETY: signalfd_siginfo is plain data.
        let mut info: libc::signalfd_siginfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::signalfd_siginfo>();
        // SAFETY: buffer of exactly `size` bytes.
        let n = unsafe { libc::read(fd.as_raw_fd(), (&mut info as *mut libc::signalfd_siginfo).cast(), size) };
        if n as usize != size {
            break;
        }
        out.push(info.ssi_signo);
    }
    out
}

pub enum Exit {
    Code(i32),
    Signal(i32),
}

/// Reap one exited child, if any (non-blocking).
pub fn reap() -> Option<(i32, Exit)> {
    let mut status = 0;
    // SAFETY: valid out pointer.
    let pid = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
    if pid <= 0 {
        return None;
    }
    let exit = if libc::WIFEXITED(status) {
        Exit::Code(libc::WEXITSTATUS(status))
    } else {
        Exit::Signal(libc::WTERMSIG(status))
    };
    Some((pid, exit))
}

pub fn poll(fds: &mut [libc::pollfd], timeout: Option<Duration>) -> usize {
    let ms = timeout.map_or(-1, |d| d.as_millis().min(i32::MAX as u128) as i32);
    // SAFETY: valid slice of pollfd.
    let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, ms) };
    n.max(0) as usize
}

pub fn pollfd(fd: RawFd) -> libc::pollfd {
    libc::pollfd { fd, events: libc::POLLIN, revents: 0 }
}

/// Bring the loopback interface up (SIOCSIFFLAGS IFF_UP).
pub fn loopback_up() -> io::Result<()> {
    // SAFETY: socket/ioctl with a properly initialized ifreq.
    unsafe {
        let s = libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0);
        if s < 0 {
            return Err(io::Error::last_os_error());
        }
        let s = OwnedFd::from_raw_fd(s);
        let mut req: libc::ifreq = std::mem::zeroed();
        for (i, b) in b"lo".iter().enumerate() {
            req.ifr_name[i] = *b as libc::c_char;
        }
        check(libc::ioctl(s.as_raw_fd(), libc::SIOCGIFFLAGS as _, &mut req))?;
        req.ifr_ifru.ifru_flags |= (libc::IFF_UP | libc::IFF_RUNNING) as libc::c_short;
        check(libc::ioctl(s.as_raw_fd(), libc::SIOCSIFFLAGS as _, &req))
    }
}

/// Make `fd` (a tty) the controlling terminal of the calling session leader.
pub fn set_controlling_tty(fd: RawFd) -> io::Result<()> {
    // SAFETY: ioctl on an fd we own.
    check(unsafe { libc::ioctl(fd, libc::TIOCSCTTY as _, 1) })
}

pub fn setsid() -> io::Result<()> {
    // SAFETY: plain syscall.
    if unsafe { libc::setsid() } < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
}

/// Drop privileges; async-signal-safe (no allocation), for use after fork.
pub fn set_ids(uid: u32, gid: u32, groups: &[libc::gid_t]) -> io::Result<()> {
    // SAFETY: plain syscalls with a valid groups buffer.
    unsafe {
        check(libc::setgroups(groups.len(), groups.as_ptr()))?;
        check(libc::setgid(gid))?;
        check(libc::setuid(uid))?;
    }
    Ok(())
}
