//! Interactive session: prompt, syntax highlighting and completion.

use crate::editor::Helper;
use crate::exec::{self, Shell};
use crate::sys;

const KEYWORDS: &[&str] = &[
    "if", "then", "else", "elif", "fi", "do", "done", "case", "esac", "while", "until", "for", "in", "{", "}", "!",
];

/// Words after which the next word is in command position.
const COMMAND_PREFIXES: &[&str] = &["if", "then", "else", "elif", "do", "while", "until", "!", "{", "time", "exec", "command", "sudo"];

pub struct Session<'a> {
    pub sh: &'a mut Shell,
}

impl Session<'_> {
    fn command_exists(&mut self, word: &str) -> bool {
        KEYWORDS.contains(&word)
            || exec::is_builtin(word)
            || self.sh.functions.contains_key(word)
            || self.sh.aliases.contains_key(word)
            || (word.contains('/') && std::fs::metadata(word).is_ok_and(|m| m.is_file()) && sys::access_x(word))
            || (!word.contains('/') && self.sh.find_in_path(word).is_some())
    }
}

/// Colors (ANSI 16-color, readable on any palette).
mod color {
    pub const COMMAND: &str = "\x1b[1;32m";
    pub const BAD_COMMAND: &str = "\x1b[1;31m";
    pub const KEYWORD: &str = "\x1b[1;35m";
    pub const STRING: &str = "\x1b[33m";
    pub const VARIABLE: &str = "\x1b[36m";
    pub const OPERATOR: &str = "\x1b[35m";
    pub const COMMENT: &str = "\x1b[90m";
    pub const OPTION: &str = "\x1b[34m";
    pub const PATH: &str = "\x1b[4m";
    pub const RESET: &str = "\x1b[0m";
}

