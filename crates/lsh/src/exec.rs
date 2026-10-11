//! The interpreter: shell state and execution of the syntax tree.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::ast::*;
use crate::expand::{self, Env, ExpandError};
use crate::glob;
use crate::jobs::{JobState, JobTable};
use crate::parser::{self, ParseError};
use crate::sys::{self, Fd, WaitStatus};

/// Non-local control flow out of a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Break(usize),
    Continue(usize),
    Return(i32),
    Exit(i32),
}

pub type ExecResult = Result<i32, Flow>;

#[derive(Debug, Clone, Default)]
pub struct Var {
    pub value: String,
    pub exported: bool,
    pub readonly: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub errexit: bool,
    pub nounset: bool,
    pub xtrace: bool,
    pub noglob: bool,
    pub noclobber: bool,
    pub pipefail: bool,
    /// Job control (process groups and terminal ownership).
    pub monitor: bool,
    pub interactive: bool,
}

pub const SPECIAL_BUILTINS: &[&str] = &[
    ":", ".", "break", "continue", "eval", "exec", "exit", "export", "readonly", "return", "set", "shift", "trap",
    "unset", "source", "times",
];

pub const BUILTINS: &[&str] = &[
    "alias", "unalias", "bg", "fg", "jobs", "cd", "pwd", "echo", "printf", "test", "[", "true", "false", "read", "type",
    "command", "wait", "kill", "umask", "local", "history", "getopts", "hash", "help",
];

pub fn is_builtin(name: &str) -> bool {
    SPECIAL_BUILTINS.contains(&name) || BUILTINS.contains(&name)
}

/// Signals caught for traps; set from the handler, consumed between commands.
static PENDING: [AtomicBool; 32] = [const { AtomicBool::new(false) }; 32];

extern "C" fn trap_handler(sig: libc::c_int) {
    if (0..32).contains(&sig) {
        PENDING[sig as usize].store(true, Ordering::SeqCst);
    }
}

pub struct Shell {
    pub vars: HashMap<String, Var>,
    locals: Vec<Vec<(String, Option<Var>)>>,
    pub positional: Vec<String>,
    pub arg0: String,
    pub functions: HashMap<String, Rc<Command>>,
    pub aliases: HashMap<String, String>,
    pub status: i32,
    pub last_bg: Option<libc::pid_t>,
    pub opts: Options,
    pub jobs: JobTable,
    pub tty: Fd,
    pub shell_pgid: libc::pid_t,
    pub shell_termios: Option<sys::Termios>,
    pub traps: HashMap<i32, String>,
    loop_depth: usize,
    func_depth: usize,
    pub subshell: bool,
    errexit_suppressed: usize,
    subst_status: Option<i32>,
    path_cache: HashMap<String, String>,
    pub history: Vec<String>,
    /// getopts: index into the current option cluster.
    pub getopts_pos: usize,
    /// PID of the top-level shell (`$$` in subshells).
    top_pid: libc::pid_t,
    /// Source text of the list item being run (job listings).
    current_text: String,
    /// In a forked child whose only remaining work is one simple command: exec
    /// external commands directly instead of forking again.
    direct_exec: bool,
}

impl Shell {
    pub fn new(arg0: &str, args: Vec<String>) -> Self {
        let mut vars = HashMap::new();
        for (k, v) in std::env::vars() {
            vars.insert(k, Var { value: v, exported: true, readonly: false });
        }
        let mut sh = Shell {
            vars,
            locals: Vec::new(),
            positional: args,
            arg0: arg0.to_string(),
            functions: HashMap::new(),
            aliases: HashMap::new(),
            status: 0,
            last_bg: None,
            opts: Options::default(),
            jobs: JobTable::default(),
            tty: 0,
            shell_pgid: sys::getpgrp(),
            shell_termios: None,
            traps: HashMap::new(),
            loop_depth: 0,
            func_depth: 0,
            subshell: false,
            errexit_suppressed: 0,
            subst_status: None,
            path_cache: HashMap::new(),
            history: Vec::new(),
            getopts_pos: 0,
            top_pid: sys::getpid(),
            current_text: String::new(),
            direct_exec: false,
        };
        if !sh.vars.contains_key("PATH") {
            sh.set("PATH", "/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin");
        }
        sh.set("PPID", &unsafe { libc::getppid() }.to_string());
        if !sh.vars.contains_key("IFS") {
            sh.vars.insert("IFS".into(), Var { value: " \t\n".into(), exported: false, readonly: false });
        }
        sh.set("OPTIND", "1");
        if let Ok(cwd) = std::env::current_dir() {
            let pwd_valid = sh.get("PWD").is_some_and(|p| {
                std::fs::canonicalize(&p).ok() == std::fs::canonicalize(&cwd).ok()
            });
            if !pwd_valid {
                sh.set("PWD", &cwd.to_string_lossy());
            }
        }
        sh
    }

    // -- variables -------------------------------------------------------------------------

    pub fn get(&self, name: &str) -> Option<String> {
        self.vars.get(name).map(|v| v.value.clone())
    }

    pub fn set(&mut self, name: &str, value: &str) {
        let _ = self.try_set(name, value);
    }

