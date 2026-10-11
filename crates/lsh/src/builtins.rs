//! Built-in commands.

use crate::exec::{self, ExecResult, Flow, Shell, Var, BUILTINS, SPECIAL_BUILTINS};
use crate::sys;

impl Shell {
    pub fn run_builtin(&mut self, argv: &[String]) -> ExecResult {
        let args = &argv[1..];
        let name = argv[0].as_str();
        match name {
            ":" | "true" => Ok(0),
            "false" => Ok(1),
            "echo" => Ok(self.bi_echo(args)),
            "printf" => Ok(self.bi_printf(args)),
            "cd" => Ok(self.bi_cd(args)),
            "pwd" => Ok(self.bi_pwd(args)),
            "test" => Ok(self.bi_test(args, false)),
            "[" => Ok(self.bi_test(args, true)),
            "exit" => {
                let code = match args.first() {
                    Some(a) => match a.parse::<i32>() {
                        Ok(c) => c & 0xff,
                        Err(_) => {
                            self.err(&format!("exit: {a}: numeric argument required"));
                            2
                        }
                    },
                    None => self.status,
                };
                Err(Flow::Exit(code))
            }
            "return" => {
                if !self.in_function() && !self.sourcing() {
                    self.err("return: can only be used in a function or sourced script");
                    return Ok(1);
                }
                let code = args.first().and_then(|a| a.parse::<i32>().ok()).unwrap_or(self.status);
                Err(Flow::Return(code & 0xff))
            }
            "break" | "continue" => {
                let n = match args.first().map(|a| a.parse::<usize>()) {
                    None => 1,
                    Some(Ok(n)) if n > 0 => n,
                    _ => {
                        self.err(&format!("{name}: bad loop count"));
                        return Ok(1);
                    }
                };
                let depth = self.in_loop();
                if depth == 0 {
                    return Ok(0);
                }
                let n = n.min(depth);
                Err(if name == "break" { Flow::Break(n) } else { Flow::Continue(n) })
            }
            "export" | "readonly" => Ok(self.bi_export(name, args)),
            "unset" => Ok(self.bi_unset(args)),
            "local" => Ok(self.bi_local(args)),
            "set" => Ok(self.bi_set(args)),
            "shift" => {
                let n = args.first().and_then(|a| a.parse::<usize>().ok()).unwrap_or(1);
                if n > self.positional.len() {
                    self.err("shift: shift count out of range");
                    return Ok(1);
                }
                self.positional.drain(..n);
                Ok(0)
            }
            "eval" => {
                let src = args.join(" ");
                self.run_source(&src)
            }
            "." | "source" => self.bi_source(args),
            "trap" => Ok(self.bi_trap(args)),
            "alias" => Ok(self.bi_alias(args)),
            "unalias" => {
                if args.first().map(String::as_str) == Some("-a") {
                    self.aliases.clear();
                    return Ok(0);
                }
                let mut st = 0;
                for a in args {
                    if self.aliases.remove(a).is_none() {
                        self.err(&format!("unalias: {a}: not found"));
                        st = 1;
                    }
                }
                Ok(st)
            }
            "read" => Ok(self.bi_read(args)),
            "type" => Ok(self.bi_type(args, false)),
            "command" => self.bi_command(args),
            "hash" => Ok(0),
            "jobs" => {
                self.reap_and_notify(false);
                let long = args.iter().any(|a| a == "-l");
                let mut out = String::new();
                for j in &self.jobs.jobs {
                    out.push_str(&self.jobs.format(j, long));
                    out.push('\n');
                }
                self.out(&out);
                Ok(0)
            }
            "fg" | "bg" => {
                if !self.job_control() {
                    self.err(&format!("{name}: no job control"));
                    return Ok(1);
                }
                match self.jobs.resolve(args.first().map(String::as_str)) {
                    Ok(id) => Ok(self.continue_job(id, name == "fg")),
                    Err(e) => {
                        self.err(&format!("{name}: {e}"));
                        Ok(1)
                    }
                }
            }
            "wait" => Ok(self.bi_wait(args)),
            "kill" => Ok(self.bi_kill(args)),
            "umask" => Ok(self.bi_umask(args)),
            "history" => {
                let mut out = String::new();
                for (i, h) in self.history.iter().enumerate() {
                    out.push_str(&format!("{:5}  {h}\n", i + 1));
                }
                self.out(&out);
                Ok(0)
            }
            "getopts" => Ok(self.bi_getopts(args)),
            "times" => {
                // SAFETY: tms is plain data.
                let mut t: libc::tms = unsafe { std::mem::zeroed() };
                unsafe { libc::times(&mut t) };
                let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as f64;
                let f = |v: libc::clock_t| {
                    let s = v as f64 / hz;
                    format!("{}m{:.3}s", (s / 60.0) as u64, s % 60.0)
                };
                self.out(&format!("{} {}\n{} {}\n", f(t.tms_utime), f(t.tms_stime), f(t.tms_cutime), f(t.tms_cstime)));
                Ok(0)
            }
            "help" => {
                let mut names: Vec<&str> = SPECIAL_BUILTINS.iter().chain(BUILTINS.iter()).copied().collect();
                names.sort();
                self.out(&format!("lsh built-in commands:\n  {}\n", names.join(" ")));
                Ok(0)
            }
            _ => {
                self.err(&format!("{name}: not a builtin"));
                Ok(1)
            }
        }
    }

    fn sourcing(&self) -> bool {
        self.get("__LSH_SOURCING").is_some_and(|v| v != "0")
    }

    fn bi_echo(&mut self, args: &[String]) -> i32 {
        let mut newline = true;
        let mut escapes = false;
        let mut i = 0;
        while i < args.len() {
            let a = &args[i];
            if a.len() > 1 && a.starts_with('-') && a[1..].chars().all(|c| "neE".contains(c)) {
                for c in a[1..].chars() {
                    match c {
                        'n' => newline = false,
                        'e' => escapes = true,
                        _ => escapes = false,
                    }
                }
                i += 1;
            } else {
                break;
            }
        }
        let mut out = String::new();
        for (j, a) in args[i..].iter().enumerate() {
            if j > 0 {
                out.push(' ');
            }
            if escapes {
                let (s, stop) = unescape(a, true);
                out.push_str(&s);
                if stop {
                    self.out(&out);
                    return 0;
                }
            } else {
                out.push_str(a);
            }
        }
        if newline {
            out.push('\n');
        }
        self.out(&out);
        0
    }

