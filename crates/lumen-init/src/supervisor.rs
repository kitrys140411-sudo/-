//! Stage 2: PID 1 on the real root filesystem.
//!
//! Mounts the API filesystems, starts services in dependency order, supervises
//! them (restart with backoff), reaps every zombie, and performs an orderly
//! shutdown on request: SIGTERM to services → grace period → SIGKILL to
//! everything → sync → read-only remount → reboot(2).
//!
//! Requests arrive as signals (busybox-compatible: SIGTERM reboot, SIGUSR1 halt,
//! SIGUSR2 poweroff, SIGINT = Ctrl+Alt+Del reboot) or as text commands on the
//! control socket `/run/lumen/control` (used by `lumenctl`). Services can report
//! readiness by sending `READY=1` to the datagram socket `/run/lumen/notify`.

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixDatagram, UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::service::{self, Kind, Ready, Restart, ServiceDef};
use crate::{cmdline, log, sys, users};

pub const CONTROL_SOCKET: &str = "/run/lumen/control";
pub const NOTIFY_SOCKET: &str = "/run/lumen/notify";
const SERVICE_DIR: &str = "/etc/lumen/services";
const STOP_GRACE: Duration = Duration::from_secs(4);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Waiting,
    Running,
    /// Oneshot finished successfully.
    Done,
    /// Waiting for a restart deadline.
    Backoff,
    /// Gave up (or restart = never and it exited).
    Failed,
    Stopped,
}

