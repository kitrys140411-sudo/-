//! Configuration format of the Lumen boot loader.
//!
//! The file lives on the ESP at `\loader\lumen.conf` and is a small INI dialect:
//!
//! ```text
//! # comment
//! timeout = 5            # seconds; 0 boots immediately, -1 waits forever
//! default = lumen        # id of the default entry
//!
//! [entry lumen]
//! title   = Lumen
//! kernel  = \lumen\vmlinuz.efi
//! initrd  = \lumen\initramfs.img
//! cmdline = root=PARTUUID=... quiet
//!
//! [entry firmware]
//! title  = Firmware settings
//! action = firmware      # firmware | reboot | poweroff
//! ```
//!
//! Values run to the end of the line; a `#` starts a comment only at the beginning
//! of a line or after whitespace, so kernel command lines may contain `#` inside
//! words. Values may be wrapped in double quotes to keep leading/trailing spaces.
#![no_std]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

/// What selecting an entry does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Start a Linux kernel through its EFI stub.
    Linux {
        kernel: String,
        initrd: Option<String>,
        cmdline: String,
    },
    /// Reboot into the firmware setup UI.
    Firmware,
    Reboot,
    Poweroff,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub title: String,
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Auto-boot delay in seconds; `None` waits for the user.
    pub timeout: Option<u32>,
    /// Index into `entries` of the default entry.
    pub default: usize,
    pub entries: Vec<Entry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub line: usize,
    pub message: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line == 0 {
            write!(f, "{}", self.message)
        } else {
            write!(f, "line {}: {}", self.line, self.message)
        }
    }
}

fn err(line: usize, message: impl ToString) -> Error {
    Error {
        line,
        message: message.to_string(),
    }
}

/// Strip a trailing comment (`#` at line start or preceded by whitespace).
fn strip_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut in_quotes = false;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'"' => in_quotes = !in_quotes,
            b'#' if !in_quotes && (i == 0 || bytes[i - 1].is_ascii_whitespace()) => {
                return &line[..i];
            }
            _ => {}
        }
    }
    line
}

fn unquote(value: &str, line: usize) -> Result<String, Error> {
    let v = value.trim();
    if let Some(rest) = v.strip_prefix('"') {
        match rest.strip_suffix('"') {
            Some(inner) => Ok(inner.to_string()),
            None => Err(err(line, "unterminated quoted value")),
        }
    } else {
        Ok(v.to_string())
    }
}

#[derive(Default)]
struct PendingEntry {
    id: String,
    line: usize,
    title: Option<String>,
    kernel: Option<String>,
    initrd: Option<String>,
    cmdline: Option<String>,
    action: Option<String>,
}

impl PendingEntry {
    fn finish(self) -> Result<Entry, Error> {
        let title = self.title.unwrap_or_else(|| self.id.clone());
        let action = match self.action.as_deref() {
            None | Some("linux") => {
                let kernel = self
                    .kernel
                    .ok_or_else(|| err(self.line, alloc::format!("entry '{}' has no kernel", self.id)))?;
                Action::Linux {
                    kernel,
                    initrd: self.initrd,
                    cmdline: self.cmdline.unwrap_or_default(),
                }
            }
            Some("firmware") => Action::Firmware,
            Some("reboot") => Action::Reboot,
            Some("poweroff") => Action::Poweroff,
            Some(other) => {
                return Err(err(self.line, alloc::format!("unknown action '{other}'")));
            }
        };
        Ok(Entry {
            id: self.id,
            title,
            action,
        })
    }
}

/// Parse the configuration text.
pub fn parse(text: &str) -> Result<Config, Error> {
    let mut timeout: Option<u32> = Some(5);
    let mut default_id: Option<String> = None;
    let mut entries: Vec<Entry> = Vec::new();
    let mut current: Option<PendingEntry> = None;

    for (idx, raw) in text.lines().enumerate() {
        let lineno = idx + 1;
        let line = strip_comment(raw.trim_start_matches('\u{feff}')).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(header) = line.strip_prefix('[') {
            let header = header
                .strip_suffix(']')
                .ok_or_else(|| err(lineno, "missing ']' in section header"))?
                .trim();
            let mut parts = header.split_whitespace();
            match (parts.next(), parts.next(), parts.next()) {
                (Some("entry"), Some(id), None) => {
                    if let Some(prev) = current.take() {
                        entries.push(prev.finish()?);
                    }
                    if entries.iter().any(|e| e.id == id) {
                        return Err(err(lineno, alloc::format!("duplicate entry '{id}'")));
                    }
                    current = Some(PendingEntry {
                        id: id.to_string(),
                        line: lineno,
                        ..Default::default()
                    });
                }
                _ => return Err(err(lineno, "expected '[entry <id>]'")),
            }
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| err(lineno, "expected 'key = value'"))?;
        let key = key.trim();
        let value = unquote(value, lineno)?;
        match current.as_mut() {
            None => match key {
                "timeout" => {
                    let t: i64 = value
                        .parse()
                        .map_err(|_| err(lineno, "timeout must be an integer"))?;
                    timeout = if t < 0 { None } else { Some(t.min(3600) as u32) };
                }
                "default" => default_id = Some(value),
                _ => return Err(err(lineno, alloc::format!("unknown global key '{key}'"))),
            },
            Some(entry) => {
                let slot = match key {
                    "title" => &mut entry.title,
                    "kernel" => &mut entry.kernel,
                    "initrd" => &mut entry.initrd,
                    "cmdline" | "options" => &mut entry.cmdline,
                    "action" => &mut entry.action,
                    _ => return Err(err(lineno, alloc::format!("unknown entry key '{key}'"))),
                };
                *slot = Some(value);
            }
        }
    }
    if let Some(prev) = current.take() {
        entries.push(prev.finish()?);
    }
    if entries.is_empty() {
        return Err(err(0, "no boot entries defined"));
    }
    let default = match default_id {
        None => 0,
        Some(id) => entries
            .iter()
            .position(|e| e.id == id)
            .ok_or_else(|| err(0, alloc::format!("default entry '{id}' does not exist")))?,
    };
    Ok(Config {
        timeout,
        default,
        entries,
    })
}