    fn bi_printf(&mut self, args: &[String]) -> i32 {
        let Some(fmt) = args.first() else {
            self.err("printf: usage: printf format [arguments]");
            return 2;
        };
        let mut rest = &args[1..];
        let mut out = String::new();
        let mut status = 0;
        loop {
            let used = match format_printf(fmt, rest, &mut out) {
                Ok(n) => n,
                Err(e) => {
                    self.err(&format!("printf: {e}"));
                    status = 1;
                    break;
                }
            };
            if used == 0 || used >= rest.len() {
                break;
            }
            rest = &rest[used..];
        }
        if out.contains('\u{0}') {
            // \c: stop output
            out.truncate(out.find('\u{0}').unwrap());
        }
        self.out(&out);
        status
    }

    fn bi_cd(&mut self, args: &[String]) -> i32 {
        let args: Vec<&String> = args.iter().filter(|a| *a != "-L" && *a != "-P").collect();
        let (target, print) = match args.first().map(|s| s.as_str()) {
            None => match self.get("HOME") {
                Some(h) => (h, false),
                None => {
                    self.err("cd: HOME not set");
                    return 1;
                }
            },
            Some("-") => match self.get("OLDPWD") {
                Some(o) => (o, true),
                None => {
                    self.err("cd: OLDPWD not set");
                    return 1;
                }
            },
            Some(d) => (d.to_string(), false),
        };
        let old = self.get("PWD").unwrap_or_else(|| std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default());
        // Logical path handling: resolve "." and ".." textually against $PWD.
        let logical = if target.starts_with('/') { target.clone() } else { format!("{old}/{target}") };
        let mut parts: Vec<&str> = Vec::new();
        for p in logical.split('/') {
            match p {
                "" | "." => {}
                ".." => {
                    parts.pop();
                }
                p => parts.push(p),
            }
        }
        let normalized = format!("/{}", parts.join("/"));
        let dest = if std::path::Path::new(&normalized).is_dir() { normalized } else { target.clone() };
        if let Err(e) = std::env::set_current_dir(&dest) {
            self.err(&format!("cd: {target}: {}", exec::describe_io(&e)));
            return 1;
        }
        let new = if dest.starts_with('/') {
            dest
        } else {
            std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or(dest)
        };
        self.set("OLDPWD", &old);
        self.set("PWD", &new);
        self.export("PWD");
        if print {
            self.out(&format!("{new}\n"));
        }
        0
    }

    fn bi_pwd(&mut self, args: &[String]) -> i32 {
        let physical = args.iter().any(|a| a == "-P");
        let pwd = if physical {
            std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
        } else {
            self.get("PWD").unwrap_or_else(|| std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default())
        };
        self.out(&format!("{pwd}\n"));
        0
    }

    fn bi_test(&mut self, args: &[String], bracket: bool) -> i32 {
        let mut args: Vec<&str> = args.iter().map(String::as_str).collect();
        if bracket {
            if args.last() != Some(&"]") {
                self.err("[: missing ']'");
                return 2;
            }
            args.pop();
        }
        match test_expr(&args) {
            Ok(true) => 0,
            Ok(false) => 1,
            Err(e) => {
                self.err(&format!("{}: {e}", if bracket { "[" } else { "test" }));
                2
            }
        }
    }

    fn bi_export(&mut self, name: &str, args: &[String]) -> i32 {
        let readonly = name == "readonly";
        let names: Vec<&String> = args.iter().filter(|a| *a != "-p").collect();
        if names.is_empty() {
            let mut list: Vec<(&String, &Var)> =
                self.vars.iter().filter(|(_, v)| if readonly { v.readonly } else { v.exported }).collect();
            list.sort_by(|a, b| a.0.cmp(b.0));
            let mut out = String::new();
            for (k, v) in list {
                out.push_str(&format!("{name} {k}='{}'\n", v.value.replace('\'', "'\\''")));
            }
            self.out(&out);
            return 0;
        }
        let mut status = 0;
        for a in names {
            let (k, v) = match a.split_once('=') {
                Some((k, v)) => (k, Some(v)),
                None => (a.as_str(), None),
            };
            if !crate::parser::is_name(k) {
                self.err(&format!("{name}: '{a}': not a valid identifier"));
                status = 1;
                continue;
            }
            if let Some(v) = v {
                if let Err(e) = self.try_set(k, v) {
                    self.err(&e);
                    status = 1;
                    continue;
                }
            }
            let var = self.vars.entry(k.to_string()).or_default();
            if readonly {
                var.readonly = true;
            } else {
                var.exported = true;
            }
        }
        status
    }

    fn bi_unset(&mut self, args: &[String]) -> i32 {
        let mut functions = false;
        let mut status = 0;
        for a in args {
            match a.as_str() {
                "-f" => functions = true,
                "-v" => functions = false,
                n if functions => {
                    self.functions.remove(n);
                }
                n => {
                    if let Err(e) = self.unset(n) {
                        self.err(&format!("unset: {e}"));
                        status = 1;
                    }
                }
            }
        }
        status
    }

    fn bi_local(&mut self, args: &[String]) -> i32 {
        for a in args {
            let (k, v) = match a.split_once('=') {
                Some((k, v)) => (k, Some(v)),
                None => (a.as_str(), None),
            };
            if let Err(e) = self.make_local(k) {
                self.err(&e);
                return 1;
            }
            match v {
                Some(v) => self.set(k, v),
                None => {
                    // A new local starts unset unless it already has a value.
                }
            }
        }
        0
    }

