//! File system helpers shared by the file chooser and file-handling apps:
//! directory listings, natural name order, icons by file type, glob
//! filters, human-readable sizes and dates, and the user's standard folders.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::icons::Icon;

/// One entry of a directory listing.
#[derive(Clone, Debug, PartialEq)]
pub struct DirEntry {
    pub name: String,
    pub path: PathBuf,
    /// A directory, or a symbolic link to one.
    pub is_dir: bool,
    pub is_symlink: bool,
    /// Size in bytes (0 for directories).
    pub size: u64,
    pub modified: Option<SystemTime>,
}

impl DirEntry {
    /// Dot files.
    pub fn is_hidden(&self) -> bool {
        self.name.starts_with('.')
    }

    /// The symbolic icon for the entry's type.
    pub fn icon(&self) -> Icon {
        icon_for(&self.name, self.is_dir)
    }
}

/// Every entry of `dir` (hidden ones included), unsorted. Entries whose
/// metadata cannot be read (vanished, broken links) are still listed.
pub fn read_dir(dir: &Path) -> std::io::Result<Vec<DirEntry>> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir)? {
        let Ok(e) = e else { continue };
        let path = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        let link = e.file_type().map(|t| t.is_symlink()).unwrap_or(false);
        // Links count as what they point to; broken ones as files.
        let (is_dir, size, modified) = match std::fs::metadata(&path) {
            Ok(m) => (m.is_dir(), if m.is_dir() { 0 } else { m.len() }, m.modified().ok()),
            Err(_) => (false, 0, std::fs::symlink_metadata(&path).ok().and_then(|m| m.modified().ok())),
        };
        out.push(DirEntry { name, path, is_dir, is_symlink: link, size, modified });
    }
    Ok(out)
}

/// "Natural" order of names, as people expect: case-insensitive, and runs of
/// digits compared by value ("file2" before "file10").
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut s = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        s.push(c);
                        it.next();
                    }
                    s
                };
                let (m, n) = (take(&mut x), take(&mut y));
                let (mt, nt) = (m.trim_start_matches('0'), n.trim_start_matches('0'));
                let o = mt.len().cmp(&nt.len()).then_with(|| mt.cmp(nt));
                if o != Ordering::Equal {
                    return o;
                }
            }
            (Some(c), Some(d)) => {
                let o = c.to_lowercase().cmp(d.to_lowercase());
                if o != Ordering::Equal {
                    return o;
                }
                x.next();
                y.next();
            }
        }
    }
}

/// The extension of `name` in lower case ("" when there is none; dot files
/// like `.bashrc` have none).
pub fn extension(name: &str) -> String {
    match name.rfind('.') {
        Some(i) if i > 0 => name[i + 1..].to_ascii_lowercase(),
        _ => String::new(),
    }
}

/// The symbolic icon for a file name.
pub fn icon_for(name: &str, is_dir: bool) -> Icon {
    if is_dir {
        return Icon::Folder;
    }
    match extension(name).as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" | "tif" | "tiff" | "ico" | "avif" | "heic" => Icon::Image,
        "mp4" | "mkv" | "webm" | "avi" | "mov" | "m4v" | "ogv" => Icon::Video,
        "mp3" | "flac" | "ogg" | "opus" | "wav" | "m4a" | "aac" => Icon::Audio,
        "zip" | "tar" | "gz" | "tgz" | "xz" | "zst" | "bz2" | "7z" | "rar" | "apk" | "deb" | "rpm" | "flatpak" => Icon::Archive,
        "rs" | "c" | "h" | "cc" | "cpp" | "hpp" | "py" | "js" | "ts" | "go" | "java" | "sh" | "toml" | "json" | "yaml" | "yml" | "html" | "css"
        | "xml" | "lua" | "rb" | "zig" | "mk" => Icon::FileCode,
        "txt" | "md" | "log" | "csv" | "conf" | "ini" | "cfg" | "desktop" | "pdf" | "doc" | "docx" | "odt" | "rtf" => Icon::FileText,
        _ => Icon::File,
    }
}

