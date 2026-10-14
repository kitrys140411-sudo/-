//! Applications from freedesktop `.desktop` files, and search over them.
//!
//! Entries are read from `applications/` in every XDG data directory
//! (`$XDG_DATA_HOME`, then `$XDG_DATA_DIRS`) plus the Flatpak export
//! directories, which are added even when the session's `XDG_DATA_DIRS` does
//! not name them, so an app installed with Flatpak (Steam) shows up at once.
//! Earlier directories win; a `Hidden=true` entry hides the same id in later
//! ones. Field codes in `Exec` are expanded when the entry is read (Lumen
//! never passes files or URLs to apps it launches), and `Terminal=true` apps
//! are started inside lumen-term.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct App {
    /// Desktop file id: the file name without ".desktop", with subdirectories
    /// joined by '-' (e.g. "lumen-term", "kde-foo"); also matched against
    /// window app ids.
    pub id: String,
    pub name: String,
    pub comment: String,
    pub keywords: Vec<String>,
    /// The command line with field codes expanded, split into words.
    pub exec: Vec<String>,
    pub icon: String,
    pub terminal: bool,
}

impl App {
    /// What to run: the command, or lumen-term running it for terminal apps
    /// (its window carries the app's id, so the dock matches it).
    pub fn command(&self) -> Vec<String> {
        if !self.terminal {
            return self.exec.clone();
        }
        let mut cmd: Vec<String> = ["lumen-term", "--app-id", &self.id, "--title", &self.name, "-e"].iter().map(|s| s.to_string()).collect();
        cmd.extend(self.exec.iter().cloned());
        cmd
    }
}

/// XDG data directories, most specific first: `$XDG_DATA_HOME`, the
/// entries of `$XDG_DATA_DIRS`, then the Flatpak exports (user, system).
pub fn data_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let var = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
    let mut dirs: Vec<PathBuf> = Vec::new();
    match var("XDG_DATA_HOME") {
        Some(d) => dirs.push(d.into()),
        None => dirs.extend(home.as_ref().map(|h| h.join(".local/share"))),
    }
    let system = var("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    dirs.extend(system.split(':').filter(|d| !d.is_empty()).map(PathBuf::from));
    dirs.extend(home.as_ref().map(|h| h.join(".local/share/flatpak/exports/share")));
    dirs.push("/var/lib/flatpak/exports/share".into());
    let mut seen: Vec<PathBuf> = Vec::new();
    dirs.retain(|d| {
        let d = PathBuf::from(d.to_string_lossy().trim_end_matches('/'));
        let new = !seen.contains(&d);
        seen.push(d);
        new
    });
    dirs
}

/// Directories searched for `.desktop` files, most specific first
/// (`$LUMEN_APPS_DIR` first: tests and development).
fn dirs() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::env::var_os("LUMEN_APPS_DIR").map(PathBuf::from).into_iter().collect();
    v.extend(data_dirs().into_iter().map(|d| d.join("applications")));
    v
}

/// The desktops we count as for `OnlyShowIn`/`NotShowIn`.
fn current_desktops() -> Vec<String> {
    std::env::var("XDG_CURRENT_DESKTOP")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "Lumen".into())
        .split(':')
        .map(|s| s.to_ascii_lowercase())
        .collect()
}

/// Is `prog` an executable file, given as a path or found in `$PATH`?
fn executable(prog: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let is_exec = |p: &Path| std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0);
    if prog.contains('/') {
        return is_exec(Path::new(prog));
    }
    std::env::var("PATH").unwrap_or_else(|_| "/usr/local/bin:/usr/bin:/bin".into()).split(':').any(|d| is_exec(&Path::new(d).join(prog)))
}

/// Undo the escapes of desktop-entry string values (\s \n \t \r \\).
fn unescape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut chars = v.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(o) => {
                out.push('\\');
                out.push(o);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// What the `%i`, `%c` and `%k` field codes expand to.
#[derive(Debug, Clone, Default)]
pub struct ExecContext<'a> {
    pub icon: &'a str,
    pub name: &'a str,
    pub path: &'a str,
}