struct Svc {
    def: ServiceDef,
    state: State,
    pid: Option<i32>,
    ready: bool,
    started: Option<Instant>,
    fast_failures: u32,
    restart_at: Option<Instant>,
    waiting_since: Instant,
    last_exit: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shutdown {
    Poweroff,
    Reboot,
    Halt,
}

struct Init {
    services: Vec<Svc>,
    target: String,
    shutdown: Option<Shutdown>,
    booted: Instant,
}

fn setup_filesystems() {
    let nosuid_nodev = libc::MS_NOSUID | libc::MS_NODEV;
    let _ = sys::mount_once("proc", "/proc", nosuid_nodev | libc::MS_NOEXEC, None);
    let _ = sys::mount_once("sysfs", "/sys", nosuid_nodev | libc::MS_NOEXEC, None);
    let _ = sys::mount_once("devtmpfs", "/dev", libc::MS_NOSUID, Some("mode=0755"));
    let _ = sys::mount_once("devpts", "/dev/pts", libc::MS_NOSUID | libc::MS_NOEXEC, Some("gid=5,mode=620,ptmxmode=666"));
    let _ = sys::mount_once("tmpfs", "/dev/shm", nosuid_nodev, Some("mode=1777"));
    let _ = sys::mount_once("tmpfs", "/run", nosuid_nodev, Some("mode=0755"));
    let _ = sys::mount_once("tmpfs", "/tmp", nosuid_nodev, Some("mode=1777"));
    // The root filesystem comes up writable from stage 1 unless `ro` was given.
    for dir in ["/run/lumen", "/run/log", "/run/user"] {
        let _ = fs::create_dir_all(dir);
    }
    let _ = sys::chmod("/run/user", 0o755);
    let _ = std::os::unix::fs::symlink("/proc/self/fd", "/dev/fd");
    let _ = std::os::unix::fs::symlink("/proc/self/fd/0", "/dev/stdin");
    let _ = std::os::unix::fs::symlink("/proc/self/fd/1", "/dev/stdout");
    let _ = std::os::unix::fs::symlink("/proc/self/fd/2", "/dev/stderr");
}

fn setup_system() {
    let hostname = fs::read_to_string("/etc/hostname").unwrap_or_default();
    let hostname = hostname.trim();
    if !hostname.is_empty() {
        let _ = sys::sethostname(hostname);
    }
    if let Err(e) = sys::loopback_up() {
        log::warn(&format!("loopback: {e}"));
    }
}

pub fn run() -> ! {
    let booted = Instant::now();
    setup_filesystems();
    log::open();
    log::notice(&format!("stage 2 at {:.3}s", sys::boottime().as_secs_f64()));
    setup_system();
    sys::disable_ctrl_alt_del();

    let signals = sys::SigSet::of(&[
        libc::SIGCHLD,
        libc::SIGTERM,
        libc::SIGINT,
        libc::SIGUSR1,
        libc::SIGUSR2,
        libc::SIGHUP,
        libc::SIGPWR,
    ]);
    signals.block();
    let sigfd = signals.signalfd().ok();
    if sigfd.is_none() {
        log::error("signalfd failed; signals will not be handled");
    }

    let control = bind_control();
    let notify = bind_notify();

    let args = cmdline::parse(&fs::read_to_string("/proc/cmdline").unwrap_or_default());
    let target = cmdline::get(&args, "lumen.target").unwrap_or("graphical").to_string();
    let (defs, errors) = service::load_dir(SERVICE_DIR);
    for e in errors {
        log::error(&e);
    }
    let now = Instant::now();
    let services: Vec<Svc> = defs
        .into_iter()
        .filter(|d| d.wanted_by(&target))
        .map(|def| Svc {
            def,
            state: State::Waiting,
            pid: None,
            ready: false,
            started: None,
            fast_failures: 0,
            restart_at: None,
            waiting_since: now,
            last_exit: None,
        })
        .collect();
    log::notice(&format!(
        "target {target}: {}",
        services.iter().map(|s| s.def.name.as_str()).collect::<Vec<_>>().join(" ")
    ));
    let mut init = Init { services, target, shutdown: None, booted };
    init.main_loop(sigfd, control, notify)
}

fn bind_control() -> Option<UnixListener> {
    let _ = fs::remove_file(CONTROL_SOCKET);
    match UnixListener::bind(CONTROL_SOCKET) {
        Ok(l) => {
            let _ = l.set_nonblocking(true);
            // Members of group "power" (the desktop user) may shut the machine down.
            let gid = fs::read_to_string("/etc/group").ok().and_then(|g| users::group_gid(&g, "power")).unwrap_or(0);
            let _ = sys::chown(CONTROL_SOCKET, 0, gid);
            let _ = sys::chmod(CONTROL_SOCKET, 0o660);
            Some(l)
        }
        Err(e) => {
            log::error(&format!("control socket: {e}"));
            None
        }
    }
}

fn bind_notify() -> Option<UnixDatagram> {
    let _ = fs::remove_file(NOTIFY_SOCKET);
    match UnixDatagram::bind(NOTIFY_SOCKET) {
        Ok(s) => {
            let _ = s.set_nonblocking(true);
            let _ = sys::chmod(NOTIFY_SOCKET, 0o666);
            let one: libc::c_int = 1;
            // SAFETY: setsockopt with a valid int option.
            unsafe {
                libc::setsockopt(
                    s.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_PASSCRED,
                    (&one as *const libc::c_int).cast(),
                    std::mem::size_of::<libc::c_int>() as libc::socklen_t,
                )
            };
            Some(s)
        }
        Err(e) => {
            log::error(&format!("notify socket: {e}"));
            None
        }
    }
}

/// Receive one datagram with the sender's credentials.
fn recv_with_pid(sock: &UnixDatagram) -> Option<(String, i32)> {
    let mut buf = [0u8; 1024];
    let mut cmsg = [0u8; 64];
    let mut iov = libc::iovec { iov_base: buf.as_mut_ptr().cast(), iov_len: buf.len() };
    // SAFETY: msghdr points at live buffers for the duration of recvmsg.
    unsafe {
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = cmsg.as_mut_ptr().cast();
        msg.msg_controllen = cmsg.len() as _;
        let n = libc::recvmsg(sock.as_raw_fd(), &mut msg, libc::MSG_DONTWAIT);
        if n < 0 {
            return None;
        }
        let mut pid = 0;
        let mut c = libc::CMSG_FIRSTHDR(&msg);
        while !c.is_null() {
            if (*c).cmsg_level == libc::SOL_SOCKET && (*c).cmsg_type == libc::SCM_CREDENTIALS {
                let cred = libc::CMSG_DATA(c) as *const libc::ucred;
                pid = (*cred).pid;
            }
            c = libc::CMSG_NXTHDR(&msg, c);
        }
        Some((String::from_utf8_lossy(&buf[..n as usize]).into_owned(), pid))
    }
}

fn describe_exit(e: &sys::Exit) -> String {
    match e {
        sys::Exit::Code(c) => format!("exit status {c}"),
        sys::Exit::Signal(s) => format!("signal {s}"),
    }
}

impl Init {
    fn main_loop(&mut self, sigfd: Option<OwnedFd>, control: Option<UnixListener>, notify: Option<UnixDatagram>) -> ! {
        loop {
            self.reap_all();
            if let Some(kind) = self.shutdown {
                self.shutdown(kind, sigfd.as_ref());
            }
            self.update_readiness();
            self.start_eligible();

            let mut fds = Vec::new();
            if let Some(fd) = &sigfd {
                fds.push(sys::pollfd(fd.as_raw_fd()));
            }
            if let Some(c) = &control {
                fds.push(sys::pollfd(c.as_raw_fd()));
            }
            if let Some(n) = &notify {
                fds.push(sys::pollfd(n.as_raw_fd()));
            }
            sys::poll(&mut fds, self.next_timeout());

            if let Some(fd) = &sigfd {
                for sig in sys::read_signals(fd) {
                    self.on_signal(sig as i32);
                }
            }
            if let Some(c) = &control {
                while let Ok((stream, _)) = c.accept() {
                    self.on_control(stream);
                }
            }
            if let Some(n) = &notify {
                while let Some((msg, pid)) = recv_with_pid(n) {
                    self.on_notify(&msg, pid);
                }
            }
        }
    }