/// Shell-style pattern match of a whole file name: `*` any run, `?` one
/// character, case-insensitive (`*.png` matches `Photo.PNG`).
pub fn glob_match(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.chars().flat_map(char::to_lowercase).collect();
    let n: Vec<char> = name.chars().flat_map(char::to_lowercase).collect();
    // Iterative matcher with backtracking to the last star.
    let (mut i, mut j, mut star, mut mark) = (0, 0, None, 0);
    while j < n.len() {
        if i < p.len() && (p[i] == '?' || p[i] == n[j]) {
            i += 1;
            j += 1;
        } else if i < p.len() && p[i] == '*' {
            star = Some(i);
            mark = j;
            i += 1;
        } else if let Some(s) = star {
            i = s + 1;
            mark += 1;
            j = mark;
        } else {
            return false;
        }
    }
    p[i..].iter().all(|&c| c == '*')
}

/// "512 bytes", "4.2 kB", "1.3 MB" (decimal units, like the desktop's).
pub fn format_size(bytes: u64) -> String {
    if bytes == 1 {
        return "1 byte".into();
    }
    if bytes < 1000 {
        return format!("{bytes} bytes");
    }
    let units = ["kB", "MB", "GB", "TB", "PB"];
    let mut v = bytes as f64 / 1000.0;
    let mut u = 0;
    while v >= 999.95 && u + 1 < units.len() {
        v /= 1000.0;
        u += 1;
    }
    if v < 10.0 { format!("{v:.1} {}", units[u]) } else { format!("{v:.0} {}", units[u]) }
}

/// Local calendar time of `t`: (year, month 1–12, day, hour, minute,
/// day of the year).
fn local(t: SystemTime) -> Option<(i32, u32, u32, u32, u32, i32)> {
    let secs = match t.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    };
    // SAFETY: localtime_r writes the broken-down time into `tm`.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&(secs as _), &mut tm).is_null() {
            return None;
        }
        Some((tm.tm_year + 1900, tm.tm_mon as u32 + 1, tm.tm_mday as u32, tm.tm_hour as u32, tm.tm_min as u32, tm.tm_yday))
    }
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// A modification time the way file managers show it, relative to `now`:
/// "14:05" today, "Yesterday", "12 Oct" this year, "12 Oct 2024" before.
pub fn format_time(t: SystemTime, now: SystemTime) -> String {
    match (local(t), local(now)) {
        (Some(a), Some(b)) => describe_time(a, b),
        _ => String::new(),
    }
}

fn describe_time(a: (i32, u32, u32, u32, u32, i32), b: (i32, u32, u32, u32, u32, i32)) -> String {
    let (y, mo, d, h, mi, yday) = a;
    let (ny, _, _, _, _, nyday) = b;
    if y == ny && yday == nyday {
        format!("{h:02}:{mi:02}")
    } else if (y == ny && yday + 1 == nyday) || (y + 1 == ny && nyday == 0 && mo == 12 && d == 31) {
        "Yesterday".into()
    } else if y == ny {
        format!("{d} {}", MONTHS[(mo - 1) as usize])
    } else {
        format!("{d} {} {y}", MONTHS[(mo - 1) as usize])
    }
}

/// A standard folder for a sidebar.
#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub name: String,
    pub icon: Icon,
    pub path: PathBuf,
}

/// The user's home directory.
pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

/// Home, the XDG user folders that exist (Desktop, Documents, Downloads,
/// Music, Pictures, Videos; from `~/.config/user-dirs.dirs`, defaulting to
/// the English names) and the root of the file system.
pub fn places() -> Vec<Place> {
    let home = home();
    let config = std::fs::read_to_string(home.join(".config/user-dirs.dirs")).unwrap_or_default();
    let lookup = |key: &str, default: &str| -> PathBuf {
        for line in config.lines() {
            if let Some(v) = line.trim().strip_prefix(key).and_then(|r| r.strip_prefix('=')) {
                let v = v.trim().trim_matches('"');
                return match v.strip_prefix("$HOME") {
                    Some(rest) => home.join(rest.trim_start_matches('/')),
                    None => PathBuf::from(v),
                };
            }
        }
        home.join(default)
    };
    let mut out = vec![Place { name: "Home".into(), icon: Icon::Home, path: home.clone() }];
    for (key, name, icon) in [
        ("XDG_DESKTOP_DIR", "Desktop", Icon::Display),
        ("XDG_DOCUMENTS_DIR", "Documents", Icon::FileText),
        ("XDG_DOWNLOAD_DIR", "Downloads", Icon::Download),
        ("XDG_MUSIC_DIR", "Music", Icon::Audio),
        ("XDG_PICTURES_DIR", "Pictures", Icon::Image),
        ("XDG_VIDEOS_DIR", "Videos", Icon::Video),
    ] {
        let path = lookup(key, name);
        // A user dir set to $HOME itself means "unused".
        if path != home && path.is_dir() {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| name.to_string());
            out.push(Place { name, icon, path });
        }
    }
    out.push(Place { name: "Computer".into(), icon: Icon::HardDrive, path: PathBuf::from("/") });
    out
}

