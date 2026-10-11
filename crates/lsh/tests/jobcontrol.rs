//! Job control through a real pseudo-terminal: Ctrl+Z, jobs, bg, fg, Ctrl+C.

use std::ffi::CString;
use std::time::{Duration, Instant};

struct Pty {
    master: libc::c_int,
    pid: libc::pid_t,
    output: Vec<u8>,
}

impl Pty {
    fn spawn(home: &str) -> Pty {
        let mut master = 0;
        let mut slave = 0;
        // SAFETY: openpty with valid out pointers; the child only calls
        // async-signal-safe functions before exec.
        unsafe {
            let mut ws: libc::winsize = std::mem::zeroed();
            ws.ws_col = 200;
            ws.ws_row = 50;
            assert_eq!(libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null(), &ws), 0);
            let bin = CString::new(env!("CARGO_BIN_EXE_lsh")).unwrap();
            let argv = [bin.as_ptr(), c"-i".as_ptr(), std::ptr::null()];
            let env_strings: Vec<CString> = [
                format!("HOME={home}"),
                format!("HISTFILE={home}/hist"),
                "PATH=/usr/bin:/bin".to_string(),
                "PS1=PROMPT> ".to_string(),
                "TERM=xterm".to_string(),
            ]
            .iter()
            .map(|s| CString::new(s.as_str()).unwrap())
            .collect();
            let mut envp: Vec<*const libc::c_char> = env_strings.iter().map(|s| s.as_ptr()).collect();
            envp.push(std::ptr::null());
            let pid = libc::fork();
            if pid == 0 {
                libc::setsid();
                libc::ioctl(slave, libc::TIOCSCTTY, 0);
                libc::dup2(slave, 0);
                libc::dup2(slave, 1);
                libc::dup2(slave, 2);
                libc::close(master);
                libc::execve(bin.as_ptr(), argv.as_ptr(), envp.as_ptr());
                libc::_exit(127);
            }
            libc::close(slave);
            libc::fcntl(master, libc::F_SETFL, libc::O_NONBLOCK);
            Pty { master, pid, output: Vec::new() }
        }
    }

    fn send(&self, s: &str) {
        // SAFETY: valid buffer.
        unsafe { libc::write(self.master, s.as_ptr().cast(), s.len()) };
    }

    fn pump(&mut self, wait_ms: i32) {
        let mut p = libc::pollfd { fd: self.master, events: libc::POLLIN, revents: 0 };
        // SAFETY: one pollfd, buffer of 4096 bytes.
        unsafe {
            if libc::poll(&mut p, 1, wait_ms) > 0 {
                let mut buf = [0u8; 4096];
                loop {
                    let n = libc::read(self.master, buf.as_mut_ptr().cast(), buf.len());
                    if n <= 0 {
                        break;
                    }
                    self.output.extend_from_slice(&buf[..n as usize]);
                }
            }
        }
    }

    /// Wait for `needle` (ignoring ANSI escape sequences) to appear after byte
    /// offset `since`; returns the offset just past the match.
    fn expect(&mut self, needle: &str, since: usize) -> usize {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let raw = &self.output[since.min(self.output.len())..];
            let (text, map) = strip_ansi(raw);
            if let Some(i) = text.find(needle) {
                return since + map[i + needle.len() - 1] + 1;
            }
            if Instant::now() > deadline {
                let (all, _) = strip_ansi(&self.output); let tail: String = all.chars().rev().take(1500).collect::<Vec<_>>().into_iter().rev().collect(); let st = std::fs::read_to_string(format!("/proc/{}/status", self.pid)).unwrap_or_default(); let wchan = std::fs::read_to_string(format!("/proc/{}/wchan", self.pid)).unwrap_or_default(); let ps = std::process::Command::new("ps").arg("-o").arg("pid,ppid,pgid,stat,comm").output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default(); panic!("timed out waiting for {needle:?}; output since {since}:\n{text}\n--- full tail:\n{tail}\n--- shell: {} wchan {wchan}\n{ps}", st.lines().nth(2).unwrap_or(""));
            }
            self.pump(50);
        }
    }

    fn mark(&self) -> usize {
        self.output.len()
    }
}

/// Remove CSI sequences; returns the text and, per text byte, its raw offset.
fn strip_ansi(raw: &[u8]) -> (String, Vec<usize>) {
    let mut text = Vec::new();
    let mut map = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == 0x1b && raw.get(i + 1) == Some(&b'[') {
            i += 2;
            while i < raw.len() && !(0x40..=0x7e).contains(&raw[i]) {
                i += 1;
            }
            i += 1;
            continue;
        }
        text.push(raw[i]);
        map.push(i);
        i += 1;
    }
    (String::from_utf8_lossy(&text).into_owned(), map)
}