    fn on_signal(&mut self, sig: i32) {
        let request = match sig {
            libc::SIGTERM | libc::SIGINT => Some(Shutdown::Reboot),
            libc::SIGUSR2 | libc::SIGPWR => Some(Shutdown::Poweroff),
            libc::SIGUSR1 => Some(Shutdown::Halt),
            _ => None, // SIGCHLD is handled by reap_all(); SIGHUP ignored.
        };
        if let Some(r) = request {
            log::notice(&format!("signal {sig}: {r:?} requested"));
            self.shutdown.get_or_insert(r);
        }
    }

    /// Next wake-up: restart deadlines and polling for path readiness.
    fn next_timeout(&self) -> Option<Duration> {
        let now = Instant::now();
        let mut t: Option<Duration> = None;
        let mut take = |d: Duration| t = Some(t.map_or(d, |x| x.min(d)));
        for s in &self.services {
            if let Some(at) = s.restart_at {
                take(at.saturating_duration_since(now));
            }
            if s.state == State::Running && !s.ready && matches!(s.def.ready, Ready::Path(_)) {
                take(Duration::from_millis(10));
            }
            if s.state == State::Waiting {
                // Re-check dependency timeouts once a second.
                take(Duration::from_secs(1));
            }
        }
        t
    }

    fn index_by_pid(&self, pid: i32) -> Option<usize> {
        self.services.iter().position(|s| s.pid == Some(pid))
    }

    fn reap_all(&mut self) {
        while let Some((pid, exit)) = sys::reap() {
            let Some(i) = self.index_by_pid(pid) else {
                continue; // orphan reparented to PID 1
            };
            let now = Instant::now();
            let s = &mut self.services[i];
            s.pid = None;
            let ran = s.started.map(|t| now - t).unwrap_or_default();
            let success = matches!(exit, sys::Exit::Code(0));
            s.last_exit = Some(describe_exit(&exit));
            if self.shutdown.is_some() || s.state == State::Stopped {
                s.state = State::Stopped;
                continue;
            }
            if s.def.kind == Kind::Oneshot && success {
                s.state = State::Done;
                s.ready = true;
                log::info(&format!("{} finished ({:.3}s)", s.def.name, ran.as_secs_f64()));
                continue;
            }
            let restart = match s.def.restart {
                Restart::Always => true,
                Restart::OnFailure => !success,
                Restart::Never => false,
            };
            let msg = format!("{} exited ({}) after {:.1}s", s.def.name, describe_exit(&exit), ran.as_secs_f64());
            if success { log::notice(&msg) } else { log::warn(&msg) }
            s.ready = false;
            if !restart {
                s.state = if success { State::Stopped } else { State::Failed };
                continue;
            }
            if ran < Duration::from_secs(5) {
                s.fast_failures += 1;
            } else {
                s.fast_failures = 0;
            }
            if s.fast_failures > 8 {
                log::error(&format!("{} keeps failing; giving up", s.def.name));
                s.state = State::Failed;
                continue;
            }
            let delay = Duration::from_millis(200u64 << s.fast_failures.min(6)); // 0.2 s .. 12.8 s
            s.state = State::Backoff;
            s.restart_at = Some(now + delay);
        }
    }

