//! Thin wrappers over the POSIX calls the shell needs.

use std::ffi::{CString, OsStr};
use std::io;
use std::os::unix::ffi::OsStrExt;

pub type Fd = libc::c_int;

fn check(r: libc::c_int) -> io::Result<libc::c_int> {
    if r < 0 { Err(io::Error::last_os_error()) } else { Ok(r) }
}

fn retry<F: FnMut() -> libc::c_int>(mut f: F) -> io::Result<libc::c_int> {
    loop {
        let r = f();
        if r >= 0 {
            return Ok(r);
        }
        let e = io::Error::last_os_error();
        if e.raw_os_error() != Some(libc::EINTR) {
            return Err(e);
        }
    }
}

pub fn cstring(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap_or_default()
}

pub fn pipe() -> io::Result<(Fd, Fd)> {
    let mut fds = [0; 2];
    // SAFETY: valid 2-element buffer.
    check(unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) })?;
    Ok((fds[0], fds[1]))
}

pub fn dup2(from: Fd, to: Fd) -> io::Result<()> {
    if from == to {
        // Clear close-on-exec so the fd survives exec.
        // SAFETY: fcntl on an fd.
        unsafe { libc::fcntl(to, libc::F_SETFD, 0) };
        return Ok(());
    }
    // SAFETY: plain syscall.
    retry(|| unsafe { libc::dup2(from, to) }).map(|_| ())
}

/// Duplicate to a high, close-on-exec fd (for saving fds around redirections).
pub fn dup_high(fd: Fd) -> io::Result<Fd> {
    // SAFETY: plain syscall.
    check(unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 10) })
}

pub fn close(fd: Fd) {
    // SAFETY: closing an fd we own.
    unsafe { libc::close(fd) };
}

pub fn is_open(fd: Fd) -> bool {
    // SAFETY: F_GETFD has no side effects.
    unsafe { libc::fcntl(fd, libc::F_GETFD) >= 0 }
}

pub fn open(path: &str, flags: libc::c_int, mode: libc::mode_t) -> io::Result<Fd> {
    let p = cstring(path);
    // SAFETY: valid C string.
    retry(|| unsafe { libc::open(p.as_ptr(), flags | libc::O_CLOEXEC, mode as libc::c_uint) })
}

pub fn write_all(fd: Fd, mut data: &[u8]) -> io::Result<()> {
    while !data.is_empty() {
        // SAFETY: valid buffer.
        let n = unsafe { libc::write(fd, data.as_ptr().cast(), data.len()) };
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return Err(e);
        }
        data = &data[n as usize..];
    }
    Ok(())
}

pub fn read(fd: Fd, buf: &mut [u8]) -> io::Result<usize> {
    // SAFETY: valid buffer.
    retry(|| unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) as libc::c_int }).map(|n| n as usize)
}

/// File descriptor containing `data`, positioned at the start (here-documents).
pub fn memfd_with(data: &[u8]) -> io::Result<Fd> {
    let name = cstring("lsh-heredoc");
    // SAFETY: valid C string.
    let fd = check(unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) })?;
    write_all(fd, data)?;
    // SAFETY: plain syscall.
    unsafe { libc::lseek(fd, 0, libc::SEEK_SET) };
    Ok(fd)
}

pub fn fork() -> io::Result<libc::pid_t> {
    // SAFETY: the shell is single-threaded.
    let pid = unsafe { libc::fork() };
    if pid < 0 { Err(io::Error::last_os_error()) } else { Ok(pid) }
}

pub fn exit_now(code: i32) -> ! {
    // SAFETY: _exit skips atexit handlers and stdio flushing of the parent's buffers.
    unsafe { libc::_exit(code) }
}

pub fn getpid() -> libc::pid_t {
    // SAFETY: plain syscall.
    unsafe { libc::getpid() }
}

pub fn setpgid(pid: libc::pid_t, pgid: libc::pid_t) {
    // SAFETY: plain syscall; failure (e.g. after exec) is harmless.
    unsafe { libc::setpgid(pid, pgid) };
}

pub fn getpgrp() -> libc::pid_t {
    // SAFETY: plain syscall.
    unsafe { libc::getpgrp() }
}