    pub fn try_set(&mut self, name: &str, value: &str) -> Result<(), String> {
        let e = self.vars.entry(name.to_string()).or_default();
        if e.readonly {
            return Err(format!("{name}: readonly variable"));
        }
        e.value = value.to_string();
        if name == "PATH" {
            self.path_cache.clear();
        }
        Ok(())
    }

    pub fn unset(&mut self, name: &str) -> Result<(), String> {
        if self.vars.get(name).is_some_and(|v| v.readonly) {
            return Err(format!("{name}: readonly variable"));
        }
        self.vars.remove(name);
        if name == "PATH" {
            self.path_cache.clear();
        }
        Ok(())
    }

    pub fn export(&mut self, name: &str) {
        self.vars.entry(name.to_string()).or_default().exported = true;
    }

    /// `local NAME`: save the current value, restored when the function returns.
    pub fn make_local(&mut self, name: &str) -> Result<(), String> {
        let Some(frame) = self.locals.last_mut() else {
            return Err("local: can only be used in a function".into());
        };
        if !frame.iter().any(|(n, _)| n == name) {
            frame.push((name.to_string(), self.vars.get(name).cloned()));
        }
        Ok(())
    }

    pub fn env_list(&self, extra: &[(String, String)]) -> Vec<String> {
        let mut map: Vec<(String, String)> =
            self.vars.iter().filter(|(_, v)| v.exported).map(|(k, v)| (k.clone(), v.value.clone())).collect();
        for (k, v) in extra {
            map.retain(|(n, _)| n != k);
            map.push((k.clone(), v.clone()));
        }
        map.into_iter().map(|(k, v)| format!("{k}={v}")).collect()
    }

    // -- output helpers ---------------------------------------------------------------------

    pub fn out(&self, s: &str) {
        let _ = sys::write_all(1, s.as_bytes());
    }

    pub fn err(&self, msg: &str) {
        let _ = sys::write_all(2, format!("lsh: {msg}\n").as_bytes());
    }

    // -- entry points ---------------------------------------------------------------------

    pub fn parse(&self, src: &str) -> Result<Program, ParseError> {
        parser::parse_with_aliases(src, &self.aliases)
    }

    /// Parse and run source text (scripts, -c, eval, the interactive loop).
    pub fn run_source(&mut self, src: &str) -> ExecResult {
        match self.parse(src) {
            Ok(prog) => self.run_program(&prog),
            Err(e) => {
                self.err(&format!("syntax error: {}", e.message.trim_start_matches("syntax error: ")));
                self.status = 2;
                if self.opts.interactive { Ok(2) } else { Err(Flow::Exit(2)) }
            }
        }
    }

    pub fn run_program(&mut self, prog: &Program) -> ExecResult {
        let mut status = self.status;
        for item in &prog.0 {
            self.run_traps()?;
            self.current_text.clone_from(&item.text);
            if item.background {
                status = self.spawn_background(item);
            } else {
                status = self.run_and_or(&item.and_or)?;
            }
            self.status = status;
            if status != 0 && self.opts.errexit && self.errexit_suppressed == 0 && !item.background {
                let negated = item.and_or.rest.last().map_or(item.and_or.first.negate, |(_, p)| p.negate);
                if !negated {
                    return Err(Flow::Exit(status));
                }
            }
        }
        self.run_traps()?;
        Ok(status)
    }

    fn run_and_or(&mut self, ao: &AndOr) -> ExecResult {
        let has_rest = !ao.rest.is_empty();
        if has_rest {
            self.errexit_suppressed += 1;
        }
        let first = self.run_pipeline(&ao.first);
        if has_rest {
            self.errexit_suppressed -= 1;
        }
        let mut status = first?;
        self.status = status;
        for (i, (op, pl)) in ao.rest.iter().enumerate() {
            let run = match op {
                AndOrOp::And => status == 0,
                AndOrOp::Or => status != 0,
            };
            if !run {
                continue;
            }
            let last = i + 1 == ao.rest.len();
            if !last {
                self.errexit_suppressed += 1;
            }
            let r = self.run_pipeline(pl);
            if !last {
                self.errexit_suppressed -= 1;
            }
            status = r?;
            self.status = status;
        }
        Ok(status)
    }

    fn run_pipeline(&mut self, pl: &Pipeline) -> ExecResult {
        if pl.negate {
            self.errexit_suppressed += 1;
        }
        let r = if pl.commands.len() == 1 {
            self.run_command(&pl.commands[0])
        } else {
            let text = if self.current_text.is_empty() { pipeline_text(pl) } else { self.current_text.clone() };
            Ok(self.run_forked_pipeline(&pl.commands, false, &text))
        };
        if pl.negate {
            self.errexit_suppressed -= 1;
        }
        let status = r?;
        Ok(if pl.negate { (status == 0) as i32 } else { status })
    }

    // -- commands ---------------------------------------------------------------------------