impl Helper for Session<'_> {
    fn highlight(&mut self, line: &str) -> String {
        let chars: Vec<char> = line.chars().collect();
        let mut out = String::new();
        let mut i = 0;
        let mut command_pos = true;
        while i < chars.len() {
            let c = chars[i];
            if c == ' ' || c == '\t' || c == '\n' {
                out.push(c);
                i += 1;
                if c == '\n' {
                    command_pos = true;
                }
                continue;
            }
            if c == '#' && (i == 0 || chars[i - 1].is_whitespace()) {
                let rest: String = chars[i..].iter().collect();
                out.push_str(color::COMMENT);
                out.push_str(&rest);
                out.push_str(color::RESET);
                break;
            }
            if "|&;()<>".contains(c) {
                let start = i;
                while i < chars.len() && "|&;()<>".contains(chars[i]) {
                    i += 1;
                }
                let op: String = chars[start..i].iter().collect();
                out.push_str(color::OPERATOR);
                out.push_str(&op);
                out.push_str(color::RESET);
                if !op.contains('<') && !op.contains('>') {
                    command_pos = true;
                }
                continue;
            }
            // A word: scan with quote awareness and build its colored form.
            let start = i;
            let mut plain = String::new();
            let mut colored = String::new();
            while i < chars.len() {
                let c = chars[i];
                if c.is_whitespace() || "|&;()<>".contains(c) {
                    break;
                }
                match c {
                    '\'' => {
                        let s = i;
                        i += 1;
                        while i < chars.len() && chars[i] != '\'' {
                            i += 1;
                        }
                        i = (i + 1).min(chars.len());
                        let text: String = chars[s..i].iter().collect();
                        plain.push_str(&text);
                        colored.push_str(&format!("{}{text}{}", color::STRING, color::RESET));
                    }
                    '"' => {
                        let s = i;
                        i += 1;
                        while i < chars.len() && chars[i] != '"' {
                            if chars[i] == '\\' {
                                i += 1;
                            }
                            i += 1;
                        }
                        i = (i + 1).min(chars.len());
                        let text: String = chars[s..i].iter().collect();
                        plain.push_str(&text);
                        colored.push_str(&format!("{}{text}{}", color::STRING, color::RESET));
                    }
                    '$' => {
                        let s = i;
                        i += 1;
                        if i < chars.len() && chars[i] == '{' {
                            while i < chars.len() && chars[i] != '}' {
                                i += 1;
                            }
                            i = (i + 1).min(chars.len());
                        } else if i < chars.len() && chars[i] == '(' {
                            let mut depth = 0;
                            while i < chars.len() {
                                if chars[i] == '(' {
                                    depth += 1;
                                } else if chars[i] == ')' {
                                    depth -= 1;
                                    if depth == 0 {
                                        i += 1;
                                        break;
                                    }
                                }
                                i += 1;
                            }
                        } else {
                            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_' || "?$!#@*-".contains(chars[i]) && i == s + 1) {
                                i += 1;
                                if i == s + 2 && "?$!#@*-".contains(chars[i - 1]) {
                                    break;
                                }
                            }
                        }
                        let text: String = chars[s..i].iter().collect();
                        plain.push_str(&text);
                        colored.push_str(&format!("{}{text}{}", color::VARIABLE, color::RESET));
                    }
                    '\\' => {
                        let s = i;
                        i = (i + 2).min(chars.len());
                        let text: String = chars[s..i].iter().collect();
                        plain.push_str(&text);
                        colored.push_str(&text);
                    }
                    c => {
                        plain.push(c);
                        colored.push(c);
                        i += 1;
                    }
                }
            }
            let is_assignment = command_pos
                && plain.find('=').is_some_and(|eq| crate::parser::is_name(&plain[..eq]));
            if command_pos && !is_assignment {
                let word_text: String = chars[start..i].iter().collect();
                let (col, next_cmd) = if KEYWORDS.contains(&plain.as_str()) {
                    (color::KEYWORD, COMMAND_PREFIXES.contains(&plain.as_str()))
                } else if self.command_exists(&plain) {
                    (color::COMMAND, COMMAND_PREFIXES.contains(&plain.as_str()))
                } else {
                    (color::BAD_COMMAND, false)
                };
                out.push_str(col);
                out.push_str(&word_text);
                out.push_str(color::RESET);
                command_pos = next_cmd;
            } else if is_assignment {
                out.push_str(&colored);
            } else if plain.starts_with('-') {
                out.push_str(color::OPTION);
                out.push_str(&colored);
                out.push_str(color::RESET);
            } else if colored == plain && plain.len() > 1 && (plain.contains('/') || plain.starts_with('~')) && path_exists(&plain, self.sh) {
                out.push_str(color::PATH);
                out.push_str(&plain);
                out.push_str(color::RESET);
            } else {
                out.push_str(&colored);
            }
        }
        out
    }

    fn complete(&mut self, line: &str, pos: usize) -> (usize, Vec<String>) {
        let before = &line[..pos];
        // Start of the current word.
        let start = before
            .char_indices()
            .rev()
            .find(|(_, c)| c.is_whitespace() || "|&;()<>".contains(*c))
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        let word = &before[start..];
        // Command position: only blanks/operators/keywords before the word on this command.
        let prefix = before[..start].trim_end();
        let command_pos = prefix.is_empty()
            || prefix.ends_with(['|', '&', ';', '(', '\n'])
            || COMMAND_PREFIXES.iter().any(|k| prefix == *k || prefix.ends_with(&format!(" {k}")));
        if let Some(var) = word.strip_prefix('$') {
            let mut c: Vec<String> = self.sh.vars.keys().filter(|k| k.starts_with(var)).map(|k| format!("${k}")).collect();
            c.sort();
            return (start, c);
        }
        if command_pos && !word.contains('/') {
            let mut c: Vec<String> = Vec::new();
            for b in exec::SPECIAL_BUILTINS.iter().chain(exec::BUILTINS.iter()).chain(KEYWORDS.iter()) {
                if b.starts_with(word) {
                    c.push(b.to_string());
                }
            }
            c.extend(self.sh.functions.keys().filter(|f| f.starts_with(word)).cloned());
            c.extend(self.sh.aliases.keys().filter(|f| f.starts_with(word)).cloned());
            let path = self.sh.get("PATH").unwrap_or_default();
            for dir in path.split(':') {
                if let Ok(rd) = std::fs::read_dir(if dir.is_empty() { "." } else { dir }) {
                    for e in rd.flatten() {
                        let name = e.file_name().to_string_lossy().into_owned();
                        if name.starts_with(word) && sys::access_x(&e.path().to_string_lossy()) {
                            c.push(name);
                        }
                    }
                }
            }
            if !word.is_empty() || c.len() < 2000 {
                return (start, c);
            }
        }
        (start, complete_path(word, self.sh))
    }
}

