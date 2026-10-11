//! lsh entry point: `lsh [-ilsex] [-c command [name [args...]] | script [args...]]`.

use lsh::editor::{Editor, ReadResult};
use lsh::exec::{Flow, Shell};
use lsh::interactive::{default_prompt, render_prompt, Session};
use lsh::sys;

fn usage() -> ! {
    eprintln!("usage: lsh [-ilsexuCf] [-o option] [-c command [name [arg...]] | script [arg...]]");
    std::process::exit(2);
}

fn finish(sh: &mut Shell, code: i32) -> ! {
    sh.status = code;
    let _ = sh.run_exit_trap();
    std::process::exit(code & 0xff);
}

fn run(sh: &mut Shell, src: &str) -> Option<i32> {
    match sh.run_source(src) {
        Ok(_) => None,
        Err(Flow::Exit(c)) => Some(c),
        Err(_) => None,
    }
}

/// Read one line from fd 0 byte by byte, so commands run from the script can
/// still read the rest of standard input.
fn read_line_fd0() -> Option<String> {
    let mut line = Vec::new();
    let mut b = [0u8; 1];
    loop {
        match sys::read(0, &mut b) {
            Ok(1) => {
                line.push(b[0]);
                if b[0] == b'\n' {
                    break;
                }
            }
            _ => {
                if line.is_empty() {
                    return None;
                }
                break;
            }
        }
    }
    Some(String::from_utf8_lossy(&line).into_owned())
}