    fn bi_set(&mut self, args: &[String]) -> i32 {
        if args.is_empty() {
            let mut list: Vec<_> = self.vars.iter().collect();
            list.sort_by(|a, b| a.0.cmp(b.0));
            let mut out = String::new();
            for (k, v) in list {
                out.push_str(&format!("{k}='{}'\n", v.value.replace('\'', "'\\''")));
            }
            self.out(&out);
            return 0;
        }
        let mut i = 0;
        while i < args.len() {
            let a = &args[i];
            if a == "--" {
                self.positional = args[i + 1..].to_vec();
                return 0;
            }
            if a == "-o" || a == "+o" {
                let on = a == "-o";
                match args.get(i + 1).map(String::as_str) {
                    Some(opt) => {
                        if !self.set_named_option(opt, on) {
                            self.err(&format!("set: {opt}: invalid option name"));
                            return 1;
                        }
                        i += 2;
                        continue;
                    }
                    None => {
                        let o = &self.opts;
                        let list = [
                            ("errexit", o.errexit),
                            ("noglob", o.noglob),
                            ("noclobber", o.noclobber),
                            ("nounset", o.nounset),
                            ("pipefail", o.pipefail),
                            ("xtrace", o.xtrace),
                            ("monitor", o.monitor),
                        ];
                        let mut out = String::new();
                        for (n, v) in list {
                            out.push_str(&format!("{n:<12}{}\n", if v { "on" } else { "off" }));
                        }
                        self.out(&out);
                        return 0;
                    }
                }
            }
            if (a.starts_with('-') || a.starts_with('+')) && a.len() > 1 {
                let on = a.starts_with('-');
                for c in a[1..].chars() {
                    let ok = match c {
                        'e' => self.set_named_option("errexit", on),
                        'u' => self.set_named_option("nounset", on),
                        'x' => self.set_named_option("xtrace", on),
                        'f' => self.set_named_option("noglob", on),
                        'C' => self.set_named_option("noclobber", on),
                        'm' => self.set_named_option("monitor", on),
                        _ => false,
                    };
                    if !ok {
                        self.err(&format!("set: -{c}: invalid option"));
                        return 1;
                    }
                }
                i += 1;
                continue;
            }
            self.positional = args[i..].to_vec();
            return 0;
        }
        0
    }

    fn set_named_option(&mut self, name: &str, on: bool) -> bool {
        match name {
            "errexit" => self.opts.errexit = on,
            "nounset" => self.opts.nounset = on,
            "xtrace" => self.opts.xtrace = on,
            "noglob" => self.opts.noglob = on,
            "noclobber" => self.opts.noclobber = on,
            "pipefail" => self.opts.pipefail = on,
            "monitor" => self.opts.monitor = on && self.opts.interactive,
            _ => return false,
        }
        true
    }