fn expand_tilde(word: &str, sh: &Shell) -> (String, String) {
    // Returns (expanded, the prefix that replaced "~").
    if let Some(rest) = word.strip_prefix('~') {
        if rest.is_empty() || rest.starts_with('/') {
            let home = sh.get("HOME").unwrap_or_default();
            return (format!("{home}{rest}"), home);
        }
    }
    (word.to_string(), String::new())
}

fn path_exists(word: &str, sh: &Shell) -> bool {
    let (p, _) = expand_tilde(word, sh);
    std::fs::symlink_metadata(p).is_ok()
}

fn complete_path(word: &str, sh: &Shell) -> Vec<String> {
    let (expanded, home) = expand_tilde(word, sh);
    let (dir, file_prefix) = match expanded.rfind('/') {
        Some(i) => (&expanded[..=i], &expanded[i + 1..]),
        None => ("", expanded.as_str()),
    };
    let Ok(rd) = std::fs::read_dir(if dir.is_empty() { "." } else { dir }) else { return Vec::new() };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !name.starts_with(file_prefix) || (name.starts_with('.') && !file_prefix.starts_with('.')) {
            continue;
        }
        let mut full = format!("{dir}{name}");
        if std::fs::metadata(&full).is_ok_and(|m| m.is_dir()) {
            full.push('/');
        }
        // Keep the user's "~" spelling.
        if !home.is_empty() && full.starts_with(&home) {
            full = format!("~{}", &full[home.len()..]);
        }
        out.push(escape_completion(&full));
    }
    out
}