pub fn tcgetpgrp(fd: Fd) -> libc::pid_t {
    // SAFETY: plain syscall.
    unsafe { libc::tcgetpgrp(fd) }
}

pub fn tcsetpgrp(fd: Fd, pgid: libc::pid_t) {
    // SAFETY: plain syscall; SIGTTOU is ignored by the interactive shell.
    unsafe { libc::tcsetpgrp(fd, pgid) };
}

pub fn isatty(fd: Fd) -> bool {
    // SAFETY: plain syscall.
    unsafe { libc::isatty(fd) == 1 }
}

pub fn kill(pid: libc::pid_t, sig: libc::c_int) -> io::Result<()> {
    // SAFETY: plain syscall.
    check(unsafe { libc::kill(pid, sig) }).map(|_| ())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitStatus {
    Exited(i32),
    Signaled(i32, bool),
    Stopped(i32),
    Continued,
}

impl WaitStatus {
    /// Shell exit status ($?).
    pub fn code(self) -> i32 {
        match self {
            WaitStatus::Exited(c) => c,
            WaitStatus::Signaled(s, _) => 128 + s,
            WaitStatus::Stopped(s) => 128 + s,
            WaitStatus::Continued => 0,
        }
    }
}

pub fn waitpid(pid: libc::pid_t, flags: libc::c_int) -> Option<(libc::pid_t, WaitStatus)> {
    let mut status = 0;
    loop {
        // SAFETY: valid out pointer.
        let r = unsafe { libc::waitpid(pid, &mut status, flags) };
        if r < 0 && io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
            continue;
        }
        if r <= 0 {
            return None;
        }
        let ws = if libc::WIFEXITED(status) {
            WaitStatus::Exited(libc::WEXITSTATUS(status))
        } else if libc::WIFSIGNALED(status) {
            WaitStatus::Signaled(libc::WTERMSIG(status), libc::WCOREDUMP(status))
        } else if libc::WIFSTOPPED(status) {
            WaitStatus::Stopped(libc::WSTOPSIG(status))
        } else {
            WaitStatus::Continued
        };
        return Some((r, ws));
    }
}

pub fn set_signal(sig: libc::c_int, handler: libc::sighandler_t) {
    // SAFETY: installing SIG_IGN/SIG_DFL or an async-signal-safe handler.
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = handler;
        libc::sigemptyset(&mut sa.sa_mask);
        sa.sa_flags = libc::SA_RESTART;
        libc::sigaction(sig, &sa, std::ptr::null_mut());
    }
}

/// Signals the interactive shell ignores while it owns the terminal.
pub const JOB_SIGNALS: [libc::c_int; 5] = [libc::SIGINT, libc::SIGQUIT, libc::SIGTSTP, libc::SIGTTIN, libc::SIGTTOU];

/// Restore default signal dispositions in a child before exec.
pub fn reset_child_signals() {
    for s in JOB_SIGNALS.iter().chain([libc::SIGCHLD, libc::SIGPIPE, libc::SIGTERM, libc::SIGHUP].iter()) {
        set_signal(*s, libc::SIG_DFL);
    }
    // SAFETY: plain syscall with an empty set.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigprocmask(libc::SIG_SETMASK, &set, std::ptr::null_mut());
    }
}

pub fn execve(path: &str, argv: &[String], env: &[String]) -> io::Error {
    let path_c = cstring(path);
    let argv_c: Vec<CString> = argv.iter().map(|a| cstring(a)).collect();
    let env_c: Vec<CString> = env.iter().map(|a| cstring(a)).collect();
    let mut argv_p: Vec<*const libc::c_char> = argv_c.iter().map(|c| c.as_ptr()).collect();
    argv_p.push(std::ptr::null());
    let mut env_p: Vec<*const libc::c_char> = env_c.iter().map(|c| c.as_ptr()).collect();
    env_p.push(std::ptr::null());
    // SAFETY: NULL-terminated arrays of valid C strings.
    unsafe { libc::execve(path_c.as_ptr(), argv_p.as_ptr(), env_p.as_ptr()) };
    io::Error::last_os_error()
}