    fn update_readiness(&mut self) {
        let booted = self.booted;
        for s in &mut self.services {
            if s.state == State::Running && !s.ready {
                if let Ready::Path(p) = &s.def.ready {
                    if Path::new(p).exists() {
                        s.ready = true;
                        log::notice(&format!(
                            "{} ready at {:.3}s (+{:.3}s)",
                            s.def.name,
                            sys::boottime().as_secs_f64(),
                            booted.elapsed().as_secs_f64()
                        ));
                    }
                }
            }
        }
    }

    fn deps_satisfied(&self, s: &Svc) -> bool {
        s.def.after.iter().all(|dep| match self.services.iter().find(|o| &o.def.name == dep) {
            None => true, // not part of this target
            Some(o) => o.ready || matches!(o.state, State::Failed | State::Stopped) || s.waiting_since.elapsed() > Duration::from_secs(15),
        })
    }

    fn start_eligible(&mut self) {
        let now = Instant::now();
        for i in 0..self.services.len() {
            let s = &self.services[i];
            let due = match s.state {
                State::Waiting => self.deps_satisfied(s),
                State::Backoff => s.restart_at.is_some_and(|t| t <= now),
                _ => false,
            };
            if !due {
                continue;
            }
            if s.state == State::Waiting {
                for dep in &s.def.after {
                    if let Some(o) = self.services.iter().find(|o| &o.def.name == dep) {
                        if !o.ready {
                            log::warn(&format!("{}: starting without dependency {dep} ({:?})", s.def.name, o.state));
                        }
                    }
                }
            }
            self.spawn(i);
        }
    }

    fn spawn(&mut self, i: usize) {
        let s = &mut self.services[i];
        s.restart_at = None;
        match spawn_service(&s.def) {
            Ok(pid) => {
                s.pid = Some(pid);
                s.state = State::Running;
                s.started = Some(Instant::now());
                s.ready = s.def.ready == Ready::Start && s.def.kind == Kind::Simple;
                log::notice(&format!("started {} (pid {pid}) at {:.3}s", s.def.name, sys::boottime().as_secs_f64()));
            }
            Err(e) => {
                log::error(&format!("cannot start {}: {e}", s.def.name));
                s.fast_failures += 1;
                if s.fast_failures > 8 || s.def.restart == Restart::Never {
                    s.state = State::Failed;
                } else {
                    s.state = State::Backoff;
                    s.restart_at = Some(Instant::now() + Duration::from_secs(1));
                }
            }
        }
    }

    fn on_notify(&mut self, msg: &str, pid: i32) {
        // The sender may be the main process or anything in its process group.
        // SAFETY: getpgid has no memory effects.
        let pgid = unsafe { libc::getpgid(pid) };
        let booted = self.booted;
        let Some(s) = self.services.iter_mut().find(|s| s.pid.is_some_and(|p| p == pid || p == pgid)) else {
            log::warn(&format!("notify from unknown pid {pid}: {}", msg.trim()));
            return;
        };
        for line in msg.lines() {
            if line == "READY=1" && !s.ready {
                s.ready = true;
                log::notice(&format!(
                    "{} ready at {:.3}s (+{:.3}s since init)",
                    s.def.name,
                    sys::boottime().as_secs_f64(),
                    booted.elapsed().as_secs_f64()
                ));
            } else if let Some(status) = line.strip_prefix("STATUS=") {
                log::info(&format!("{}: {status}", s.def.name));
            }
        }
    }

