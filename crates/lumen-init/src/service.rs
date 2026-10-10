//! Service definitions: `/etc/lumen/services/<name>.service`.
//!
//! ```text
//! description = Seat management daemon
//! exec        = /usr/bin/seatd -g seat
//! user        = root            # default root
//! after       = other services  # must be ready first
//! ready       = path:/run/seatd.sock   # start (default) | path:<file> | notify
//! type        = simple          # simple | oneshot
//! restart     = always          # always | on-failure (default) | never
//! target      = graphical       # graphical | console | rescue, space separated; default: all
//! tty         = /dev/ttyS0      # stdio + controlling terminal (default: /run/log/<name>.log)
//! env         = KEY=value       # may repeat
//! runtime_dir = yes             # create /run/user/<uid> for the user
//! workdir     = /               # default: the user's home
//! ```

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ready {
    /// Ready as soon as the process is spawned.
    Start,
    /// Ready once the file exists (e.g. a listening socket).
    Path(String),
    /// Ready when the service sends `READY=1` to the notify socket.
    Notify,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Restart {
    Always,
    OnFailure,
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Simple,
    /// Runs to completion; ready when it exits with status 0.
    Oneshot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceDef {
    pub name: String,
    pub description: String,
    pub exec: Vec<String>,
    pub user: String,
    pub after: Vec<String>,
    pub ready: Ready,
    pub kind: Kind,
    pub restart: Restart,
    pub targets: Vec<String>,
    pub tty: Option<String>,
    pub env: Vec<(String, String)>,
    pub runtime_dir: bool,
    pub workdir: Option<String>,
}

impl ServiceDef {
    pub fn wanted_by(&self, target: &str) -> bool {
        self.targets.is_empty() || self.targets.iter().any(|t| t == target)
    }
}

/// Split a command line into words with simple shell-like quoting.
pub fn split_words(s: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut have = false;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                have = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => cur.push(c),
                        None => return Err("unterminated single quote".into()),
                    }
                }
            }
            '"' => {
                have = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(c) => cur.push(c),
                            None => return Err("trailing backslash".into()),
                        },
                        Some(c) => cur.push(c),
                        None => return Err("unterminated double quote".into()),
                    }
                }
            }
            '\\' => match chars.next() {
                Some(c) => {
                    cur.push(c);
                    have = true;
                }
                None => return Err("trailing backslash".into()),
            },
            c if c.is_whitespace() => {
                if have {
                    words.push(std::mem::take(&mut cur));
                    have = false;
                }
            }
            c => {
                cur.push(c);
                have = true;
            }
        }
    }
    if have {
        words.push(cur);
    }
    Ok(words)
}

pub fn parse(name: &str, text: &str) -> Result<ServiceDef, String> {
    let mut def = ServiceDef {
        name: name.to_string(),
        description: String::new(),
        exec: Vec::new(),
        user: "root".into(),
        after: Vec::new(),
        ready: Ready::Start,
        kind: Kind::Simple,
        restart: Restart::OnFailure,
        targets: Vec::new(),
        tty: None,
        env: Vec::new(),
        runtime_dir: false,
        workdir: None,
    };
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let at = |m: String| format!("{name}.service line {}: {m}", i + 1);
        let (key, value) = line.split_once('=').ok_or_else(|| at("expected 'key = value'".into()))?;
        let (key, value) = (key.trim(), value.trim());
        match key {
            "description" => def.description = value.to_string(),
            "exec" => def.exec = split_words(value).map_err(at)?,
            "user" => def.user = value.to_string(),
            "after" => def.after = value.split_whitespace().map(String::from).collect(),
            "ready" => {
                def.ready = match value {
                    "start" => Ready::Start,
                    "notify" => Ready::Notify,
                    v => match v.strip_prefix("path:") {
                        Some(p) if p.starts_with('/') => Ready::Path(p.to_string()),
                        _ => return Err(at(format!("bad ready value '{v}'"))),
                    },
                }
            }
            "type" => {
                def.kind = match value {
                    "simple" => Kind::Simple,
                    "oneshot" => Kind::Oneshot,
                    v => return Err(at(format!("bad type '{v}'"))),
                }
            }
            "restart" => {
                def.restart = match value {
                    "always" => Restart::Always,
                    "on-failure" => Restart::OnFailure,
                    "never" => Restart::Never,
                    v => return Err(at(format!("bad restart value '{v}'"))),
                }
            }
            "target" => def.targets = value.split_whitespace().map(String::from).collect(),
            "tty" => def.tty = Some(value.to_string()),
            "env" => {
                let (k, v) = value.split_once('=').ok_or_else(|| at("env needs KEY=value".into()))?;
                def.env.push((k.trim().to_string(), v.trim().to_string()));
            }
            "runtime_dir" => def.runtime_dir = matches!(value, "yes" | "true" | "1"),
            "workdir" => def.workdir = Some(value.to_string()),
            k => return Err(at(format!("unknown key '{k}'"))),
        }
    }
    if def.exec.is_empty() {
        return Err(format!("{name}.service: missing exec"));
    }
    Ok(def)
}