    pub fn run_command(&mut self, cmd: &Command) -> ExecResult {
        match cmd {
            Command::Simple(sc) => self.run_simple(sc),
            Command::Group(body, redirs) => self.with_redirects(redirs, |sh| sh.run_program(body)),
            Command::Subshell(body, redirs) => {
                let text = self.current_text.clone();
                let body = body.clone();
                let redirs = redirs.clone();
                Ok(self.fork_job(&text, false, move |sh| {
                    sh.direct_exec = single_simple(&body);
                    sh.with_redirects(&redirs, |sh| sh.run_program(&body))
                }))
            }
            Command::If { branches, otherwise, redirects } => self.with_redirects(redirects, |sh| {
                for (cond, body) in branches {
                    sh.errexit_suppressed += 1;
                    let c = sh.run_program(cond);
                    sh.errexit_suppressed -= 1;
                    if c? == 0 {
                        return sh.run_program(body);
                    }
                }
                match otherwise {
                    Some(o) => sh.run_program(o),
                    None => Ok(0),
                }
            }),
            Command::While { until, cond, body, redirects } => self.with_redirects(redirects, |sh| {
                let mut status = 0;
                sh.loop_depth += 1;
                let result = loop {
                    sh.errexit_suppressed += 1;
                    let c = sh.run_program(cond);
                    sh.errexit_suppressed -= 1;
                    let c = match c {
                        Ok(c) => c,
                        Err(f) => break Err(f),
                    };
                    if (c == 0) == *until {
                        break Ok(status);
                    }
                    match sh.run_program(body) {
                        Ok(s) => status = s,
                        Err(Flow::Break(n)) => {
                            if n > 1 {
                                break Err(Flow::Break(n - 1));
                            }
                            break Ok(0);
                        }
                        Err(Flow::Continue(n)) => {
                            if n > 1 {
                                break Err(Flow::Continue(n - 1));
                            }
                        }
                        Err(f) => break Err(f),
                    }
                };
                sh.loop_depth -= 1;
                result
            }),
            Command::For { var, items, body, redirects } => self.with_redirects(redirects, |sh| {
                let values = match items {
                    Some(words) => match expand::expand_words(sh, words) {
                        Ok(v) => v,
                        Err(e) => return sh.expansion_failed(e),
                    },
                    None => sh.positional.clone(),
                };
                let mut status = 0;
                sh.loop_depth += 1;
                let mut result = Ok(0);
                for v in values {
                    if let Err(e) = sh.try_set(var, &v) {
                        sh.err(&e);
                        result = Ok(1);
                        break;
                    }
                    match sh.run_program(body) {
                        Ok(s) => status = s,
                        Err(Flow::Break(n)) => {
                            result = if n > 1 { Err(Flow::Break(n - 1)) } else { Ok(status) };
                            break;
                        }
                        Err(Flow::Continue(n)) => {
                            if n > 1 {
                                result = Err(Flow::Continue(n - 1));
                                break;
                            }
                        }
                        Err(f) => {
                            result = Err(f);
                            break;
                        }
                    }
                    result = Ok(status);
                }
                sh.loop_depth -= 1;
                result
            }),
            Command::Case { word, items, redirects } => self.with_redirects(redirects, |sh| {
                let value = match expand::expand_string(sh, word) {
                    Ok(v) => v,
                    Err(e) => return sh.expansion_failed(e),
                };
                for item in items {
                    for pat in &item.patterns {
                        let p = match expand::expand_pattern(sh, pat) {
                            Ok(p) => p,
                            Err(e) => return sh.expansion_failed(e),
                        };
                        if glob::matches(&p, &value) {
                            return sh.run_program(&item.body);
                        }
                    }
                }
                Ok(0)
            }),
            Command::Function { name, body } => {
                self.functions.insert(name.clone(), Rc::new((**body).clone()));
                Ok(0)
            }
        }
    }

    /// Expansion errors (`${x?}`, `set -u`, arithmetic) abort the command; a
    /// non-interactive shell exits. Status 2 like dash.
    fn expansion_failed(&mut self, e: ExpandError) -> ExecResult {
        self.err(&e.0);
        self.status = 2;
        if self.opts.interactive { Ok(2) } else { Err(Flow::Exit(2)) }
    }

    fn run_simple(&mut self, sc: &SimpleCommand) -> ExecResult {
        self.subst_status = None;
        let argv = match expand::expand_words(self, &sc.words) {
            Ok(v) => v,
            Err(e) => return self.expansion_failed(e),
        };
        let mut assigns = Vec::new();
        for a in &sc.assignments {
            match expand::expand_string(self, &a.value) {
                Ok(v) => assigns.push((a.name.clone(), v)),
                Err(e) => return self.expansion_failed(e),
            }
        }
        if self.opts.xtrace {
            let mut line = String::from("+");
            for (k, v) in &assigns {
                line.push_str(&format!(" {k}={}", quote_for_trace(v)));
            }
            for a in &argv {
                line.push(' ');
                line.push_str(&quote_for_trace(a));
            }
            let _ = sys::write_all(2, format!("{line}\n").as_bytes());
        }

        if argv.is_empty() {
            for (k, v) in &assigns {
                if let Err(e) = self.try_set(k, v) {
                    self.err(&e);
                    return if self.opts.interactive { Ok(1) } else { Err(Flow::Exit(1)) };
                }
            }
            let status = self.subst_status.unwrap_or(0);
            return self.with_redirects(&sc.redirects, |_| Ok(status));
        }

        let name = argv[0].as_str();
        if SPECIAL_BUILTINS.contains(&name) {
            for (k, v) in &assigns {
                if let Err(e) = self.try_set(k, v) {
                    self.err(&e);
                    return Ok(1);
                }
            }
            if name == "exec" {
                return self.builtin_exec(&argv, &sc.redirects, &assigns);
            }
            return self.with_redirects(&sc.redirects, |sh| sh.run_builtin(&argv));
        }
        if let Some(body) = self.functions.get(name).cloned() {
            return self.with_temp_assigns(&assigns, |sh| {
                sh.with_redirects(&sc.redirects, |sh| sh.call_function(&body, &argv))
            });
        }
        if BUILTINS.contains(&name) {
            return self.with_temp_assigns(&assigns, |sh| sh.with_redirects(&sc.redirects, |sh| sh.run_builtin(&argv)));
        }
        // External command.
        if std::mem::take(&mut self.direct_exec) {
            if let Err(e) = self.apply_redirects(&sc.redirects, false) {
                self.err(&e);
                return Ok(1);
            }
            return self.exec_external(&argv, &assigns);
        }
        let text = argv.join(" ");
        let redirs = sc.redirects.clone();
        Ok(self.fork_job(&text, false, move |sh| {
            if let Err(e) = sh.apply_redirects(&redirs, false) {
                sh.err(&e);
                return Ok(1);
            }
            sh.exec_external(&argv, &assigns)
        }))
    }