    fn bi_source(&mut self, args: &[String]) -> ExecResult {
        let Some(file) = args.first() else {
            self.err(".: filename argument required");
            return Ok(2);
        };
        let path = if file.contains('/') {
            file.clone()
        } else {
            self.find_readable_in_path(file).unwrap_or_else(|| file.clone())
        };
        let src = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                self.err(&format!("{file}: {}", exec::describe_io(&e)));
                return if self.opts.interactive { Ok(1) } else { Err(Flow::Exit(1)) };
            }
        };
        let saved_args = (args.len() > 1).then(|| std::mem::replace(&mut self.positional, args[1..].to_vec()));
        let depth: i32 = self.get("__LSH_SOURCING").and_then(|v| v.parse().ok()).unwrap_or(0);
        self.set("__LSH_SOURCING", &(depth + 1).to_string());
        let r = self.run_source(&src);
        self.set("__LSH_SOURCING", &depth.to_string());
        if let Some(a) = saved_args {
            self.positional = a;
        }
        match r {
            Err(Flow::Return(s)) => Ok(s),
            other => other,
        }
    }

    fn find_readable_in_path(&self, name: &str) -> Option<String> {
        let path = self.get("PATH").unwrap_or_default();
        path.split(':').map(|d| format!("{}/{name}", if d.is_empty() { "." } else { d })).find(|p| std::path::Path::new(p).is_file())
    }

    fn bi_trap(&mut self, args: &[String]) -> i32 {
        if args.is_empty() {
            let mut list: Vec<_> = self.traps.iter().collect();
            list.sort();
            let mut out = String::new();
            for (sig, action) in list {
                let name = if *sig == 0 { "EXIT".to_string() } else { sys::signal_name(*sig).to_string() };
                out.push_str(&format!("trap -- '{}' {name}\n", action.replace('\'', "'\\''")));
            }
            self.out(&out);
            return 0;
        }
        let (action, sigs) = if args.len() == 1 || args[0] == "-" {
            (None, if args.len() == 1 { &args[..] } else { &args[1..] })
        } else {
            (Some(args[0].clone()), &args[1..])
        };
        let mut status = 0;
        for s in sigs {
            let sig = if s == "EXIT" || s == "0" { Some(0) } else { sys::signal_number(s) };
            match sig {
                Some(n) if n != libc::SIGKILL && n != libc::SIGSTOP => self.set_trap(n, action.clone()),
                _ => {
                    self.err(&format!("trap: {s}: invalid signal specification"));
                    status = 1;
                }
            }
        }
        status
    }

    fn bi_alias(&mut self, args: &[String]) -> i32 {
        if args.is_empty() {
            let mut list: Vec<_> = self.aliases.iter().collect();
            list.sort();
            let mut out = String::new();
            for (k, v) in list {
                out.push_str(&format!("alias {k}='{}'\n", v.replace('\'', "'\\''")));
            }
            self.out(&out);
            return 0;
        }
        let mut status = 0;
        for a in args {
            match a.split_once('=') {
                Some((k, v)) => {
                    self.aliases.insert(k.to_string(), v.to_string());
                }
                None => match self.aliases.get(a) {
                    Some(v) => self.out(&format!("alias {a}='{}'\n", v.replace('\'', "'\\''"))),
                    None => {
                        self.err(&format!("alias: {a}: not found"));
                        status = 1;
                    }
                },
            }
        }
        status
    }

    fn bi_read(&mut self, args: &[String]) -> i32 {
        let mut raw = false;
        let mut prompt = None;
        let mut delim = b'\n';
        let mut names = Vec::new();
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "-r" => raw = true,
                "-p" => {
                    prompt = args.get(i + 1).cloned();
                    i += 1;
                }
                "-d" => {
                    delim = args.get(i + 1).and_then(|d| d.bytes().next()).unwrap_or(0);
                    i += 1;
                }
                n => names.push(n.to_string()),
            }
            i += 1;
        }
        if names.is_empty() {
            names.push("REPLY".to_string());
        }
        if let Some(p) = prompt {
            if sys::isatty(0) {
                let _ = sys::write_all(2, p.as_bytes());
            }
        }
        // Read byte by byte so nothing beyond the line is consumed.
        let mut line = Vec::new();
        let mut eof = true;
        let mut byte = [0u8; 1];
        while let Ok(1) = sys::read(0, &mut byte) {
            if byte[0] == delim {
                eof = false;
                break;
            }
            if byte[0] == b'\\' && !raw {
                if let Ok(1) = sys::read(0, &mut byte) {
                    if byte[0] != b'\n' {
                        line.push(0x01); // marks an escaped char (kept literal)
                        line.push(byte[0]);
                    }
                    continue;
                }
                break;
            }
            line.push(byte[0]);
        }
        if eof && line.is_empty() {
            for n in &names {
                self.set(n, "");
            }
            return 1;
        }
        let text = String::from_utf8_lossy(&line).into_owned();
        let ifs = self.get("IFS").unwrap_or_else(|| " \t\n".into());
        let fields = split_read(&text, &ifs, names.len());
        for (i, n) in names.iter().enumerate() {
            let v = fields.get(i).cloned().unwrap_or_default().replace('\u{1}', "");
            if let Err(e) = self.try_set(n, &v) {
                self.err(&format!("read: {e}"));
                return 1;
            }
        }
        if eof { 1 } else { 0 }
    }

    fn bi_type(&mut self, args: &[String], short: bool) -> i32 {
        let mut status = 0;
        for a in args {
            let line = if self.aliases.contains_key(a) {
                if short { format!("alias {a}='{}'", self.aliases[a]) } else { format!("{a} is an alias for {}", self.aliases[a]) }
            } else if ["if", "then", "else", "elif", "fi", "case", "esac", "for", "while", "until", "do", "done", "{", "}", "!", "in"].contains(&a.as_str()) {
                if short { a.clone() } else { format!("{a} is a shell keyword") }
            } else if self.functions.contains_key(a) {
                if short { a.clone() } else { format!("{a} is a function") }
            } else if exec::is_builtin(a) {
                if short { a.clone() } else { format!("{a} is a shell builtin") }
            } else if let Some(p) = self.find_in_path(a).filter(|p| std::path::Path::new(p).is_file()) {
                if short { p } else { format!("{a} is {p}") }
            } else {
                if !short {
                    self.err(&format!("type: {a}: not found"));
                }
                status = 1;
                continue;
            };
            self.out(&format!("{line}\n"));
        }
        status
    }

    fn bi_command(&mut self, args: &[String]) -> ExecResult {
        match args.first().map(String::as_str) {
            Some("-v") => Ok(self.bi_type(&args[1..], true)),
            Some("-V") => Ok(self.bi_type(&args[1..], false)),
            Some(_) => {
                // Run skipping functions.
                let name = args[0].as_str();
                if exec::is_builtin(name) {
                    self.run_builtin(args)
                } else {
                    let saved = self.functions.remove(name);
                    let words: Vec<crate::ast::Word> =
                        args.iter().map(|a| crate::ast::Word(vec![crate::ast::WordPart::Quoted(a.clone())])).collect();
                    let cmd = crate::ast::Command::Simple(crate::ast::SimpleCommand { assignments: vec![], words, redirects: vec![] });
                    let r = self.run_command(&cmd);
                    if let Some(f) = saved {
                        self.functions.insert(name.to_string(), f);
                    }
                    r
                }
            }
            None => Ok(0),
        }
    }

    fn bi_wait(&mut self, args: &[String]) -> i32 {
        if args.is_empty() {
            let ids: Vec<usize> = self.jobs.jobs.iter().map(|j| j.id).collect();
            for id in ids {
                self.wait_job_quiet(id);
            }
            return 0;
        }
        let mut status = 0;
        for a in args {
            match self.jobs.resolve(Some(a)) {
                Ok(id) => status = self.wait_job_quiet(id),
                Err(_) => {
                    // Unknown pid: may already have been reaped.
                    status = 127;
                }
            }
        }
        status
    }

    fn wait_job_quiet(&mut self, id: usize) -> i32 {
        loop {
            match self.jobs.get(id) {
                Some(j) if j.state() == crate::jobs::JobState::Running => {}
                Some(j) => {
                    let st = j.status(self.opts.pipefail);
                    if j.state() == crate::jobs::JobState::Done {
                        self.jobs.remove(id);
                    }
                    return st;
                }
                None => return 0,
            }
            match sys::waitpid(-1, libc::WUNTRACED) {
                Some((pid, ws)) => {
                    self.jobs.update(pid, ws);
                }
                None => return 127,
            }
        }
    }

    fn bi_kill(&mut self, args: &[String]) -> i32 {
        let mut sig = libc::SIGTERM;
        let mut targets = &args[..];
        if let Some(first) = args.first() {
            if first == "-l" {
                let names: Vec<String> = (1..32).filter(|s| !sys::signal_name(*s).is_empty()).map(|s| sys::signal_name(s).to_string()).collect();
                self.out(&format!("{}\n", names.join(" ")));
                return 0;
            }
            if first == "-s" {
                match args.get(1).and_then(|s| sys::signal_number(s)) {
                    Some(s) => sig = s,
                    None => {
                        self.err("kill: bad signal");
                        return 1;
                    }
                }
                targets = &args[2..];
            } else if let Some(s) = first.strip_prefix('-') {
                match sys::signal_number(s) {
                    Some(n) => sig = n,
                    None => {
                        self.err(&format!("kill: {s}: invalid signal"));
                        return 1;
                    }
                }
                targets = &args[1..];
            }
        }
        if targets.is_empty() {
            self.err("kill: usage: kill [-s sig | -sig] pid | %job ...");
            return 2;
        }
        let mut status = 0;
        for t in targets {
            let pid = if t.starts_with('%') {
                match self.jobs.resolve(Some(t)) {
                    Ok(id) => -self.jobs.get(id).unwrap().pgid,
                    Err(e) => {
                        self.err(&format!("kill: {e}"));
                        status = 1;
                        continue;
                    }
                }
            } else {
                match t.parse::<i32>() {
                    Ok(p) => p,
                    Err(_) => {
                        self.err(&format!("kill: {t}: arguments must be process or job IDs"));
                        status = 1;
                        continue;
                    }
                }
            };
            if let Err(e) = sys::kill(pid, sig) {
                self.err(&format!("kill: ({t}) - {}", exec::describe_io(&e)));
                status = 1;
            } else if sig == libc::SIGCONT || sig == libc::SIGKILL || sig == libc::SIGTERM {
                // Stopped jobs need SIGCONT to act on TERM.
                if pid < 0 && sig != libc::SIGCONT {
                    let _ = sys::kill(pid, libc::SIGCONT);
                }
            }
        }
        status
    }

    fn bi_umask(&mut self, args: &[String]) -> i32 {
        match args.first() {
            None => {
                let m = sys::umask(None);
                self.out(&format!("{m:04o}\n"));
                0
            }
            Some(m) => match u32::from_str_radix(m, 8) {
                Ok(v) if v <= 0o777 => {
                    sys::umask(Some(v as libc::mode_t));
                    0
                }
                _ => {
                    self.err(&format!("umask: {m}: invalid mode"));
                    1
                }
            },
        }
    }

    fn bi_getopts(&mut self, args: &[String]) -> i32 {
        if args.len() < 2 {
            self.err("getopts: usage: getopts optstring name [arg ...]");
            return 2;
        }
        let optstring = &args[0];
        let name = &args[1];
        let params: Vec<String> = if args.len() > 2 { args[2..].to_vec() } else { self.positional.clone() };
        let silent = optstring.starts_with(':');
        let mut optind: usize = self.get("OPTIND").and_then(|v| v.parse().ok()).unwrap_or(1).max(1);
        if optind == 1 && self.getopts_pos > 0 && self.get("__LSH_OPTIND_SEEN").as_deref() != Some("1") {
            self.getopts_pos = 0;
        }
        let Some(arg) = params.get(optind - 1) else {
            self.set(name, "?");
            self.getopts_pos = 0;
            return 1;
        };
        if self.getopts_pos == 0 {
            if arg == "--" {
                self.set("OPTIND", &(optind + 1).to_string());
                self.set(name, "?");
                return 1;
            }
            if !arg.starts_with('-') || arg == "-" {
                self.set(name, "?");
                return 1;
            }
            self.getopts_pos = 1;
        }
        let chars: Vec<char> = arg.chars().collect();
        let c = chars[self.getopts_pos];
        self.getopts_pos += 1;
        let at_end = self.getopts_pos >= chars.len();
        let spec = optstring.trim_start_matches(':');
        let pos = spec.find(c).filter(|_| c != ':');
        let advance = |sh: &mut Self, optind: &mut usize, extra: usize| {
            *optind += extra;
            sh.set("OPTIND", &optind.to_string());
            sh.getopts_pos = 0;
        };
        match pos {
            None => {
                if silent {
                    self.set("OPTARG", &c.to_string());
                } else {
                    self.err(&format!("illegal option -- {c}"));
                    let _ = self.unset("OPTARG");
                }
                self.set(name, "?");
                if at_end {
                    advance(self, &mut optind, 1);
                }
            }
            Some(p) if spec[p + 1..].starts_with(':') => {
                if !at_end {
                    let value: String = chars[self.getopts_pos..].iter().collect();
                    self.set("OPTARG", &value);
                    self.set(name, &c.to_string());
                    advance(self, &mut optind, 1);
                } else if let Some(v) = params.get(optind) {
                    self.set("OPTARG", v);
                    self.set(name, &c.to_string());
                    advance(self, &mut optind, 2);
                } else {
                    if silent {
                        self.set(name, ":");
                        self.set("OPTARG", &c.to_string());
                    } else {
                        self.err(&format!("option requires an argument -- {c}"));
                        self.set(name, "?");
                    }
                    advance(self, &mut optind, 1);
                }
            }
            Some(_) => {
                self.set(name, &c.to_string());
                let _ = self.unset("OPTARG");
                if at_end {
                    advance(self, &mut optind, 1);
                }
            }
        }
        0
    }
}