fn source_file(sh: &mut Shell, path: &str) {
    if std::path::Path::new(path).is_file() {
        if let Some(code) = run(sh, &format!(". '{}'", path.replace('\'', "'\\''"))) {
            finish(sh, code);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg0 = args.first().cloned().unwrap_or_else(|| "lsh".into());
    let mut login = arg0.starts_with('-');
    let mut command: Option<String> = None;
    let mut force_interactive = false;
    let mut read_stdin = false;
    let mut set_args: Vec<String> = Vec::new();
    let mut i = 1;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "-c" => {
                command = Some(args.get(i + 1).cloned().unwrap_or_else(|| usage()));
                i += 2;
                break;
            }
            "-i" => force_interactive = true,
            "-l" | "--login" => login = true,
            "-s" => read_stdin = true,
            "--" => {
                i += 1;
                break;
            }
            "--version" => {
                println!("lsh {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            "-h" | "--help" => usage(),
            "-o" | "+o" => {
                set_args.push(a.clone());
                set_args.push(args.get(i + 1).cloned().unwrap_or_else(|| usage()));
                i += 1;
            }
            s if (s.starts_with('-') || s.starts_with('+')) && s.len() > 1 => set_args.push(a.clone()),
            _ => break,
        }
        i += 1;
    }
    let rest: Vec<String> = args[i.min(args.len())..].to_vec();

    // Children must see default SIGPIPE behaviour (Rust ignores it by default).
    sys::set_signal(libc::SIGPIPE, libc::SIG_DFL);

    let (name, positional) = match (&command, read_stdin, rest.first()) {
        (Some(_), _, Some(n)) => (n.clone(), rest[1..].to_vec()),
        (Some(_), _, None) => (arg0.clone(), Vec::new()),
        (None, false, Some(script)) => (script.clone(), rest[1..].to_vec()),
        _ => (arg0.clone(), rest.clone()),
    };
    let mut sh = Shell::new(&name, positional);
    if !set_args.is_empty() {
        let mut argv = vec!["set".to_string()];
        argv.extend(set_args);
        argv.push("--".into());
        argv.extend(sh.positional.clone());
        let _ = sh.run_builtin(&argv);
    }

    if let Some(cmd) = command {
        let code = run(&mut sh, &cmd).unwrap_or(sh.status);
        finish(&mut sh, code);
    }
    if !read_stdin {
        if let Some(script) = rest.first() {
            let src = match std::fs::read(script) {
                Ok(b) => String::from_utf8_lossy(&b).into_owned(),
                Err(e) => {
                    eprintln!("lsh: {script}: {}", lsh::exec::describe_io(&e));
                    std::process::exit(127);
                }
            };
            let code = run(&mut sh, &src).unwrap_or(sh.status);
            finish(&mut sh, code);
        }
    }

    let interactive = force_interactive || (sys::isatty(0) && sys::isatty(2));
    if !interactive {
        let mut src = String::new();
        while let Some(line) = read_line_fd0() {
            src.push_str(&line);
            if !src.ends_with('\n') {
                src.push('\n');
            }
            match sh.parse(&src) {
                Err(e) if e.incomplete => continue,
                _ => {
                    if let Some(code) = run(&mut sh, &src) {
                        finish(&mut sh, code);
                    }
                    src.clear();
                }
            }
        }
        if !src.trim().is_empty() {
            if let Some(code) = run(&mut sh, &src) {
                finish(&mut sh, code);
            }
        }
        let code = sh.status;
        finish(&mut sh, code);
    }

    interactive_loop(sh, login);
}

fn interactive_loop(mut sh: Shell, login: bool) -> ! {
    let tty = 0;
    sh.opts.interactive = true;
    if sys::isatty(tty) {
        // Wait until we are in the foreground, then take over the terminal.
        loop {
            let fg = sys::tcgetpgrp(tty);
            if fg < 0 || fg == sys::getpgrp() {
                break;
            }
            let _ = sys::kill(-sys::getpgrp(), libc::SIGTTIN);
        }
        for s in sys::JOB_SIGNALS {
            sys::set_signal(s, libc::SIG_IGN);
        }
        sys::set_signal(libc::SIGTERM, libc::SIG_IGN);
        let pid = sys::getpid();
        sys::setpgid(pid, pid);
        sh.shell_pgid = sys::getpgrp();
        sys::tcsetpgrp(tty, sh.shell_pgid);
        sh.shell_termios = sys::get_termios(tty);
        sh.tty = tty;
        sh.opts.monitor = true;
    }
    if login {
        source_file(&mut sh, "/etc/profile");
        if let Some(home) = sh.get("HOME") {
            source_file(&mut sh, &format!("{home}/.profile"));
        }
    }
    source_file(&mut sh, "/etc/lshrc");
    let home = sh.get("HOME");
    if let Some(home) = &home {
        source_file(&mut sh, &format!("{home}/.lshrc"));
    }
    let history_file = sh.get("HISTFILE").or_else(|| home.as_ref().map(|h| format!("{h}/.lsh_history")));
    let mut editor = Editor::new(tty, history_file);
    sh.history = editor.history.clone();
    let mut warned_stopped = false;
    loop {
        sh.reap_and_notify(true);
        let ps1 = sh.get("PS1").unwrap_or_else(|| default_prompt(&sh));
        let prompt = render_prompt(&mut sh, &ps1);
        let mut src = match editor.read_line(&prompt, &mut Session { sh: &mut sh }) {
            ReadResult::Line(l) => l,
            ReadResult::Eof => {
                let stopped = sh.jobs.jobs.iter().any(|j| j.state() == lsh::jobs::JobState::Stopped);
                if stopped && !warned_stopped {
                    sh.err("there are stopped jobs");
                    warned_stopped = true;
                    continue;
                }
                let code = sh.status;
                finish(&mut sh, code);
            }
        };
        // Continuation lines for incomplete input.
        let mut aborted = false;
        loop {
            match sh.parse(&src) {
                Err(e) if e.incomplete => {
                    let ps2 = sh.get("PS2").unwrap_or_else(|| "\u{1}\x1b[90m\u{2}… \u{1}\x1b[0m\u{2}".into());
                    let p2 = render_prompt(&mut sh, &ps2);
                    match editor.read_line(&p2, &mut Session { sh: &mut sh }) {
                        ReadResult::Line(l) => {
                            src.push('\n');
                            src.push_str(&l);
                        }
                        ReadResult::Eof => {
                            sh.err(&format!("syntax error: {}", e.message));
                            aborted = true;
                            break;
                        }
                    }
                }
                _ => break,
            }
        }
        if aborted || src.trim().is_empty() {
            continue;
        }
        warned_stopped = false;
        editor.add_history(&src);
        sh.history = editor.history.clone();
        if let Some(code) = run(&mut sh, &src) {
            finish(&mut sh, code);
        }
    }
}