impl Drop for Pty {
    fn drop(&mut self) {
        // SAFETY: plain syscalls on our child and fd. pid <= 0 means the child
        // was already reaped (and kill(-1) would signal every process).
        unsafe {
            if self.pid > 0 {
                libc::kill(self.pid, libc::SIGKILL);
                libc::waitpid(self.pid, std::ptr::null_mut(), 0);
            }
            libc::close(self.master);
        }
    }
}

#[test]
fn job_control_in_a_pty() {
    let home = std::env::temp_dir().join(format!("lsh-jc-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    let mut t = Pty::spawn(&home.to_string_lossy());
    t.expect("PROMPT> ", 0);

    // Ctrl+Z stops the foreground job.
    let m = t.mark();
    t.send("sleep 30\r");
    t.expect("sleep 30", m);
    std::thread::sleep(Duration::from_millis(400));
    t.send("\x1a");
    let m = t.expect("Stopped", m);
    t.expect("PROMPT> ", m);

    let m = t.mark();
    t.send("jobs\r");
    let m2 = t.expect("[1]+  Stopped", m);
    t.expect("sleep 30", m2);

    // bg resumes it in the background.
    let m = t.mark();
    t.send("bg\r");
    t.expect("[1]+ sleep 30 &", m);
    let m = t.mark();
    t.send("jobs\r");
    t.expect("Running", m);

    // fg brings it back; Ctrl+C kills it and $? is 130.
    let m = t.mark();
    t.send("fg\r");
    let m = t.expect("sleep 30", m + 4);
    std::thread::sleep(Duration::from_millis(400));
    t.send("\x03");
    t.expect("PROMPT> ", m);
    let m = t.mark();
    t.send("echo st=$?\r");
    t.expect("st=130", m + 12);

    // Ctrl+C at the prompt cancels the line; the shell survives.
    let m = t.mark();
    t.send("echo never\x03");
    t.expect("^C", m);
    let m = t.mark();
    t.send("echo alive\r");
    t.expect("\nalive", m);

    // A stopped pipeline is one job; killing it is reported.
    let m = t.mark();
    t.send("cat | cat\r");
    std::thread::sleep(Duration::from_millis(400));
    t.send("\x1a");
    t.expect("Stopped", m);
    let m = t.mark();
    t.send("kill -9 %1\r");
    // The notification comes before the next prompt once the processes are reaped.
    std::thread::sleep(Duration::from_millis(300));
    t.send("\r");
    t.expect("Killed", m);

    // Background job completion is announced before the next prompt.
    let m = t.mark();
    t.send("sleep 0.2 &\r");
    t.expect("[1] ", m);
    std::thread::sleep(Duration::from_millis(600));
    let m = t.mark();
    t.send("\r");
    t.expect("Done", m);

    let m = t.mark();
    t.send("exit 3\r");
    let _ = m;
    let mut status = 0;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        // SAFETY: waitpid on our child.
        let r = unsafe { libc::waitpid(t.pid, &mut status, libc::WNOHANG) };
        if r == t.pid {
            break;
        }
        assert!(Instant::now() < deadline, "shell did not exit");
        t.pump(50);
    }
    assert!(libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 3, "exit status {status}");
    t.pid = -1;
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn line_editing_in_a_pty() {
    let home = std::env::temp_dir().join(format!("lsh-ed-{}", std::process::id()));
    std::fs::create_dir_all(home.join("some_directory")).unwrap();
    std::fs::write(home.join("hist"), "echo from-history\nprintf 'searched\\n'\n").unwrap();
    let mut t = Pty::spawn(&home.to_string_lossy());
    let mut m = t.expect("PROMPT> ", 0);
    // Each step waits for the prompt (typed-ahead input would otherwise be
    // edited by the kernel's canonical mode, as with any shell).
    let mut step = |t: &mut Pty, input: &str, want: &str| {
        let start = t.mark();
        t.send(input);
        let end = t.expect(want, start);
        m = t.expect("PROMPT> ", end);
        let _ = m;
    };

    // Tab completes builtins and paths (a directory gets a trailing slash).
    step(&mut t, &format!("ech\t{}/some_d\t\r", home.display()), "some_directory/\r\n");
    // Up recalls history (prefix-less: newest first); Ctrl+R searches it.
    step(&mut t, "\x1b[A\x1b[A\x1b[A\r", "\nfrom-history");
    step(&mut t, "\x12searc\r", "\nsearched");
    // History suggestion: typing a prefix and pressing End accepts the rest.
    step(&mut t, "echo fr\x1b[F\r", "\nfrom-history");
    // Incomplete input continues on the next line.
    step(&mut t, "for i in 1 2; do\recho n$i; done\r", "\nn2");
    // Consecutive kills accumulate; yank restores them.
    step(&mut t, "echo one two\x17\x17\x19\r", "\none two");
    let _ = std::fs::remove_dir_all(&home);
}