    fn on_control(&mut self, stream: UnixStream) {
        let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
        let _ = stream.set_nonblocking(false);
        let mut reader = BufReader::new(&stream);
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() {
            return;
        }
        let mut words = line.split_whitespace();
        let reply = match (words.next(), words.next()) {
            (Some("poweroff"), _) => self.request(Shutdown::Poweroff),
            (Some("reboot"), _) => self.request(Shutdown::Reboot),
            (Some("halt"), _) => self.request(Shutdown::Halt),
            (Some("status"), _) => self.status(),
            (Some("restart"), Some(name)) => self.restart(name),
            (Some("stop"), Some(name)) => self.stop(name),
            (Some("start"), Some(name)) => self.start(name),
            (Some("log"), _) => {
                let text = line.trim_start().strip_prefix("log").unwrap_or("").trim();
                log::info(&format!("[client] {text}"));
                "ok\n".into()
            }
            _ => "error: commands: poweroff reboot halt status restart|stop|start NAME log TEXT\n".into(),
        };
        let mut w = &stream;
        let _ = w.write_all(reply.as_bytes());
    }

    fn request(&mut self, kind: Shutdown) -> String {
        log::notice(&format!("{kind:?} requested via control socket"));
        self.shutdown.get_or_insert(kind);
        "ok\n".into()
    }

    fn status(&self) -> String {
        let mut out = format!("target {} uptime {:.1}s\n", self.target, sys::boottime().as_secs_f64());
        for s in &self.services {
            let up = s.started.filter(|_| s.pid.is_some()).map(|t| format!("{:.1}s", t.elapsed().as_secs_f64()));
            out += &format!(
                "{:<16} {:<8} pid {:<6} ready {:<5} up {:<8} {}\n",
                s.def.name,
                format!("{:?}", s.state).to_lowercase(),
                s.pid.map_or("-".to_string(), |p| p.to_string()),
                s.ready,
                up.unwrap_or_else(|| "-".into()),
                s.last_exit.as_deref().map(|e| format!("last: {e}")).unwrap_or_default()
            );
        }
        out
    }

    fn find(&mut self, name: &str) -> Option<usize> {
        self.services.iter().position(|s| s.def.name == name)
    }

    fn stop(&mut self, name: &str) -> String {
        let Some(i) = self.find(name) else { return format!("error: no service {name}\n") };
        let s = &mut self.services[i];
        s.state = State::Stopped;
        s.restart_at = None;
        if let Some(pid) = s.pid {
            let _ = sys::kill(-pid, libc::SIGTERM);
        }
        "ok\n".into()
    }

    fn start(&mut self, name: &str) -> String {
        let Some(i) = self.find(name) else { return format!("error: no service {name}\n") };
        if self.services[i].pid.is_some() {
            return "ok (already running)\n".into();
        }
        self.services[i].fast_failures = 0;
        self.spawn(i);
        "ok\n".into()
    }

    fn restart(&mut self, name: &str) -> String {
        let Some(i) = self.find(name) else { return format!("error: no service {name}\n") };
        let s = &mut self.services[i];
        s.fast_failures = 0;
        match s.pid {
            Some(pid) => {
                // Restarted by reap_all() through the normal policy (as a failure
                // for on-failure services, since it was killed).
                s.state = State::Running;
                let _ = sys::kill(-pid, libc::SIGTERM);
                if s.def.restart == Restart::Never {
                    s.def.restart = Restart::OnFailure;
                }
            }
            None => self.spawn(i),
        }
        "ok\n".into()
    }

