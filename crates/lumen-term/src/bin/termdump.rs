//! Run a program on a pseudo-terminal with Lumen's emulation core and drive
//! it from a script, dumping screens as text. Used to run vttest and other
//! full-screen programs in tests without a display.
//!
//!   lumen-termdump [--size 80x24] [--out DIR] --script FILE -- PROGRAM [ARGS...]
//!
//! Script lines (blank lines and `#` comments are ignored):
//!   send TEXT        type TEXT (escapes: \r \n \t \e \\ \xHH)
//!   wait TEXT        wait until the screen contains TEXT (10 s)
//!   expect TEXT      fail unless the screen contains TEXT now
//!   sleep MS         keep processing output for MS milliseconds
//!   settle           wait until the program has been quiet for 300 ms
//!   dump NAME        write the screen to DIR/NAME.txt
//!   resize COLSxROWS resize the terminal

use std::io::Write;
use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};

use lumen_term::pty::Pty;
use lumen_term::term::Terminal;

fn unescape(s: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 1 < b.len() {
            i += 1;
            match b[i] {
                b'r' => out.push(b'\r'),
                b'n' => out.push(b'\n'),
                b't' => out.push(b'\t'),
                b'e' => out.push(0x1b),
                b'\\' => out.push(b'\\'),
                b'x' if i + 2 < b.len() => {
                    if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                        out.push(v);
                    }
                    i += 2;
                }
                c => out.push(c),
            }
        } else {
            out.push(b[i]);
        }
        i += 1;
    }
    out
}

struct Session {
    pty: Pty,
    term: Terminal,
    last_output: Instant,
    exited: bool,
}

impl Session {
    /// Process output for up to `dur`; returns early if `until` holds.
    fn pump(&mut self, dur: Duration, until: &mut dyn FnMut(&Terminal) -> bool) -> bool {
        let deadline = Instant::now() + dur;
        let mut buf = vec![0u8; 65536];
        loop {
            if until(&self.term) {
                return true;
            }
            let now = Instant::now();
            if now >= deadline || self.exited {
                return until(&self.term);
            }
            let ms = (deadline - now).as_millis().min(50) as i32;
            let mut pfd = libc::pollfd { fd: self.pty.master.as_raw_fd(), events: libc::POLLIN, revents: 0 };
            // SAFETY: polling one valid descriptor.
            let r = unsafe { libc::poll(&mut pfd, 1, ms) };
            if r > 0 {
                match self.pty.read(&mut buf) {
                    Ok(0) => self.exited = true,
                    Ok(n) => {
                        self.term.advance(&buf[..n]);
                        self.last_output = Instant::now();
                        if !self.term.term.replies.is_empty() {
                            let replies = std::mem::take(&mut self.term.term.replies);
                            let _ = self.pty.write(&replies);
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(_) => self.exited = true,
                }
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut size = (80usize, 24usize);
    let mut out_dir = ".".to_string();
    let mut script = None;
    let mut i = 0;
    let mut cmd = Vec::new();
    while i < args.len() {
        match args[i].as_str() {
            "--size" => {
                let v = &args[i + 1];
                let (c, r) = v.split_once('x').expect("--size COLSxROWS");
                size = (c.parse().expect("cols"), r.parse().expect("rows"));
                i += 1;
            }
            "--out" => {
                out_dir = args[i + 1].clone();
                i += 1;
            }
            "--script" => {
                script = Some(args[i + 1].clone());
                i += 1;
            }
            "--" => {
                cmd = args[i + 1..].to_vec();
                break;
            }
            a => {
                eprintln!("lumen-termdump: unknown argument {a}");
                std::process::exit(2);
            }
        }
        i += 1;
    }
    let script = std::fs::read_to_string(script.expect("--script FILE")).expect("read script");
    if cmd.is_empty() {
        eprintln!("lumen-termdump: no program given");
        std::process::exit(2);
    }
    let pty = Pty::spawn(&cmd, None, (size.0 as u16, size.1 as u16, 0, 0), &[]).expect("spawn");
    let mut s = Session { pty, term: Terminal::new(size.0, size.1, 1000), last_output: Instant::now(), exited: false };
    let _ = std::fs::create_dir_all(&out_dir);
    let mut failed = false;
    for (n, line) in script.lines().enumerate() {
        let line = line.trim_end();
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let (verb, rest) = line.split_once(' ').unwrap_or((line, ""));
        match verb {
            "send" => {
                let _ = s.pty.write(&unescape(rest));
            }
            "wait" => {
                let text = String::from_utf8_lossy(&unescape(rest)).into_owned();
                if !s.pump(Duration::from_secs(10), &mut |t| t.term.screen_text().contains(&text)) {
                    eprintln!("line {}: timed out waiting for {text:?}; screen:\n{}", n + 1, s.term.term.screen_text());
                    failed = true;
                    break;
                }
            }
            "expect" => {
                let text = String::from_utf8_lossy(&unescape(rest)).into_owned();
                if !s.term.term.screen_text().contains(&text) {
                    eprintln!("line {}: expected {text:?}; screen:\n{}", n + 1, s.term.term.screen_text());
                    failed = true;
                }
            }
            "sleep" => {
                let ms: u64 = rest.trim().parse().unwrap_or(100);
                s.pump(Duration::from_millis(ms), &mut |_| false);
            }
            "settle" => {
                let start = Instant::now();
                while start.elapsed() < Duration::from_secs(10) {
                    s.pump(Duration::from_millis(50), &mut |_| false);
                    if s.last_output.elapsed() > Duration::from_millis(300) || s.exited {
                        break;
                    }
                }
            }
            "dump" => {
                let path = format!("{out_dir}/{}.txt", rest.trim());
                let mut f = std::fs::File::create(&path).expect("dump file");
                let _ = writeln!(f, "{}", s.term.term.screen_text());
            }
            "resize" => {
                let (c, r) = rest.trim().split_once('x').expect("resize COLSxROWS");
                let (c, r): (usize, usize) = (c.parse().unwrap(), r.parse().unwrap());
                s.term.term.resize(c, r);
                s.pty.resize(c as u16, r as u16, 0, 0);
            }
            v => {
                eprintln!("line {}: unknown command {v}", n + 1);
                failed = true;
                break;
            }
        }
    }
    std::process::exit(if failed { 1 } else { 0 });
}