    fn with_temp_assigns<F: FnOnce(&mut Self) -> ExecResult>(&mut self, assigns: &[(String, String)], f: F) -> ExecResult {
        if assigns.is_empty() {
            return f(self);
        }
        let saved: Vec<(String, Option<Var>)> = assigns.iter().map(|(k, _)| (k.clone(), self.vars.get(k).cloned())).collect();
        for (k, v) in assigns {
            if let Err(e) = self.try_set(k, v) {
                self.err(&e);
                return Ok(1);
            }
            self.export(k);
        }
        let r = f(self);
        for (k, old) in saved {
            match old {
                Some(v) => {
                    self.vars.insert(k, v);
                }
                None => {
                    self.vars.remove(&k);
                }
            }
        }
        r
    }

    pub fn call_function(&mut self, body: &Rc<Command>, argv: &[String]) -> ExecResult {
        if self.func_depth > 1000 {
            self.err(&format!("{}: maximum function nesting exceeded", argv[0]));
            return Ok(1);
        }
        let saved = std::mem::replace(&mut self.positional, argv[1..].to_vec());
        self.func_depth += 1;
        self.locals.push(Vec::new());
        let saved_loop = std::mem::replace(&mut self.loop_depth, 0);
        let r = self.run_command(body);
        self.loop_depth = saved_loop;
        for (name, old) in self.locals.pop().unwrap_or_default().into_iter().rev() {
            match old {
                Some(v) => {
                    self.vars.insert(name, v);
                }
                None => {
                    self.vars.remove(&name);
                }
            }
        }
        self.func_depth -= 1;
        self.positional = saved;
        match r {
            Err(Flow::Return(s)) => Ok(s),
            other => other,
        }
    }

    pub fn in_function(&self) -> bool {
        self.func_depth > 0
    }

    pub fn in_loop(&self) -> usize {
        self.loop_depth
    }

    // -- redirections --------------------------------------------------------------------------