/// Process backslash escapes (echo -e, printf %b). Returns (text, stop) where
/// stop means `\c` was seen.
pub fn unescape(s: &str, echo_style: bool) -> (String, bool) {
    let mut out = String::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c != '\\' || i + 1 >= chars.len() {
            out.push(c);
            i += 1;
            continue;
        }
        i += 1;
        match chars[i] {
            'n' => out.push('\n'),
            't' => out.push('\t'),
            'r' => out.push('\r'),
            'a' => out.push('\u{7}'),
            'b' => out.push('\u{8}'),
            'f' => out.push('\u{c}'),
            'v' => out.push('\u{b}'),
            'e' | 'E' => out.push('\u{1b}'),
            '\\' => out.push('\\'),
            'c' => return (out, true),
            '0'..='7' => {
                // \0NNN (echo) or \NNN (printf).
                let mut j = i;
                if echo_style && chars[j] == '0' {
                    j += 1;
                }
                let start = j;
                while j < chars.len() && j - start < 3 && ('0'..='7').contains(&chars[j]) {
                    j += 1;
                }
                let digits: String = chars[start..j].iter().collect();
                let v = u32::from_str_radix(if digits.is_empty() { "0" } else { &digits }, 8).unwrap_or(0);
                out.push(char::from_u32(v & 0xff).unwrap_or('?'));
                i = j;
                continue;
            }
            'x' => {
                let mut j = i + 1;
                while j < chars.len() && j - (i + 1) < 2 && chars[j].is_ascii_hexdigit() {
                    j += 1;
                }
                let digits: String = chars[i + 1..j].iter().collect();
                if digits.is_empty() {
                    out.push_str("\\x");
                } else {
                    out.push(char::from_u32(u32::from_str_radix(&digits, 16).unwrap_or(0)).unwrap_or('?'));
                }
                i = j;
                continue;
            }
            other => {
                out.push('\\');
                out.push(other);
            }
        }
        i += 1;
    }
    (out, false)
}