/// Split an Exec value into words (desktop-entry quoting rules) and expand
/// the field codes: file and URL codes (%f %F %u %U) to nothing, %i to
/// "--icon ICON", %c to the name, %k to the entry's path, %% to "%".
pub fn parse_exec(exec: &str, ctx: &ExecContext) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_word = true;
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => {
                            if let Some(n) = chars.next() {
                                cur.push(n);
                            }
                        }
                        c => cur.push(c),
                    }
                }
            }
            ' ' | '\t' | '\n' => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            '%' => match chars.next() {
                Some('%') => {
                    cur.push('%');
                    in_word = true;
                }
                // Only as a word of its own: two words, or nothing without an icon.
                Some('i') if !in_word => {
                    if !ctx.icon.is_empty() {
                        words.push("--icon".into());
                        words.push(ctx.icon.into());
                    }
                }
                Some('c') => {
                    cur.push_str(ctx.name);
                    in_word = true;
                }
                Some('k') => {
                    cur.push_str(ctx.path);
                    in_word = true;
                }
                // %f %F %u %U (no files to pass) and the deprecated codes.
                Some(_) | None => {}
            },
            c => {
                cur.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    words
}

/// Parse one desktop file; None for hidden, non-application or broken
/// entries, and for ones not meant for this desktop.
#[cfg(test)]
pub fn parse(id: &str, text: &str) -> Option<App> {
    parse_at(id, text, "", &current_desktops(), &executable)
}

fn parse_at(id: &str, text: &str, path: &str, desktops: &[String], exists: &dyn Fn(&str) -> bool) -> Option<App> {
    let mut in_entry = false;
    let mut get = std::collections::HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            // Localized keys (Name[de]) are ignored: the UI is English.
            let k = k.trim();
            if !k.contains('[') {
                get.insert(k.to_string(), unescape(v.trim()));
            }
        }
    }
    let is_true = |k: &str| get.get(k).is_some_and(|v| v == "true");
    if get.get("Type").map(String::as_str) != Some("Application") || is_true("NoDisplay") || is_true("Hidden") {
        return None;
    }
    let list = |k: &str| -> Vec<String> {
        get.get(k).map(|v| v.split(';').map(str::trim).filter(|s| !s.is_empty()).map(|s| s.to_lowercase()).collect()).unwrap_or_default()
    };
    let only = list("OnlyShowIn");
    if !only.is_empty() && !only.iter().any(|d| desktops.contains(d)) {
        return None;
    }
    if list("NotShowIn").iter().any(|d| desktops.contains(d)) {
        return None;
    }
    if let Some(t) = get.get("TryExec").filter(|t| !t.is_empty()) {
        if !exists(t) {
            return None;
        }
    }
    let name = get.get("Name")?.clone();
    let icon = get.get("Icon").cloned().unwrap_or_default();
    let exec = parse_exec(get.get("Exec")?, &ExecContext { icon: &icon, name: &name, path });
    if exec.is_empty() {
        return None;
    }
    Some(App {
        id: id.to_string(),
        name,
        comment: get.get("Comment").cloned().unwrap_or_default(),
        keywords: list("Keywords"),
        exec,
        icon,
        terminal: is_true("Terminal"),
    })
}

/// A cheap fingerprint of the application directories (their modification
/// times): the app list is re-read only when it changes.
pub fn dirs_stamp() -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for dir in dirs() {
        let t = std::fs::metadata(&dir).and_then(|m| m.modified()).ok();
        (dir, t).hash(&mut h);
    }
    h.finish()
}

/// `.desktop` files under `dir` as (desktop file id, path); subdirectories
/// prefix the id ("kde/foo.desktop" is "kde-foo").
fn entries(dir: &Path, prefix: &str, depth: usize, out: &mut Vec<(String, PathBuf)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut list: Vec<_> = rd.flatten().collect();
    list.sort_by_key(|e| e.file_name());
    for e in list {
        let path = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if depth < 3 {
                entries(&path, &format!("{prefix}{name}-"), depth + 1, out);
            }
        } else if let Some(stem) = name.strip_suffix(".desktop") {
            out.push((format!("{prefix}{stem}"), path));
        }
    }
}

pub fn load() -> Vec<App> {
    load_from(&dirs(), &current_desktops())
}