    /// Apply redirections in the current process. With `save`, returns the
    /// previous state so `restore_redirects` can undo them.
    pub fn apply_redirects(&mut self, redirs: &[Redirect], save: bool) -> Result<Vec<(Fd, Option<Fd>)>, String> {
        let mut saved: Vec<(Fd, Option<Fd>)> = Vec::new();
        let save_fd = |fd: Fd, saved: &mut Vec<(Fd, Option<Fd>)>| {
            if save && !saved.iter().any(|(f, _)| *f == fd) {
                let old = if sys::is_open(fd) { sys::dup_high(fd).ok() } else { None };
                saved.push((fd, old));
            }
        };
        for r in redirs {
            let target = match &r.kind {
                RedirKind::HereDoc { body, expand: true } => expand::expand_string(self, body),
                RedirKind::HereDoc { body, expand: false } => expand::expand_string(self, body),
                _ => expand::expand_string(self, &r.target),
            };
            let target = match target {
                Ok(t) => t,
                Err(e) => {
                    self.restore_redirects(saved);
                    return Err(e.0);
                }
            };
            let fail = |sh: &mut Self, saved: Vec<(Fd, Option<Fd>)>, msg: String| {
                sh.restore_redirects(saved);
                Err(msg)
            };
            let open_to = |path: &str, flags: libc::c_int| -> Result<Fd, String> {
                sys::open(path, flags, 0o666).map_err(|e| format!("{path}: {}", describe_io(&e)))
            };
            match &r.kind {
                RedirKind::In | RedirKind::Out | RedirKind::Clobber | RedirKind::Append | RedirKind::ReadWrite => {
                    let (default_fd, flags) = match r.kind {
                        RedirKind::In => (0, libc::O_RDONLY),
                        RedirKind::Out | RedirKind::Clobber => (1, libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC),
                        RedirKind::Append => (1, libc::O_WRONLY | libc::O_CREAT | libc::O_APPEND),
                        _ => (0, libc::O_RDWR | libc::O_CREAT),
                    };
                    let fd = r.fd.unwrap_or(default_fd);
                    if r.kind == RedirKind::Out && self.opts.noclobber {
                        if std::fs::metadata(&target).is_ok_and(|m| m.is_file()) {
                            return fail(self, saved, format!("{target}: cannot overwrite existing file"));
                        }
                    }
                    let new = match open_to(&target, flags) {
                        Ok(f) => f,
                        Err(e) => return fail(self, saved, e),
                    };
                    save_fd(fd, &mut saved);
                    if let Err(e) = move_fd(new, fd) {
                        return fail(self, saved, format!("{fd}: {e}"));
                    }
                }
                RedirKind::OutErr | RedirKind::AppendOutErr => {
                    let flags = if r.kind == RedirKind::OutErr {
                        libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC
                    } else {
                        libc::O_WRONLY | libc::O_CREAT | libc::O_APPEND
                    };
                    let new = match open_to(&target, flags) {
                        Ok(f) => f,
                        Err(e) => return fail(self, saved, e),
                    };
                    save_fd(1, &mut saved);
                    save_fd(2, &mut saved);
                    let _ = sys::dup2(new, 2);
                    let _ = move_fd(new, 1);
                }
                RedirKind::DupIn | RedirKind::DupOut => {
                    let fd = r.fd.unwrap_or(if r.kind == RedirKind::DupIn { 0 } else { 1 });
                    if target == "-" {
                        save_fd(fd, &mut saved);
                        sys::close(fd);
                    } else if let Ok(src) = target.parse::<Fd>() {
                        if !sys::is_open(src) {
                            return fail(self, saved, format!("{src}: bad file descriptor"));
                        }
                        save_fd(fd, &mut saved);
                        if let Err(e) = sys::dup2(src, fd) {
                            return fail(self, saved, format!("{fd}: {e}"));
                        }
                    } else if r.kind == RedirKind::DupOut && r.fd.is_none() {
                        // `>&file` means `&>file`.
                        let new = match open_to(&target, libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC) {
                            Ok(f) => f,
                            Err(e) => return fail(self, saved, e),
                        };
                        save_fd(1, &mut saved);
                        save_fd(2, &mut saved);
                        let _ = sys::dup2(new, 2);
                        let _ = move_fd(new, 1);
                    } else {
                        return fail(self, saved, format!("{target}: ambiguous redirect"));
                    }
                }
                RedirKind::HereDoc { .. } => {
                    let fd = r.fd.unwrap_or(0);
                    let new = match sys::memfd_with(target.as_bytes()) {
                        Ok(f) => f,
                        Err(e) => return fail(self, saved, format!("here-document: {e}")),
                    };
                    save_fd(fd, &mut saved);
                    let _ = move_fd(new, fd);
                }
            }
        }
        Ok(saved)
    }

    pub fn restore_redirects(&mut self, saved: Vec<(Fd, Option<Fd>)>) {
        for (fd, old) in saved.into_iter().rev() {
            match old {
                Some(o) => {
                    let _ = sys::dup2(o, fd);
                    sys::close(o);
                }
                None => sys::close(fd),
            }
        }
    }

    fn with_redirects<F: FnOnce(&mut Self) -> ExecResult>(&mut self, redirs: &[Redirect], f: F) -> ExecResult {
        if redirs.is_empty() {
            return f(self);
        }
        match self.apply_redirects(redirs, true) {
            Ok(saved) => {
                let r = f(self);
                self.restore_redirects(saved);
                r
            }
            Err(e) => {
                self.err(&e);
                Ok(1)
            }
        }
    }

    // -- processes ------------------------------------------------------------------------------

    pub fn job_control(&self) -> bool {
        self.opts.monitor && !self.subshell
    }

    /// Prepare a freshly forked child: process group, terminal, signals.
    fn child_setup(&mut self, pgid: libc::pid_t, foreground: bool) {
        if self.job_control() {
            let pid = sys::getpid();
            let pg = if pgid == 0 { pid } else { pgid };
            sys::setpgid(0, pg);
            if foreground {
                sys::tcsetpgrp(self.tty, pg);
            }
        }
        sys::reset_child_signals();
        for (&sig, action) in &self.traps {
            if sig > 0 && action.is_empty() {
                sys::set_signal(sig, libc::SIG_IGN);
            }
        }
        self.traps.retain(|_, a| a.is_empty());
        self.subshell = true;
        self.opts.monitor = false;
        self.opts.interactive = false;
        self.jobs = JobTable::default();
    }

    /// Fork, run `f` in the child, and wait for it as a (foreground or
    /// background) job. Returns the exit status (0 for background jobs).
    fn fork_job<F: FnOnce(&mut Self) -> ExecResult>(&mut self, text: &str, background: bool, f: F) -> i32 {
        let job_control = self.job_control();
        let pid = match sys::fork() {
            Ok(p) => p,
            Err(e) => {
                self.err(&format!("fork: {e}"));
                return 1;
            }
        };
        if pid == 0 {
            self.child_setup(0, !background);
            if background && !job_control {
                // Asynchronous lists without job control read from /dev/null.
                if let Ok(fd) = sys::open("/dev/null", libc::O_RDONLY, 0) {
                    let _ = sys::dup2(fd, 0);
                    sys::close(fd);
                }
            }
            let status = match f(self) {
                Ok(s) => s,
                Err(Flow::Exit(s)) | Err(Flow::Return(s)) => s,
                Err(_) => self.status,
            };
            self.exit_child(status);
        }
        if self.job_control() {
            sys::setpgid(pid, pid);
        }
        let id = self.jobs.add(pid, &[pid], text, background);
        if background {
            self.last_bg = Some(pid);
            if self.opts.interactive {
                let _ = sys::write_all(2, format!("[{id}] {pid}\n").as_bytes());
            }
            0
        } else {
            self.wait_foreground(id)
        }
    }