/// Format one pass of a printf format string; returns how many args were used.
pub fn format_printf(fmt: &str, args: &[String], out: &mut String) -> Result<usize, String> {
    let chars: Vec<char> = fmt.chars().collect();
    let mut used = 0;
    let mut i = 0;
    let next_arg = |used: &mut usize| -> String {
        let v = args.get(*used).cloned().unwrap_or_default();
        *used += 1;
        v
    };
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' {
            let mut j = i + 1;
            // Collect the escape sequence and reuse unescape.
            if j < chars.len() {
                j += 1;
                if chars[i + 1].is_digit(8) {
                    while j < chars.len() && j - (i + 1) < 3 && chars[j].is_digit(8) {
                        j += 1;
                    }
                } else if chars[i + 1] == 'x' {
                    while j < chars.len() && j - (i + 2) < 2 && chars[j].is_ascii_hexdigit() {
                        j += 1;
                    }
                }
            }
            let seq: String = chars[i..j].iter().collect();
            let (s, stop) = unescape(&seq, false);
            out.push_str(&s);
            if stop {
                out.push('\u{0}');
                return Ok(0);
            }
            i = j;
            continue;
        }
        if c != '%' {
            out.push(c);
            i += 1;
            continue;
        }
        i += 1;
        if i >= chars.len() {
            out.push('%');
            break;
        }
        if chars[i] == '%' {
            out.push('%');
            i += 1;
            continue;
        }
        let mut flags = String::new();
        while i < chars.len() && "-+ #0".contains(chars[i]) {
            flags.push(chars[i]);
            i += 1;
        }
        let mut width = String::new();
        if i < chars.len() && chars[i] == '*' {
            width = next_arg(&mut used);
            i += 1;
        } else {
            while i < chars.len() && chars[i].is_ascii_digit() {
                width.push(chars[i]);
                i += 1;
            }
        }
        let mut precision: Option<usize> = None;
        if i < chars.len() && chars[i] == '.' {
            i += 1;
            let mut p = String::new();
            if i < chars.len() && chars[i] == '*' {
                p = next_arg(&mut used);
                i += 1;
            } else {
                while i < chars.len() && chars[i].is_ascii_digit() {
                    p.push(chars[i]);
                    i += 1;
                }
            }
            precision = Some(p.parse().unwrap_or(0));
        }
        let Some(&conv) = chars.get(i) else { return Err("missing format character".into()) };
        i += 1;
        let width: usize = width.trim_start_matches('-').parse().unwrap_or(0);
        let left = flags.contains('-');
        let zero = flags.contains('0') && !left;
        let body = match conv {
            's' => {
                let mut s = next_arg(&mut used);
                if let Some(p) = precision {
                    s = s.chars().take(p).collect();
                }
                s
            }
            'b' => unescape(&next_arg(&mut used), true).0,
            'c' => next_arg(&mut used).chars().next().map(String::from).unwrap_or_default(),
            'd' | 'i' | 'u' | 'x' | 'X' | 'o' => {
                let a = next_arg(&mut used);
                let v = parse_int_arg(&a)?;
                let mut s = match conv {
                    'x' => format!("{:x}", v),
                    'X' => format!("{:X}", v),
                    'o' => format!("{:o}", v),
                    'u' => format!("{}", v as u64),
                    _ => format!("{}", v.unsigned_abs()),
                };
                if let Some(p) = precision {
                    while s.len() < p {
                        s.insert(0, '0');
                    }
                }
                if flags.contains('#') && v != 0 {
                    match conv {
                        'x' => s.insert_str(0, "0x"),
                        'X' => s.insert_str(0, "0X"),
                        'o' => s.insert(0, '0'),
                        _ => {}
                    }
                }
                let sign = if matches!(conv, 'd' | 'i') {
                    if v < 0 {
                        "-"
                    } else if flags.contains('+') {
                        "+"
                    } else if flags.contains(' ') {
                        " "
                    } else {
                        ""
                    }
                } else {
                    ""
                };
                if zero && precision.is_none() {
                    let total = sign.len() + s.len();
                    if width > total {
                        s.insert_str(0, &"0".repeat(width - total));
                    }
                }
                format!("{sign}{s}")
            }
            'f' | 'F' | 'e' | 'E' | 'g' | 'G' => {
                let a = next_arg(&mut used);
                let v: f64 = if a.is_empty() { 0.0 } else { a.trim().parse().map_err(|_| format!("{a}: invalid number"))? };
                let p = precision.unwrap_or(6);
                let mut s = match conv {
                    'f' | 'F' => format!("{v:.p$}"),
                    'e' => fmt_exp(v, p, false),
                    'E' => fmt_exp(v, p, true),
                    _ => {
                        let p = p.max(1);
                        let exp = if v == 0.0 { 0 } else { v.abs().log10().floor() as i32 };
                        if exp < -4 || exp >= p as i32 {
                            fmt_exp(v, p - 1, conv == 'G')
                        } else {
                            let s = format!("{v:.prec$}", prec = (p as i32 - 1 - exp).max(0) as usize);
                            if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s }
                        }
                    }
                };
                if flags.contains('+') && v >= 0.0 {
                    s.insert(0, '+');
                }
                if zero && s.len() < width {
                    let neg = s.starts_with('-') || s.starts_with('+');
                    let at = if neg { 1 } else { 0 };
                    s.insert_str(at, &"0".repeat(width - s.len()));
                }
                s
            }
            other => return Err(format!("%{other}: invalid directive")),
        };
        let len = body.chars().count();
        if len < width {
            let pad = " ".repeat(width - len);
            if left {
                out.push_str(&body);
                out.push_str(&pad);
            } else {
                out.push_str(&pad);
                out.push_str(&body);
            }
        } else {
            out.push_str(&body);
        }
    }
    Ok(used)
}