/// Normalize an ESP path: forward slashes become backslashes, and a leading
/// backslash is ensured (UEFI file paths are absolute from the volume root).
pub fn esp_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len() + 1);
    if !path.starts_with(['\\', '/']) {
        out.push('\\');
    }
    for c in path.chars() {
        out.push(if c == '/' { '\\' } else { c });
    }
    out
}

/// Encode a kernel command line as NUL-terminated UCS-2, as expected in
/// `EFI_LOADED_IMAGE_PROTOCOL.LoadOptions` by the Linux EFI stub. Characters
/// outside the BMP are replaced by '?'.
pub fn cmdline_ucs2(cmdline: &str) -> Vec<u16> {
    let mut out: Vec<u16> = cmdline
        .chars()
        .map(|c| if (c as u32) < 0x10000 { c as u16 } else { b'?' as u16 })
        .collect();
    out.push(0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
# Lumen boot configuration
timeout = 3
default = lumen

[entry lumen]
title   = Lumen
kernel  = /lumen/vmlinuz.efi
initrd  = \lumen\initramfs.img
cmdline = root=PARTUUID=abc quiet # trailing comment

[entry rescue]
title   = "Lumen (rescue shell)"
kernel  = \lumen\vmlinuz.efi
cmdline = "root=PARTUUID=abc lumen.target=rescue"

[entry fw]
title = Firmware settings
action = firmware
"#;

    #[test]
    fn parses_sample() {
        let c = parse(SAMPLE).unwrap();
        assert_eq!(c.timeout, Some(3));
        assert_eq!(c.default, 0);
        assert_eq!(c.entries.len(), 3);
        assert_eq!(c.entries[0].title, "Lumen");
        assert_eq!(
            c.entries[0].action,
            Action::Linux {
                kernel: "/lumen/vmlinuz.efi".into(),
                initrd: Some("\\lumen\\initramfs.img".into()),
                cmdline: "root=PARTUUID=abc quiet".into(),
            }
        );
        assert_eq!(c.entries[1].title, "Lumen (rescue shell)");
        assert_eq!(c.entries[2].action, Action::Firmware);
    }

    #[test]
    fn hash_inside_word_is_kept() {
        let c = parse("[entry a]\nkernel=k\ncmdline=a#b c # d\n").unwrap();
        match &c.entries[0].action {
            Action::Linux { cmdline, .. } => assert_eq!(cmdline, "a#b c"),
            _ => panic!(),
        }
    }

    #[test]
    fn default_by_id_and_negative_timeout() {
        let c = parse("timeout=-1\ndefault=b\n[entry a]\nkernel=k\n[entry b]\nkernel=k\n").unwrap();
        assert_eq!(c.timeout, None);
        assert_eq!(c.default, 1);
    }

    #[test]
    fn errors_have_line_numbers() {
        let e = parse("timeout = soon\n").unwrap_err();
        assert_eq!(e.line, 1);
        let e = parse("[entry a]\nkernel=k\nbogus=1\n").unwrap_err();
        assert_eq!(e.line, 3);
        let e = parse("[entry a]\ntitle=x\n").unwrap_err();
        assert!(e.message.contains("no kernel"), "{e}");
        let e = parse("default=zzz\n[entry a]\nkernel=k\n").unwrap_err();
        assert!(e.message.contains("zzz"));
        assert!(parse("").is_err());
        assert!(parse("[entry a]\nkernel=k\n[entry a]\nkernel=k\n").is_err());
        assert!(parse("[entry a\n").is_err());
        assert!(parse("[entry a]\naction=dance\n").is_err());
        assert!(parse("[entry a]\ntitle=\"x\n").is_err());
    }

    #[test]
    fn bom_and_crlf() {
        let c = parse("\u{feff}timeout=1\r\n[entry a]\r\nkernel=k\r\n").unwrap();
        assert_eq!(c.timeout, Some(1));
    }

    #[test]
    fn paths_and_cmdline() {
        assert_eq!(esp_path("/lumen/vmlinuz.efi"), "\\lumen\\vmlinuz.efi");
        assert_eq!(esp_path("lumen/x"), "\\lumen\\x");
        assert_eq!(esp_path("\\a"), "\\a");
        assert_eq!(cmdline_ucs2("ab"), [b'a' as u16, b'b' as u16, 0]);
        assert_eq!(cmdline_ucs2("😀"), [b'?' as u16, 0]);
    }
}