    fn shutdown(&mut self, kind: Shutdown, sigfd: Option<&OwnedFd>) -> ! {
        log::notice(&format!("{kind:?}: stopping services"));
        for s in self.services.iter().rev() {
            if let Some(pid) = s.pid {
                let _ = sys::kill(-pid, libc::SIGTERM);
                let _ = sys::kill(pid, libc::SIGCONT);
            }
        }
        let deadline = Instant::now() + STOP_GRACE;
        while Instant::now() < deadline && self.services.iter().any(|s| s.pid.is_some()) {
            if let Some(fd) = sigfd {
                let mut fds = [sys::pollfd(fd.as_raw_fd())];
                sys::poll(&mut fds, Some(Duration::from_millis(50)));
                sys::read_signals(fd);
            } else {
                std::thread::sleep(Duration::from_millis(50));
            }
            self.reap_all();
        }
        for s in &self.services {
            if s.pid.is_some() {
                log::warn(&format!("{} did not stop in time; killing", s.def.name));
            }
        }
        // Everything else (stray user processes) gets TERM, a moment, then KILL.
        let _ = sys::kill(-1, libc::SIGTERM);
        std::thread::sleep(Duration::from_millis(300));
        let _ = sys::kill(-1, libc::SIGKILL);
        while sys::reap().is_some() {}
        log::notice("syncing filesystems");
        sys::sync();
        if let Err(e) = sys::mount("none", "/", None, libc::MS_REMOUNT | libc::MS_RDONLY, None) {
            log::warn(&format!("remounting / read-only: {e}"));
        }
        sys::sync();
        let cmd = match kind {
            Shutdown::Poweroff => libc::RB_POWER_OFF,
            Shutdown::Reboot => libc::RB_AUTOBOOT,
            Shutdown::Halt => libc::RB_HALT_SYSTEM,
        };
        log::notice(&format!("{kind:?} now"));
        let err = sys::reboot(cmd);
        log::error(&format!("reboot(2) failed: {err}"));
        loop {
            std::thread::sleep(Duration::from_secs(3600));
        }
    }
}

fn open_log(name: &str) -> std::io::Result<fs::File> {
    let path = format!("/run/log/{name}.log");
    fs::OpenOptions::new().create(true).append(true).mode(0o640).open(path)
}

fn spawn_service(def: &ServiceDef) -> std::io::Result<i32> {
    let (user, groups) = users::lookup(&def.user)
        .ok_or_else(|| std::io::Error::other(format!("unknown user '{}'", def.user)))?;
    let mut cmd = Command::new(&def.exec[0]);
    cmd.args(&def.exec[1..]).env_clear();
    let mut env: HashMap<String, String> = HashMap::new();
    env.insert("PATH".into(), "/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin".into());
    env.insert("HOME".into(), user.home.clone());
    env.insert("USER".into(), user.name.clone());
    env.insert("LOGNAME".into(), user.name.clone());
    env.insert("SHELL".into(), user.shell.clone());
    env.insert("LANG".into(), "C.UTF-8".into());
    env.insert("LUMEN_NOTIFY_SOCKET".into(), NOTIFY_SOCKET.into());
    if def.runtime_dir {
        let dir = format!("/run/user/{}", user.uid);
        let _ = fs::create_dir_all(&dir);
        let _ = sys::chown(&dir, user.uid, user.gid);
        let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
        env.insert("XDG_RUNTIME_DIR".into(), dir);
    }
    for (k, v) in &def.env {
        env.insert(k.clone(), v.clone());
    }
    cmd.envs(&env);
    let workdir = def.workdir.clone().unwrap_or_else(|| user.home.clone());
    cmd.current_dir(if Path::new(&workdir).is_dir() { workdir } else { "/".into() });

    let tty_fd = match &def.tty {
        Some(tty) => {
            let f = fs::OpenOptions::new().read(true).write(true).custom_flags(libc::O_NOCTTY).open(tty)?;
            cmd.stdin(Stdio::from(f.try_clone()?)).stdout(Stdio::from(f.try_clone()?)).stderr(Stdio::from(f));
            cmd.env("TERM", if tty.contains("ttyS") { "vt100" } else { "linux" });
            true
        }
        None => {
            let f = open_log(&def.name)?;
            cmd.stdin(Stdio::null()).stdout(Stdio::from(f.try_clone()?)).stderr(Stdio::from(f));
            false
        }
    };
    let (uid, gid) = (user.uid, user.gid);
    let mut all_groups = groups;
    all_groups.push(gid);
    // SAFETY: only async-signal-safe syscalls run between fork and exec.
    unsafe {
        cmd.pre_exec(move || {
            sys::reset_signals_in_child();
            sys::setsid()?;
            if tty_fd {
                sys::set_controlling_tty(0)?;
            }
            sys::set_ids(uid, gid, &all_groups)?;
            Ok(())
        });
    }
    let child = cmd.spawn()?;
    Ok(child.id() as i32)
}