    fn exit_child(&mut self, status: i32) -> ! {
        let _ = self.run_exit_trap();
        sys::exit_now(status & 0xff)
    }

    fn spawn_background(&mut self, item: &ListItem) -> i32 {
        let ao = item.and_or.clone();
        self.fork_job(&item.text, true, move |sh| {
            sh.direct_exec = ao.rest.is_empty() && !ao.first.negate && ao.first.commands.len() == 1
                && matches!(ao.first.commands[0], Command::Simple(_));
            sh.run_and_or(&ao)
        });
        0
    }

    fn run_forked_pipeline(&mut self, cmds: &[Command], background: bool, text: &str) -> i32 {
        let n = cmds.len();
        let mut pids = Vec::new();
        let mut pgid: libc::pid_t = 0;
        let mut prev_read: Option<Fd> = None;
        for (i, cmd) in cmds.iter().enumerate() {
            let (r, w) = if i + 1 < n {
                match sys::pipe() {
                    Ok(p) => (Some(p.0), Some(p.1)),
                    Err(e) => {
                        self.err(&format!("pipe: {e}"));
                        break;
                    }
                }
            } else {
                (None, None)
            };
            let pid = match sys::fork() {
                Ok(p) => p,
                Err(e) => {
                    self.err(&format!("fork: {e}"));
                    break;
                }
            };
            if pid == 0 {
                self.child_setup(pgid, !background);
                if let Some(pr) = prev_read {
                    let _ = sys::dup2(pr, 0);
                    sys::close(pr);
                }
                if let Some(w) = w {
                    let _ = sys::dup2(w, 1);
                    sys::close(w);
                }
                if let Some(r) = r {
                    sys::close(r);
                }
                self.direct_exec = matches!(cmd, Command::Simple(_));
                let status = match self.run_command(cmd) {
                    Ok(s) => s,
                    Err(Flow::Exit(s)) | Err(Flow::Return(s)) => s,
                    Err(_) => self.status,
                };
                self.exit_child(status);
            }
            if pgid == 0 {
                pgid = pid;
            }
            if self.job_control() {
                sys::setpgid(pid, pgid);
            }
            pids.push(pid);
            if let Some(pr) = prev_read.take() {
                sys::close(pr);
            }
            if let Some(w) = w {
                sys::close(w);
            }
            prev_read = r;
        }
        if let Some(pr) = prev_read {
            sys::close(pr);
        }
        if pids.is_empty() {
            return 1;
        }
        let id = self.jobs.add(pgid, &pids, text, background);
        if background {
            self.last_bg = pids.last().copied();
            0
        } else {
            self.wait_foreground(id)
        }
    }

    /// Wait until job `id` finishes or stops; give the terminal back to the shell.
    pub fn wait_foreground(&mut self, id: usize) -> i32 {
        loop {
            match self.jobs.get(id) {
                Some(j) if j.state() == JobState::Running => {}
                _ => break,
            }
            match sys::waitpid(-1, libc::WUNTRACED) {
                Some((pid, ws)) => {
                    self.jobs.update(pid, ws);
                }
                None => break,
            }
        }
        if self.job_control() {
            sys::tcsetpgrp(self.tty, self.shell_pgid);
        }
        let Some(job) = self.jobs.get(id).cloned() else { return 0 };
        let status = job.status(self.opts.pipefail);
        match job.state() {
            JobState::Stopped => {
                if self.job_control() {
                    if let Some(j) = self.jobs.get_mut(id) {
                        j.termios = sys::get_termios(self.tty);
                        j.background = true;
                        j.reported = Some(JobState::Stopped);
                    }
                    self.jobs.touch(id);
                    if let Some(t) = &self.shell_termios {
                        sys::set_termios(self.tty, t);
                    }
                    let line = self.jobs.format(self.jobs.get(id).unwrap(), false);
                    let _ = sys::write_all(2, format!("\n{line}\n").as_bytes());
                }
            }
            _ => {
                if self.opts.interactive {
                    if let Some(crate::jobs::ProcState::Done(WaitStatus::Signaled(sig, core))) = job.procs.last().map(|p| p.state) {
                        if sig == libc::SIGINT {
                            let _ = sys::write_all(2, b"\n");
                        } else if let Some(d) = sys::signal_description(sig) {
                            let core = if core { " (core dumped)" } else { "" };
                            let _ = sys::write_all(2, format!("{d}{core}\n").as_bytes());
                        }
                    }
                    if let Some(t) = &self.shell_termios {
                        sys::set_termios(self.tty, t);
                    }
                }
                self.jobs.remove(id);
            }
        }
        status
    }