/// How a path is shown to people: `~` for the home directory.
pub fn display_path(path: &Path) -> String {
    let home = home();
    match path.strip_prefix(&home) {
        Ok(rest) if home != Path::new("/") => {
            if rest.as_os_str().is_empty() {
                "~".into()
            } else {
                format!("~/{}", rest.display())
            }
        }
        _ => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order() {
        let mut v = vec!["file10", "File2", "file1", "a", "B", "file02b", "file2a"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, vec!["a", "B", "file1", "File2", "file2a", "file02b", "file10"]);
        assert_eq!(natural_cmp("x", "x"), Ordering::Equal);
    }

    #[test]
    fn globs() {
        assert!(glob_match("*.png", "Photo.PNG"));
        assert!(glob_match("*", ".hidden"));
        assert!(glob_match("img_??.jpg", "img_01.jpg"));
        assert!(!glob_match("img_??.jpg", "img_1.jpg"));
        assert!(glob_match("*.tar.*", "a.tar.gz"));
        assert!(!glob_match("*.png", "png"));
        assert!(glob_match("a*b*c", "aXXbYYc"));
        assert!(!glob_match("a*b*c", "aXXbYY"));
    }

    #[test]
    fn sizes_and_icons() {
        assert_eq!(format_size(0), "0 bytes");
        assert_eq!(format_size(999), "999 bytes");
        assert_eq!(format_size(4200), "4.2 kB");
        assert_eq!(format_size(1_300_000), "1.3 MB");
        assert_eq!(format_size(999_999), "1.0 MB");
        assert_eq!(format_size(25_000_000_000), "25 GB");
        assert_eq!(icon_for("a.JPG", false), Icon::Image);
        assert_eq!(icon_for("src", true), Icon::Folder);
        assert_eq!(icon_for(".bashrc", false), Icon::File);
        assert_eq!(extension("archive.tar.gz"), "gz");
    }

    #[test]
    fn relative_dates() {
        let now = (2025, 10, 14, 12, 0, 286);
        assert_eq!(describe_time((2025, 10, 14, 11, 5, 286), now), "11:05");
        assert_eq!(describe_time((2025, 10, 13, 23, 59, 285), now), "Yesterday");
        assert_eq!(describe_time((2025, 10, 4, 9, 0, 276), now), "4 Oct");
        assert_eq!(describe_time((2024, 9, 9, 9, 0, 252), now), "9 Sep 2024");
        assert_eq!(describe_time((2024, 12, 31, 20, 0, 365), (2025, 1, 1, 8, 0, 0)), "Yesterday", "across new year");
        // The real clock path works too.
        let t = SystemTime::now();
        assert!(!format_time(t, t).is_empty());
    }

    #[test]
    fn listing_a_directory() {
        let dir = std::env::temp_dir().join(format!("lumen-ui-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.txt"), "hello").unwrap();
        std::os::unix::fs::symlink("sub", dir.join("link")).unwrap();
        std::os::unix::fs::symlink("missing", dir.join("broken")).unwrap();
        let mut v = read_dir(&dir).unwrap();
        v.sort_by(|a, b| natural_cmp(&a.name, &b.name));
        let names: Vec<(&str, bool, u64)> = v.iter().map(|e| (e.name.as_str(), e.is_dir, e.size)).collect();
        assert_eq!(names, vec![("a.txt", false, 5), ("broken", false, 0), ("link", true, 0), ("sub", true, 0)]);
        assert!(v[2].is_symlink);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