fn fmt_exp(v: f64, p: usize, upper: bool) -> String {
    let s = format!("{v:.p$e}");
    // Rust prints 1.5e2; C prints 1.5e+02.
    let (mant, exp) = s.split_once('e').unwrap_or((&s, "0"));
    let e: i32 = exp.parse().unwrap_or(0);
    let r = format!("{mant}e{}{:02}", if e < 0 { '-' } else { '+' }, e.abs());
    if upper { r.to_uppercase() } else { r }
}

fn parse_int_arg(a: &str) -> Result<i64, String> {
    let t = a.trim();
    if t.is_empty() {
        return Ok(0);
    }
    if let Some(c) = t.strip_prefix('\'').or_else(|| t.strip_prefix('"')) {
        return Ok(c.chars().next().map(|c| c as i64).unwrap_or(0));
    }
    let (neg, digits) = match t.strip_prefix('-') {
        Some(d) => (true, d),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let v = if let Some(h) = digits.strip_prefix("0x").or_else(|| digits.strip_prefix("0X")) {
        i64::from_str_radix(h, 16)
    } else if digits.len() > 1 && digits.starts_with('0') {
        i64::from_str_radix(&digits[1..], 8)
    } else {
        digits.parse()
    }
    .map_err(|_| format!("{a}: invalid number"))?;
    Ok(if neg { -v } else { v })
}

/// Split a line read by `read` into `n` fields; the last field gets the rest.
fn split_read(text: &str, ifs: &str, n: usize) -> Vec<String> {
    let is_ws = |c: char| ifs.contains(c) && c.is_whitespace();
    let is_sep = |c: char| ifs.contains(c);
    let chars: Vec<char> = text.chars().collect();
    let mut fields = Vec::new();
    let mut i = 0;
    // Skip leading IFS whitespace.
    while i < chars.len() && is_ws(chars[i]) {
        i += 1;
    }
    while i < chars.len() {
        if fields.len() + 1 == n {
            // Last variable: the remainder minus trailing IFS whitespace.
            let mut rest: String = chars[i..].iter().collect();
            while rest.ends_with(|c: char| is_ws(c)) && !rest.ends_with("\u{1} ") {
                rest.pop();
            }
            fields.push(rest);
            return fields;
        }
        let start = i;
        while i < chars.len() && !(is_sep(chars[i]) && (i == 0 || chars[i - 1] != '\u{1}')) {
            i += 1;
        }
        fields.push(chars[start..i].iter().collect());
        // Consume one delimiter plus surrounding whitespace.
        while i < chars.len() && is_ws(chars[i]) {
            i += 1;
        }
        if i < chars.len() && is_sep(chars[i]) && !is_ws(chars[i]) {
            i += 1;
            while i < chars.len() && is_ws(chars[i]) {
                i += 1;
            }
        }
    }
    fields
}

/// Evaluate a `test` expression.
pub fn test_expr(args: &[&str]) -> Result<bool, String> {
    let mut p = TestParser { args, pos: 0 };
    if args.is_empty() {
        return Ok(false);
    }
    let v = p.or()?;
    if p.pos != args.len() {
        return Err(format!("{}: unexpected argument", args[p.pos]));
    }
    Ok(v)
}

struct TestParser<'a> {
    args: &'a [&'a str],
    pos: usize,
}