    /// Poll for background job changes (non-blocking) and report them.
    pub fn reap_and_notify(&mut self, notify: bool) {
        while let Some((pid, ws)) = sys::waitpid(-1, libc::WNOHANG | libc::WUNTRACED | libc::WCONTINUED) {
            self.jobs.update(pid, ws);
        }
        let mut done = Vec::new();
        let mut lines = Vec::new();
        for job in &self.jobs.jobs {
            let st = job.state();
            if job.reported != Some(st) && job.background {
                if notify && st != JobState::Running {
                    lines.push(self.jobs.format(job, false));
                }
                if st == JobState::Done {
                    done.push(job.id);
                }
            }
        }
        for job in &mut self.jobs.jobs {
            job.reported = Some(job.state());
        }
        for l in lines {
            let _ = sys::write_all(2, format!("{l}\n").as_bytes());
        }
        for id in done {
            self.jobs.remove(id);
        }
    }

    /// Continue a job in the foreground or background.
    pub fn continue_job(&mut self, id: usize, foreground: bool) -> i32 {
        let Some(job) = self.jobs.get(id).cloned() else { return 1 };
        if foreground {
            self.out(&format!("{}\n", job.text));
            if self.job_control() {
                if let Some(t) = &job.termios {
                    sys::set_termios(self.tty, t);
                }
                sys::tcsetpgrp(self.tty, job.pgid);
            }
        } else {
            self.out(&format!("[{}]{} {} &\n", job.id, self.jobs.marker(job.id), job.text));
        }
        if let Some(j) = self.jobs.get_mut(id) {
            for p in &mut j.procs {
                if matches!(p.state, crate::jobs::ProcState::Stopped(_)) {
                    p.state = crate::jobs::ProcState::Running;
                }
            }
            j.background = !foreground;
            j.reported = Some(JobState::Running);
        }
        let target = if self.job_control() { -job.pgid } else { job.pgid };
        let _ = sys::kill(target, libc::SIGCONT);
        if foreground { self.wait_foreground(id) } else { 0 }
    }

    // -- external commands ------------------------------------------------------------------

    pub fn find_in_path(&mut self, name: &str) -> Option<String> {
        if name.contains('/') {
            return Some(name.to_string());
        }
        if let Some(p) = self.path_cache.get(name) {
            if sys::access_x(p) {
                return Some(p.clone());
            }
        }
        let path = self.get("PATH").unwrap_or_default();
        for dir in path.split(':') {
            let dir = if dir.is_empty() { "." } else { dir };
            let candidate = format!("{dir}/{name}");
            if let Ok(m) = std::fs::metadata(&candidate) {
                if m.is_file() && sys::access_x(&candidate) {
                    self.path_cache.insert(name.to_string(), candidate.clone());
                    return Some(candidate);
                }
            }
        }
        None
    }

    /// Replace the current (child) process with `argv`. Never returns normally
    /// except with a status for scripts without a #! line.
    fn exec_external(&mut self, argv: &[String], assigns: &[(String, String)]) -> ExecResult {
        let Some(path) = self.find_in_path(&argv[0]) else {
            self.err(&format!("{}: command not found", argv[0]));
            return Ok(127);
        };
        let env = self.env_list(assigns);
        let e = sys::execve(&path, argv, &env);
        match e.raw_os_error() {
            Some(libc::ENOEXEC) => {
                // A script without #!: run it with this shell (we are already a fork).
                let src = std::fs::read_to_string(&path).unwrap_or_default();
                for (k, v) in assigns {
                    self.set(k, v);
                    self.export(k);
                }
                self.arg0 = argv[0].clone();
                self.positional = argv[1..].to_vec();
                self.functions.clear();
                self.run_source(&src)
            }
            Some(libc::ENOENT) => {
                self.err(&format!("{}: not found", argv[0]));
                Ok(127)
            }
            _ => {
                self.err(&format!("{}: {}", argv[0], describe_io(&e)));
                Ok(126)
            }
        }
    }

    fn builtin_exec(&mut self, argv: &[String], redirs: &[Redirect], assigns: &[(String, String)]) -> ExecResult {
        if argv.len() == 1 {
            // Permanent redirections.
            return match self.apply_redirects(redirs, false) {
                Ok(_) => Ok(0),
                Err(e) => {
                    self.err(&e);
                    Ok(1)
                }
            };
        }
        if let Err(e) = self.apply_redirects(redirs, false) {
            self.err(&e);
            return Ok(1);
        }
        sys::reset_child_signals();
        let r = self.exec_external(&argv[1..], assigns);
        // exec failed: a non-interactive shell exits.
        let status = r.unwrap_or(127);
        if self.opts.interactive { Ok(status) } else { Err(Flow::Exit(status)) }
    }

    // -- traps -----------------------------------------------------------------------------------

    pub fn set_trap(&mut self, sig: i32, action: Option<String>) {
        match action {
            None => {
                self.traps.remove(&sig);
                if sig > 0 {
                    let interactive_ignored = self.opts.interactive && sys::JOB_SIGNALS.contains(&sig);
                    sys::set_signal(sig, if interactive_ignored { libc::SIG_IGN } else { libc::SIG_DFL });
                }
            }
            Some(a) => {
                if sig > 0 {
                    if a.is_empty() {
                        sys::set_signal(sig, libc::SIG_IGN);
                    } else {
                        sys::set_signal(sig, trap_handler as *const () as libc::sighandler_t);
                    }
                }
                self.traps.insert(sig, a);
            }
        }
    }