fn load_from(dirs: &[PathBuf], desktops: &[String]) -> Vec<App> {
    let mut apps: Vec<App> = Vec::new();
    // Ids already decided (shown or hidden): earlier directories win.
    let mut seen = std::collections::HashSet::new();
    for dir in dirs {
        let mut found = Vec::new();
        entries(dir, "", 0, &mut found);
        for (id, path) in found {
            if !seen.insert(id.clone()) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            if let Some(app) = parse_at(&id, &text, &path.to_string_lossy(), desktops, &executable) {
                apps.push(app);
            }
        }
    }
    apps.sort_by_key(|a| a.name.to_lowercase());
    apps
}

/// Relevance of `app` for `query` (0 = no match): name prefix > word prefix >
/// name substring > keyword/comment match.
pub fn score(app: &App, query: &str) -> u32 {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return 0;
    }
    let name = app.name.to_lowercase();
    if name.starts_with(&q) {
        return 100;
    }
    if name.split(|c: char| !c.is_alphanumeric()).any(|w| w.starts_with(&q)) {
        return 80;
    }
    if app.id.to_lowercase().starts_with(&q) {
        return 70;
    }
    if name.contains(&q) {
        return 60;
    }
    if app.keywords.iter().any(|k| k.starts_with(&q)) {
        return 40;
    }
    if app.comment.to_lowercase().contains(&q) {
        return 20;
    }
    0
}