/// Load every `*.service` file in `dir`, sorted by name. Broken files are
/// reported and skipped so one typo cannot keep the system from booting.
pub fn load_dir(dir: &str) -> (Vec<ServiceDef>, Vec<String>) {
    let mut defs = Vec::new();
    let mut errors = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        errors.push(format!("cannot read {dir}"));
        return (defs, errors);
    };
    let mut names: Vec<_> = rd.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    names.sort();
    for path in names {
        let Some(stem) = path.file_name().and_then(|f| f.to_str()).and_then(|f| f.strip_suffix(".service")) else {
            continue;
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => match parse(stem, &text) {
                Ok(d) => defs.push(d),
                Err(e) => errors.push(e),
            },
            Err(e) => errors.push(format!("{}: {e}", path.display())),
        }
    }
    (defs, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_service() {
        let d = parse(
            "lumen",
            "# desktop\ndescription = Desktop\nexec = /usr/bin/lumen --backend \"udev auto\" 'x y'\nuser = lumen\n\
             after = seatd setup\nready = notify\nrestart = always\ntarget = graphical\nenv = A=1\nenv = B = two\n\
             runtime_dir = yes\n",
        )
        .unwrap();
        assert_eq!(d.exec, vec!["/usr/bin/lumen", "--backend", "udev auto", "x y"]);
        assert_eq!(d.user, "lumen");
        assert_eq!(d.after, vec!["seatd", "setup"]);
        assert_eq!(d.ready, Ready::Notify);
        assert_eq!(d.restart, Restart::Always);
        assert!(d.wanted_by("graphical") && !d.wanted_by("console"));
        assert_eq!(d.env, vec![("A".into(), "1".into()), ("B".into(), "two".into())]);
        assert!(d.runtime_dir);
    }

    #[test]
    fn defaults_and_errors() {
        let d = parse("s", "exec=/bin/true\nready = path:/run/x.sock\ntype=oneshot\n").unwrap();
        assert_eq!(d.user, "root");
        assert_eq!(d.restart, Restart::OnFailure);
        assert_eq!(d.kind, Kind::Oneshot);
        assert_eq!(d.ready, Ready::Path("/run/x.sock".into()));
        assert!(d.wanted_by("console"));
        assert!(parse("s", "description=x\n").is_err());
        assert!(parse("s", "exec=/bin/x\nbogus=1\n").unwrap_err().contains("line 2"));
        assert!(parse("s", "exec=/bin/x\nready=path:relative\n").is_err());
        assert!(parse("s", "exec=\"/bin/x\n").is_err());
    }

    #[test]
    fn word_splitting() {
        assert_eq!(split_words(r#"a "b c" d\ e 'f"g'"#).unwrap(), vec!["a", "b c", "d e", "f\"g"]);
        assert_eq!(split_words("  ").unwrap(), Vec::<String>::new());
        assert_eq!(split_words("''").unwrap(), vec![""]);
    }
}