    fn run_traps(&mut self) -> Result<(), Flow> {
        for sig in 1..32 {
            if PENDING[sig].swap(false, Ordering::SeqCst) {
                if let Some(action) = self.traps.get(&(sig as i32)).cloned() {
                    let saved = self.status;
                    self.run_source(&action)?;
                    self.status = saved;
                }
            }
        }
        Ok(())
    }

    pub fn run_exit_trap(&mut self) -> Result<(), Flow> {
        if let Some(action) = self.traps.remove(&0) {
            let saved = self.status;
            let r = self.run_source(&action);
            self.status = saved;
            r?;
        }
        Ok(())
    }
}

impl Env for Shell {
    fn get_var(&self, name: &str) -> Option<String> {
        self.get(name)
    }

    fn set_var(&mut self, name: &str, value: &str) -> Result<(), String> {
        self.try_set(name, value)
    }

    fn positional(&self) -> Vec<String> {
        self.positional.clone()
    }

    fn special(&self, c: char) -> Option<String> {
        match c {
            '?' => Some(self.status.to_string()),
            '$' => Some(self.top_pid.to_string()),
            '!' => self.last_bg.map(|p| p.to_string()),
            '0' => Some(self.arg0.clone()),
            '-' => {
                let mut f = String::new();
                if self.opts.errexit {
                    f.push('e');
                }
                if self.opts.noglob {
                    f.push('f');
                }
                if self.opts.interactive {
                    f.push('i');
                }
                if self.opts.monitor {
                    f.push('m');
                }
                if self.opts.nounset {
                    f.push('u');
                }
                if self.opts.xtrace {
                    f.push('x');
                }
                if self.opts.noclobber {
                    f.push('C');
                }
                Some(f)
            }
            _ => None,
        }
    }

    fn command_subst(&mut self, prog: &Program) -> String {
        let (r, w) = match sys::pipe() {
            Ok(p) => p,
            Err(e) => {
                self.err(&format!("pipe: {e}"));
                return String::new();
            }
        };
        let pid = match sys::fork() {
            Ok(p) => p,
            Err(e) => {
                self.err(&format!("fork: {e}"));
                sys::close(r);
                sys::close(w);
                return String::new();
            }
        };
        if pid == 0 {
            sys::close(r);
            let _ = sys::dup2(w, 1);
            sys::close(w);
            sys::reset_child_signals();
            self.subshell = true;
            self.opts.monitor = false;
            self.opts.interactive = false;
            self.jobs = JobTable::default();
            self.traps.retain(|_, a| a.is_empty());
            self.direct_exec = single_simple(prog);
            let status = match self.run_program(prog) {
                Ok(s) => s,
                Err(Flow::Exit(s)) | Err(Flow::Return(s)) => s,
                Err(_) => self.status,
            };
            self.exit_child(status);
        }
        sys::close(w);
        let mut out = Vec::new();
        let mut buf = [0u8; 8192];
        while let Ok(n) = sys::read(r, &mut buf) {
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
        }
        sys::close(r);
        let status = match sys::waitpid(pid, 0) {
            Some((_, ws)) => ws.code(),
            None => 0,
        };
        self.status = status;
        self.subst_status = Some(status);
        let mut s = String::from_utf8_lossy(&out).into_owned();
        while s.ends_with('\n') {
            s.pop();
        }
        s
    }

    fn home_dir(&self, user: &str) -> Option<String> {
        let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
        passwd.lines().find_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            (f.len() >= 6 && f[0] == user).then(|| f[5].to_string())
        })
    }

    fn nounset(&self) -> bool {
        self.opts.nounset
    }

    fn noglob(&self) -> bool {
        self.opts.noglob
    }
}

/// Is the program exactly one simple command (eligible for exec without fork)?
fn single_simple(prog: &Program) -> bool {
    prog.0.len() == 1
        && !prog.0[0].background
        && prog.0[0].and_or.rest.is_empty()
        && !prog.0[0].and_or.first.negate
        && prog.0[0].and_or.first.commands.len() == 1
        && matches!(prog.0[0].and_or.first.commands[0], Command::Simple(_))
}

/// Make `new` available as `fd` (closing `new` unless it already is `fd`).
fn move_fd(new: Fd, fd: Fd) -> std::io::Result<()> {
    let r = sys::dup2(new, fd);
    if new != fd {
        sys::close(new);
    }
    r
}

pub fn describe_io(e: &std::io::Error) -> String {
    match e.raw_os_error() {
        Some(libc::ENOENT) => "No such file or directory".into(),
        Some(libc::EACCES) => "Permission denied".into(),
        Some(libc::EISDIR) => "Is a directory".into(),
        Some(libc::ENOTDIR) => "Not a directory".into(),
        Some(libc::EEXIST) => "File exists".into(),
        _ => {
            let s = e.to_string();
            s.split(" (os error").next().unwrap_or(&s).to_string()
        }
    }
}

fn quote_for_trace(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || "-_./=:,+@%".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

fn pipeline_text(pl: &Pipeline) -> String {
    // Reconstructed for job listings when the source text is not at hand.
    pl.commands
        .iter()
        .map(|c| match c {
            Command::Simple(s) => s
                .words
                .iter()
                .map(|w| parser::literal_text(w).unwrap_or_else(|| "…".into()))
                .collect::<Vec<_>>()
                .join(" "),
            _ => "…".into(),
        })
        .collect::<Vec<_>>()
        .join(" | ")
}