pub fn access_x(path: &str) -> bool {
    let p = cstring(path);
    // SAFETY: valid C string.
    unsafe { libc::access(p.as_ptr(), libc::X_OK) == 0 }
}

pub fn umask(mask: Option<libc::mode_t>) -> libc::mode_t {
    // SAFETY: plain syscall.
    unsafe {
        let old = libc::umask(mask.unwrap_or(0o022));
        if mask.is_none() {
            libc::umask(old);
        }
        old
    }
}

pub type Termios = libc::termios;

pub fn get_termios(fd: Fd) -> Option<Termios> {
    // SAFETY: termios is plain data.
    unsafe {
        let mut t: libc::termios = std::mem::zeroed();
        (libc::tcgetattr(fd, &mut t) == 0).then_some(t)
    }
}

pub fn set_termios(fd: Fd, t: &Termios) {
    // SAFETY: valid termios.
    unsafe { libc::tcsetattr(fd, libc::TCSADRAIN, t) };
}

/// Raw mode for the line editor: no echo, no canonical input, no signals from
/// keys (Ctrl+C/Z arrive as bytes), output processing kept.
pub fn raw_termios(t: &Termios) -> Termios {
    let mut r = *t;
    r.c_lflag &= !(libc::ECHO | libc::ICANON | libc::ISIG | libc::IEXTEN);
    r.c_iflag &= !(libc::IXON | libc::ICRNL | libc::INLCR | libc::IGNCR);
    r.c_cc[libc::VMIN] = 1;
    r.c_cc[libc::VTIME] = 0;
    r
}

pub fn terminal_width(fd: Fd) -> usize {
    // SAFETY: winsize is plain data.
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) == 0 && ws.ws_col > 0 { ws.ws_col as usize } else { 80 }
    }
}

/// Wait until `fd` is readable or `timeout_ms` passes. Returns true if readable.
pub fn poll_read(fd: Fd, timeout_ms: i32) -> bool {
    let mut p = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
    // SAFETY: one valid pollfd.
    unsafe { libc::poll(&mut p, 1, timeout_ms) > 0 }
}

pub fn signal_name(sig: i32) -> &'static str {
    match sig {
        libc::SIGHUP => "HUP",
        libc::SIGINT => "INT",
        libc::SIGQUIT => "QUIT",
        libc::SIGILL => "ILL",
        libc::SIGTRAP => "TRAP",
        libc::SIGABRT => "ABRT",
        libc::SIGBUS => "BUS",
        libc::SIGFPE => "FPE",
        libc::SIGKILL => "KILL",
        libc::SIGUSR1 => "USR1",
        libc::SIGSEGV => "SEGV",
        libc::SIGUSR2 => "USR2",
        libc::SIGPIPE => "PIPE",
        libc::SIGALRM => "ALRM",
        libc::SIGTERM => "TERM",
        libc::SIGCHLD => "CHLD",
        libc::SIGCONT => "CONT",
        libc::SIGSTOP => "STOP",
        libc::SIGTSTP => "TSTP",
        libc::SIGTTIN => "TTIN",
        libc::SIGTTOU => "TTOU",
        libc::SIGWINCH => "WINCH",
        _ => "",
    }
}

pub fn signal_number(name: &str) -> Option<i32> {
    let n = name.trim_start_matches("SIG");
    if let Ok(v) = n.parse::<i32>() {
        return Some(v);
    }
    (1..32).find(|&s| signal_name(s).eq_ignore_ascii_case(n))
}

/// Description printed when a foreground job dies from a signal.
pub fn signal_description(sig: i32) -> Option<&'static str> {
    Some(match sig {
        libc::SIGSEGV => "Segmentation fault",
        libc::SIGABRT => "Aborted",
        libc::SIGKILL => "Killed",
        libc::SIGBUS => "Bus error",
        libc::SIGFPE => "Floating point exception",
        libc::SIGILL => "Illegal instruction",
        libc::SIGTERM => "Terminated",
        libc::SIGQUIT => "Quit",
        libc::SIGHUP => "Hangup",
        libc::SIGALRM => "Alarm clock",
        _ => return None,
    })
}

pub fn os(s: &str) -> &OsStr {
    OsStr::from_bytes(s.as_bytes())
}
