//! Applications from freedesktop `.desktop` files, and search over them.

#[derive(Debug, Clone, PartialEq)]
pub struct App {
    /// File stem, e.g. "lumen-term" (also matched against window app_ids).
    pub id: String,
    pub name: String,
    pub comment: String,
    pub keywords: Vec<String>,
    /// Command line with field codes removed, split into words.
    pub exec: Vec<String>,
    pub icon: String,
    pub terminal: bool,
}

/// Directories searched for `.desktop` files, most specific first.
fn dirs() -> Vec<String> {
    let mut v = Vec::new();
    if let Ok(d) = std::env::var("LUMEN_APPS_DIR") {
        v.push(d);
    }
    if let Ok(home) = std::env::var("HOME") {
        v.push(format!("{home}/.local/share/applications"));
    }
    v.push("/usr/local/share/applications".into());
    v.push("/usr/share/applications".into());
    v
}

/// Split an Exec value into words (desktop-entry quoting rules) and drop the
/// %-field codes we cannot fill in (files/URLs), keeping literal "%%".
pub fn parse_exec(exec: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = exec.chars().peekable();
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
            ' ' | '\t' => {
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
                // %f %F %u %U %i %c %k and deprecated codes expand to nothing.
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

/// Parse one desktop file; None for hidden, non-application or broken entries.
pub fn parse(id: &str, text: &str) -> Option<App> {
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
                get.insert(k.to_string(), v.trim().to_string());
            }
        }
    }
    let is_true = |k: &str| get.get(k).is_some_and(|v| v == "true");
    if get.get("Type").map(String::as_str) != Some("Application") || is_true("NoDisplay") || is_true("Hidden") {
        return None;
    }
    let exec = parse_exec(get.get("Exec")?);
    if exec.is_empty() {
        return None;
    }
    let list = |k: &str| -> Vec<String> {
        get.get(k).map(|v| v.split(';').filter(|s| !s.is_empty()).map(|s| s.to_lowercase()).collect()).unwrap_or_default()
    };
    Some(App {
        id: id.to_string(),
        name: get.get("Name")?.clone(),
        comment: get.get("Comment").cloned().unwrap_or_default(),
        keywords: list("Keywords"),
        exec,
        icon: get.get("Icon").cloned().unwrap_or_default(),
        terminal: is_true("Terminal"),
    })
}

pub fn load() -> Vec<App> {
    let mut apps: Vec<App> = Vec::new();
    for dir in dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()) != Some("desktop") {
                continue;
            }
            let id = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string();
            // Earlier directories override later ones.
            if apps.iter().any(|a| a.id == id) {
                continue;
            }
            if let Some(app) = std::fs::read_to_string(&path).ok().and_then(|t| parse(&id, &t)) {
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

    #[test]
    fn parses_entry_and_exec_quoting() {
        let app = parse("lumen-term", TERM).unwrap();
        assert_eq!(app.name, "Terminal");
        assert_eq!(app.exec, vec!["lumen-term", "--working-directory", "/home/a b"]);
        assert_eq!(app.keywords, vec!["shell", "prompt", "command"]);
        assert_eq!(parse_exec("echo 100%% done %u"), vec!["echo", "100%", "done"]);
    }

    #[test]
    fn hidden_and_non_apps_are_skipped() {
        assert!(parse("x", "[Desktop Entry]\nType=Application\nName=X\nExec=x\nNoDisplay=true\n").is_none());
        assert!(parse("x", "[Desktop Entry]\nType=Link\nName=X\nURL=http://x\n").is_none());
        assert!(parse("x", "[Desktop Entry]\nType=Application\nName=X\n").is_none());
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
