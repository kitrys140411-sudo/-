//! Pseudo-terminals: open a pair, run the shell on the slave side as a
//! session leader with it as the controlling terminal.

use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

pub struct Pty {
    pub master: OwnedFd,
    pub child: Child,
}

fn winsize(cols: u16, rows: u16, px_w: u16, px_h: u16) -> libc::winsize {
    libc::winsize { ws_row: rows, ws_col: cols, ws_xpixel: px_w, ws_ypixel: px_h }
}

fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    // SAFETY: plain fcntl calls on a descriptor we own.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags < 0 || libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// The program to run: `$SHELL`, else lsh, else /bin/sh.
pub fn default_shell() -> Vec<String> {
    if let Ok(s) = std::env::var("SHELL") {
        if !s.is_empty() && Path::new(&s).exists() {
            return vec![s];
        }
    }
    for s in ["/usr/bin/lsh", "/bin/lsh", "/bin/sh"] {
        if Path::new(s).exists() {
            return vec![s.to_string()];
        }
    }
    vec!["/bin/sh".into()]
}

impl Pty {
    pub fn spawn(cmd: &[String], cwd: Option<&Path>, size: (u16, u16, u16, u16), extra_env: &[(&str, &str)]) -> io::Result<Pty> {
        let (mut master, mut slave) = (-1, -1);
        let mut ws = winsize(size.0, size.1, size.2, size.3);
        // SAFETY: openpty writes two descriptors; the name buffer and termios
        // pointers may be NULL.
        let r = unsafe { libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null(), &mut ws) };
        if r != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: openpty returned two fresh descriptors that we now own.
        let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
        // SAFETY: setting FD_CLOEXEC on our own descriptor.
        unsafe {
            libc::fcntl(master.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC);
        }
        let program = cmd.first().cloned().unwrap_or_else(|| "/bin/sh".into());
        let mut c = Command::new(&program);
        c.args(&cmd[1..]);
        if let Some(d) = cwd {
            c.current_dir(d);
        }
        c.env("TERM", "xterm-256color")
            .env("COLORTERM", "truecolor")
            .env("TERM_PROGRAM", "lumen-term")
            .env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"));
        for (k, v) in extra_env {
            c.env(k, v);
        }
        c.stdin(Stdio::from(slave.try_clone()?));
        c.stdout(Stdio::from(slave.try_clone()?));
        c.stderr(Stdio::from(slave));
        // SAFETY: only async-signal-safe calls between fork and exec.
        unsafe {
            c.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(io::Error::last_os_error());
                }
                if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                // Default signal dispositions for the shell.
                for s in [libc::SIGCHLD, libc::SIGHUP, libc::SIGINT, libc::SIGQUIT, libc::SIGTERM, libc::SIGALRM, libc::SIGPIPE] {
                    libc::signal(s, libc::SIG_DFL);
                }
                Ok(())
            });
        }
        let child = c.spawn()?;
        set_nonblocking(master.as_raw_fd())?;
        Ok(Pty { master, child })
    }

    pub fn resize(&self, cols: u16, rows: u16, px_w: u16, px_h: u16) {
        let ws = winsize(cols, rows, px_w, px_h);
        // SAFETY: TIOCSWINSZ with a valid winsize on the master.
        unsafe {
            libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ as _, &ws);
        }
    }

    /// Non-blocking read; Ok(0) means the slave side is gone (EIO on Linux
    /// is mapped to 0 as well).
    pub fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        // SAFETY: reading into a valid buffer.
        let n = unsafe { libc::read(self.master.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.raw_os_error() == Some(libc::EIO) {
                return Ok(0);
            }
            return Err(e);
        }
        Ok(n as usize)
    }

    pub fn write(&self, data: &[u8]) -> io::Result<usize> {
        // SAFETY: writing from a valid buffer.
        let n = unsafe { libc::write(self.master.as_raw_fd(), data.as_ptr().cast(), data.len()) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(n as usize)
    }

    /// Working directory of the foreground process (for new tabs/splits).
    pub fn foreground_cwd(&self) -> Option<PathBuf> {
        // SAFETY: tcgetpgrp on our master descriptor.
        let pgrp = unsafe { libc::tcgetpgrp(self.master.as_raw_fd()) };
        let pid = if pgrp > 0 { pgrp as u32 } else { self.child.id() };
        std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
    }

    /// Name of the foreground process (window title fallback).
    pub fn foreground_name(&self) -> Option<String> {
        // SAFETY: as above.
        let pgrp = unsafe { libc::tcgetpgrp(self.master.as_raw_fd()) };
        let pid = if pgrp > 0 { pgrp as u32 } else { self.child.id() };
        std::fs::read_to_string(format!("/proc/{pid}/comm")).ok().map(|s| s.trim().to_string())
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        // Hang up the session like closing a real terminal would.
        // SAFETY: signalling our child's process group.
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGHUP);
        }
        let _ = self.child.try_wait();
    }
}

/// Helper for C strings in tests and tools.
pub fn cstr(s: &str) -> CString {
    CString::new(s).unwrap_or_default()
}