fn escape_completion(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if " \t'\"\\$`|&;()<>*?[]!#".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Expand prompt escapes (bash-compatible subset). Non-printing sequences are
/// wrapped in \x01..\x02 so the editor can measure the prompt width.
pub fn render_prompt(sh: &mut Shell, ps: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = ps.chars().collect();
    let mut i = 0;
    let user = sh.get("USER").unwrap_or_else(|| {
        // SAFETY: getuid has no side effects.
        if unsafe { libc::getuid() } == 0 { "root".into() } else { "user".into() }
    });
    while i < chars.len() {
        let c = chars[i];
        if c != '\\' || i + 1 >= chars.len() {
            out.push(c);
            i += 1;
            continue;
        }
        i += 1;
        match chars[i] {
            'u' => out.push_str(&user),
            'h' | 'H' => {
                let host = std::fs::read_to_string("/proc/sys/kernel/hostname").unwrap_or_default();
                let host = host.trim();
                out.push_str(if chars[i] == 'h' { host.split('.').next().unwrap_or(host) } else { host });
            }
            'w' | 'W' => {
                let pwd = sh.get("PWD").unwrap_or_default();
                let home = sh.get("HOME").unwrap_or_default();
                let shown = if !home.is_empty() && (pwd == home || pwd.starts_with(&format!("{home}/"))) {
                    format!("~{}", &pwd[home.len()..])
                } else {
                    pwd.clone()
                };
                if chars[i] == 'W' {
                    out.push_str(if shown == "/" || shown == "~" { &shown } else { shown.rsplit('/').next().unwrap_or(&shown) });
                } else {
                    out.push_str(&shown);
                }
            }
            '$' => out.push(if unsafe { libc::geteuid() } == 0 { '#' } else { '$' }),
            'n' => out.push('\n'),
            'e' => out.push('\u{1b}'),
            'a' => out.push('\u{7}'),
            '[' => out.push('\u{1}'),
            ']' => out.push('\u{2}'),
            '\\' => out.push('\\'),
            'j' => out.push_str(&sh.jobs.jobs.len().to_string()),
            '?' => out.push_str(&sh.status.to_string()),
            't' | 'T' => {
                // SAFETY: time/localtime_r with valid pointers.
                let (h, m, s) = unsafe {
                    let t = libc::time(std::ptr::null_mut());
                    let mut tm: libc::tm = std::mem::zeroed();
                    libc::localtime_r(&t, &mut tm);
                    (tm.tm_hour, tm.tm_min, tm.tm_sec)
                };
                if chars[i] == 't' {
                    out.push_str(&format!("{h:02}:{m:02}:{s:02}"));
                } else {
                    out.push_str(&format!("{:02}:{m:02}:{s:02}", if h % 12 == 0 { 12 } else { h % 12 }));
                }
            }
            c => {
                out.push('\\');
                out.push(c);
            }
        }
        i += 1;
    }
    if out.contains('$') {
        // Parameter expansion as in a double-quoted string.
        let quoted = format!("\"{}\"", out.replace('\\', "\\\\").replace('"', "\\\""));
        if let Ok(prog) = crate::parser::parse(&format!(": {quoted}")) {
            if let Some(crate::ast::Command::Simple(s)) = prog.0.first().map(|i| &i.and_or.first.commands[0]) {
                if let Some(w) = s.words.get(1) {
                    let status = sh.status;
                    if let Ok(v) = crate::expand::expand_string(sh, w) {
                        out = v;
                    }
                    sh.status = status;
                }
            }
        }
    }
    out
}

/// The default prompt: directory, then a symbol colored by the last status.
pub fn default_prompt(sh: &Shell) -> String {
    let utf8 = ["LC_ALL", "LC_CTYPE", "LANG"].iter().any(|v| sh.get(v).is_some_and(|l| l.to_ascii_uppercase().contains("UTF")));
    // SAFETY: geteuid has no side effects.
    let root = unsafe { libc::geteuid() } == 0;
    let symbol = if root { "#" } else if utf8 { "❯" } else { "$" };
    let symbol_color = if sh.status == 0 { "\\[\\e[1;35m\\]" } else { "\\[\\e[1;31m\\]" };
    let host = if root { "\\[\\e[1;31m\\]\\h\\[\\e[0m\\] " } else { "" };
    format!("{host}\\[\\e[1;34m\\]\\w\\[\\e[0m\\] {symbol_color}{symbol}\\[\\e[0m\\] ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip(s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\u{1b}' && chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    #[test]
    fn highlighting_keeps_text() {
        let mut sh = Shell::new("lsh", vec![]);
        sh.set("PATH", "/bin:/usr/bin");
        let mut s = Session { sh: &mut sh };
        for line in [
            "echo 'a b' \"c $d\" ${e:-f} $(g h) # comment",
            "if true; then ls -la /; fi | grep x && nosuchcmd >out 2>&1",
            "A=1 B=\"x\" env",
            "for i in 1 2; do echo $i; done",
            "echo \\$ unterminated 'quote",
            "日本語 ç",
        ] {
            assert_eq!(strip(&s.highlight(line)), line);
        }
        let h = s.highlight("echo hi");
        assert!(h.starts_with(color::COMMAND), "{h:?}");
        let h = s.highlight("definitely-not-a-command");
        assert!(h.starts_with(color::BAD_COMMAND), "{h:?}");
    }

    #[test]
    fn prompt_escapes() {
        let mut sh = Shell::new("lsh", vec![]);
        sh.set("HOME", "/home/u");
        sh.set("PWD", "/home/u/src/proj");
        sh.set("USER", "u");
        assert_eq!(render_prompt(&mut sh, "\\u:\\w"), "u:~/src/proj");
        assert_eq!(render_prompt(&mut sh, "\\W"), "proj");
        assert_eq!(render_prompt(&mut sh, "\\[\\e[1m\\]x\\[\\e[0m\\]"), "\u{1}\u{1b}[1m\u{2}x\u{1}\u{1b}[0m\u{2}");
        sh.status = 3;
        assert_eq!(render_prompt(&mut sh, "[\\?] $USER"), "[3] u");
        let ps1 = default_prompt(&sh);
        let rendered = render_prompt(&mut sh, &ps1);
        let visible: String = strip(&rendered).replace(['\u{1}', '\u{2}'], "");
        assert!(visible.contains("~/src/proj ") && visible.ends_with(' '), "{visible:?}");
        assert_eq!(crate::editor::display_width(&rendered), visible.chars().count());
    }

    #[test]
    fn completion() {
        let mut sh = Shell::new("lsh", vec![]);
        sh.set("PATH", "/nonexistent");
        sh.functions.insert("my_func".into(), std::rc::Rc::new(crate::ast::Command::Group(Default::default(), vec![])));
        let mut s = Session { sh: &mut sh };
        let (start, c) = s.complete("ech", 3);
        assert_eq!((start, c), (0, vec!["echo".to_string()]));
        let (_, c) = s.complete("true | my_", 10);
        assert_eq!(c, vec!["my_func".to_string()]);
        let (start, c) = s.complete("ls /pro", 7);
        assert_eq!(start, 3);
        assert!(c.contains(&"/proc/".to_string()), "{c:?}");
        s.sh.set("MY_VARIABLE", "1");
        let (_, c) = s.complete("echo $MY_V", 10);
        assert_eq!(c, vec!["$MY_VARIABLE".to_string()]);
    }
}