/// Matching apps, best first.
pub fn search<'a>(apps: &'a [App], query: &str) -> Vec<&'a App> {
    let mut v: Vec<(u32, &App)> = apps.iter().map(|a| (score(a, query), a)).filter(|(s, _)| *s > 0).collect();
    v.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(&b.1.name)));
    v.into_iter().map(|(_, a)| a).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TERM: &str = "[Desktop Entry]\nType=Application\nName=Terminal\nName[de]=Konsole\nComment=Use the command line\n\
        Keywords=shell;prompt;command;\nExec=lumen-term --working-directory \"/home/a b\" %F\nIcon=utilities-terminal\n\
        [Desktop Action new]\nName=New Window\nExec=other\n";

    fn ctx() -> ExecContext<'static> {
        ExecContext { icon: "steam", name: "Steam", path: "/usr/share/applications/steam.desktop" }
    }

    #[test]
    fn parses_entry_and_exec_quoting() {
        let app = parse("lumen-term", TERM).unwrap();
        assert_eq!(app.name, "Terminal");
        assert_eq!(app.exec, vec!["lumen-term", "--working-directory", "/home/a b"]);
        assert_eq!(app.keywords, vec!["shell", "prompt", "command"]);
        assert_eq!(app.command(), app.exec);
        assert_eq!(parse_exec("echo 100%% done %u", &ctx()), vec!["echo", "100%", "done"]);
    }

    #[test]
    fn field_codes() {
        let c = ctx();
        assert_eq!(parse_exec("steam %i -title %c %U", &c), vec!["steam", "--icon", "steam", "-title", "Steam"]);
        assert_eq!(parse_exec("app --from=%k %f", &c), vec!["app", "--from=/usr/share/applications/steam.desktop"]);
        assert_eq!(parse_exec("app %i", &ExecContext::default()), vec!["app"]);
        assert_eq!(parse_exec("app \"%c literal\" %d%D%n%N%v%m", &c), vec!["app", "%c literal"]);
        // Flatpak's exported entries keep their file-forwarding markers balanced.
        let flatpak = "/usr/bin/flatpak run --branch=stable --arch=x86_64 --command=/app/bin/steam-wrapper --file-forwarding \
            com.valvesoftware.Steam @@u %U @@";
        let words = parse_exec(flatpak, &c);
        assert_eq!(&words[words.len() - 3..], ["com.valvesoftware.Steam", "@@u", "@@"]);
        // General escapes come first: "\\s" is a space, "\\\\" a backslash that quoting then keeps.
        let app = parse_at("x", "[Desktop Entry]\nType=Application\nName=A\\sB\nExec=\"/opt/my\\\\\\\\app\" --x\n", "", &[], &|_| true).unwrap();
        assert_eq!((app.name.as_str(), app.exec.clone()), ("A B", vec!["/opt/my\\app".to_string(), "--x".to_string()]));
    }

    #[test]
    fn hidden_and_non_apps_are_skipped() {
        assert!(parse("x", "[Desktop Entry]\nType=Application\nName=X\nExec=x\nNoDisplay=true\n").is_none());
        assert!(parse("x", "[Desktop Entry]\nType=Link\nName=X\nURL=http://x\n").is_none());
        assert!(parse("x", "[Desktop Entry]\nType=Application\nName=X\n").is_none());
    }

    #[test]
    fn desktops_and_try_exec() {
        let lumen = vec!["lumen".to_string()];
        let entry = |extra: &str| format!("[Desktop Entry]\nType=Application\nName=X\nExec=x\n{extra}");
        let shown = |extra: &str| parse_at("x", &entry(extra), "", &lumen, &|p| p == "/usr/bin/present").is_some();
        assert!(shown(""));
        assert!(!shown("OnlyShowIn=GNOME;KDE;\n"));
        assert!(shown("OnlyShowIn=GNOME;Lumen;\n"));
        assert!(!shown("NotShowIn=Lumen;\n"));
        assert!(shown("NotShowIn=KDE;\n"));
        assert!(!shown("TryExec=/usr/bin/missing\n"));
        assert!(shown("TryExec=/usr/bin/present\n"));
        assert!(executable("sh") && !executable("no-such-program-here"));
    }

    #[test]
    fn terminal_apps_run_in_lumen_term() {
        let app = parse_at("htop", "[Desktop Entry]\nType=Application\nName=Htop\nExec=htop -d 10\nTerminal=true\n", "", &[], &|_| true).unwrap();
        assert_eq!(app.command(), vec!["lumen-term", "--app-id", "htop", "--title", "Htop", "-e", "htop", "-d", "10"]);
    }

    #[test]
    fn directories_ids_and_overrides() {
        let root = std::env::temp_dir().join(format!("lumen-apps-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (user, system) = (root.join("user"), root.join("system"));
        std::fs::create_dir_all(system.join("kde")).unwrap();
        std::fs::create_dir_all(&user).unwrap();
        let entry = |name: &str| format!("[Desktop Entry]\nType=Application\nName={name}\nExec=/bin/true\n");
        std::fs::write(system.join("a.desktop"), entry("System A")).unwrap();
        std::fs::write(system.join("b.desktop"), entry("B")).unwrap();
        std::fs::write(system.join("kde/c.desktop"), entry("C")).unwrap();
        std::fs::write(user.join("a.desktop"), entry("User A")).unwrap();
        std::fs::write(user.join("b.desktop"), "[Desktop Entry]\nType=Application\nName=B\nExec=b\nHidden=true\n").unwrap();
        let apps = load_from(&[user, system], &[]);
        let names: Vec<(&str, &str)> = apps.iter().map(|a| (a.id.as_str(), a.name.as_str())).collect();
        assert_eq!(names, vec![("kde-c", "C"), ("a", "User A")]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn xdg_dirs_include_flatpak_exports() {
        let dirs = data_dirs();
        assert!(dirs.contains(&PathBuf::from("/var/lib/flatpak/exports/share")), "{dirs:?}");
        assert!(dirs.iter().any(|d| d.ends_with("share") && d.starts_with("/usr")), "{dirs:?}");
        let mut uniq = dirs.clone();
        uniq.dedup();
        assert_eq!(uniq.len(), dirs.len());
    }

    #[test]
    fn search_ranks_name_prefix_first() {
        let term = parse("lumen-term", TERM).unwrap();
        let mut files = term.clone();
        files.name = "Files".into();
        files.id = "files".into();
        files.keywords = vec!["folder".into()];
        files.comment = "Browse the terminal-free file system".into();
        let apps = vec![files.clone(), term.clone()];
        assert_eq!(search(&apps, "term")[0].id, "lumen-term");
        assert_eq!(search(&apps, "fold")[0].id, "files");
        assert_eq!(search(&apps, "shell").len(), 1);
        assert!(search(&apps, "zzz").is_empty());
    }
}