impl<'a> TestParser<'a> {
    fn peek(&self, off: usize) -> Option<&'a str> {
        self.args.get(self.pos + off).copied()
    }

    fn or(&mut self) -> Result<bool, String> {
        let mut v = self.and()?;
        while self.peek(0) == Some("-o") {
            self.pos += 1;
            let r = self.and()?;
            v = v || r;
        }
        Ok(v)
    }

    fn and(&mut self) -> Result<bool, String> {
        let mut v = self.not()?;
        while self.peek(0) == Some("-a") {
            self.pos += 1;
            let r = self.not()?;
            v = v && r;
        }
        Ok(v)
    }

    fn not(&mut self) -> Result<bool, String> {
        // "!" is negation unless it is the only remaining argument (a string).
        if self.peek(0) == Some("!") && self.args.len() - self.pos > 1 {
            self.pos += 1;
            return Ok(!self.not()?);
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<bool, String> {
        let Some(a) = self.peek(0) else { return Err("argument expected".into()) };
        let remaining = self.args.len() - self.pos;
        if a == "(" && remaining >= 3 {
            self.pos += 1;
            let v = self.or()?;
            if self.peek(0) != Some(")") {
                return Err("')' expected".into());
            }
            self.pos += 1;
            return Ok(v);
        }
        // Binary operators.
        if remaining >= 3 {
            if let Some(op) = self.peek(1) {
                if is_binary_op(op) {
                    let l = a;
                    let r = self.peek(2).unwrap();
                    self.pos += 3;
                    return binary_test(l, op, r);
                }
            }
        }
        if a.len() == 2 && a.starts_with('-') && remaining >= 2 && a != "-a" && a != "-o" {
            let operand = self.peek(1).unwrap();
            self.pos += 2;
            return unary_test(a, operand);
        }
        self.pos += 1;
        Ok(!a.is_empty())
    }
}

fn is_binary_op(op: &str) -> bool {
    matches!(op, "=" | "==" | "!=" | "<" | ">" | "-eq" | "-ne" | "-lt" | "-le" | "-gt" | "-ge" | "-nt" | "-ot" | "-ef")
}

fn binary_test(l: &str, op: &str, r: &str) -> Result<bool, String> {
    let int = |s: &str| -> Result<i64, String> { s.trim().parse::<i64>().map_err(|_| format!("{s}: integer expression expected")) };
    Ok(match op {
        "=" | "==" => l == r,
        "!=" => l != r,
        "<" => l < r,
        ">" => l > r,
        "-eq" => int(l)? == int(r)?,
        "-ne" => int(l)? != int(r)?,
        "-lt" => int(l)? < int(r)?,
        "-le" => int(l)? <= int(r)?,
        "-gt" => int(l)? > int(r)?,
        "-ge" => int(l)? >= int(r)?,
        "-nt" | "-ot" => {
            let m = |p: &str| std::fs::metadata(p).and_then(|m| m.modified()).ok();
            match (m(l), m(r)) {
                (Some(a), Some(b)) => if op == "-nt" { a > b } else { a < b },
                (Some(_), None) => op == "-nt",
                (None, Some(_)) => op == "-ot",
                _ => false,
            }
        }
        "-ef" => {
            use std::os::unix::fs::MetadataExt;
            match (std::fs::metadata(l), std::fs::metadata(r)) {
                (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
                _ => false,
            }
        }
        _ => return Err(format!("{op}: unknown operator")),
    })
}

fn unary_test(op: &str, x: &str) -> Result<bool, String> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
    let meta = || std::fs::metadata(x);
    let access = |mode: libc::c_int| {
        let p = sys::cstring(x);
        // SAFETY: valid C string.
        unsafe { libc::access(p.as_ptr(), mode) == 0 }
    };
    Ok(match op {
        "-z" => x.is_empty(),
        "-n" => !x.is_empty(),
        "-e" => meta().is_ok(),
        "-f" => meta().is_ok_and(|m| m.is_file()),
        "-d" => meta().is_ok_and(|m| m.is_dir()),
        "-s" => meta().is_ok_and(|m| m.len() > 0),
        "-L" | "-h" => std::fs::symlink_metadata(x).is_ok_and(|m| m.file_type().is_symlink()),
        "-b" => meta().is_ok_and(|m| m.file_type().is_block_device()),
        "-c" => meta().is_ok_and(|m| m.file_type().is_char_device()),
        "-p" => meta().is_ok_and(|m| m.file_type().is_fifo()),
        "-S" => meta().is_ok_and(|m| m.file_type().is_socket()),
        "-r" => access(libc::R_OK),
        "-w" => access(libc::W_OK),
        "-x" => access(libc::X_OK),
        "-u" => meta().is_ok_and(|m| m.permissions().mode() & 0o4000 != 0),
        "-g" => meta().is_ok_and(|m| m.permissions().mode() & 0o2000 != 0),
        "-k" => meta().is_ok_and(|m| m.permissions().mode() & 0o1000 != 0),
        "-O" => meta().is_ok_and(|m| m.uid() == unsafe { libc::geteuid() }),
        "-G" => meta().is_ok_and(|m| m.gid() == unsafe { libc::getegid() }),
        "-t" => x.parse::<i32>().is_ok_and(sys::isatty),
        _ => return Err(format!("{op}: unary operator expected")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pf(fmt: &str, args: &[&str]) -> String {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let mut out = String::new();
        let mut rest = &args[..];
        loop {
            let used = format_printf(fmt, rest, &mut out).unwrap();
            if used == 0 || used >= rest.len() {
                break;
            }
            rest = &rest[used..];
        }
        out
    }

    #[test]
    fn printf_formats() {
        assert_eq!(pf("%s-%d\\n", &["a", "42"]), "a-42\n");
        assert_eq!(pf("%5s|%-5s|", &["ab", "cd"]), "   ab|cd   |");
        assert_eq!(pf("%05d %x %X %o %#x", &["42", "255", "255", "8", "255"]), "00042 ff FF 10 0xff");
        assert_eq!(pf("%.2f %e", &["3.14159", "1500"]), "3.14 1.500000e+03");
        assert_eq!(pf("%s ", &["a", "b", "c"]), "a b c ");
        assert_eq!(pf("%c%%", &["xyz"]), "x%");
        assert_eq!(pf("%b", &["a\\tb"]), "a\tb");
        assert_eq!(pf("%d", &["'A"]), "65");
        assert_eq!(pf("%+d %+d", &["5", "-5"]), "+5 -5");
        assert_eq!(pf("[%*d]", &["4", "7"]), "[   7]");
        assert_eq!(pf("%.3s", &["abcdef"]), "abc");
        assert_eq!(pf("\\101\\x42", &[]), "AB");
    }

    #[test]
    fn test_builtin() {
        let t = |a: &[&str]| test_expr(a).unwrap();
        assert!(t(&["abc"]));
        assert!(!t(&[""]));
        assert!(t(&["-n", "x"]));
        assert!(t(&["-z", ""]));
        assert!(t(&["a", "=", "a"]));
        assert!(t(&["1", "-lt", "2"]));
        assert!(!t(&["!", "1", "-lt", "2"]));
        assert!(t(&["(", "1", "-eq", "1", ")", "-a", "x", "!=", "y"]));
        assert!(t(&["0", "-eq", "1", "-o", "2", "-gt", "1"]));
        assert!(t(&["-d", "/"]));
        assert!(!t(&["-f", "/"]));
        assert!(t(&["!"]));
        assert!(test_expr(&["a", "-eq", "1"]).is_err());
    }

    #[test]
    fn read_splitting() {
        assert_eq!(split_read("  a b   c  ", " \t\n", 2), vec!["a", "b   c"]);
        assert_eq!(split_read("a:b:c", ":", 3), vec!["a", "b", "c"]);
        assert_eq!(split_read("a", " ", 3), vec!["a"]);
    }

    #[test]
    fn echo_escapes() {
        assert_eq!(unescape("a\\nb\\0101\\c rest", true), ("a\nbA".to_string(), true));
    }
}
