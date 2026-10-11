//! Pattern matching (`case`, `${x%pat}`) and pathname expansion.
//!
//! Patterns are sequences of `(char, quoted)`; quoted characters always match
//! literally.

#[derive(Debug, Clone, PartialEq)]
enum Pat {
    Lit(char),
    Any,
    Star,
    Class { negate: bool, items: Vec<ClassItem> },
}

#[derive(Debug, Clone, PartialEq)]
enum ClassItem {
    Char(char),
    Range(char, char),
    Named(String),
}

fn compile(pattern: &[(char, bool)]) -> Vec<Pat> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < pattern.len() {
        let (c, quoted) = pattern[i];
        if quoted {
            out.push(Pat::Lit(c));
            i += 1;
            continue;
        }
        match c {
            '*' => {
                if out.last() != Some(&Pat::Star) {
                    out.push(Pat::Star);
                }
                i += 1;
            }
            '?' => {
                out.push(Pat::Any);
                i += 1;
            }
            '[' => match parse_class(pattern, i + 1) {
                Some((class, next)) => {
                    out.push(class);
                    i = next;
                }
                None => {
                    out.push(Pat::Lit('['));
                    i += 1;
                }
            },
            '\\' if i + 1 < pattern.len() => {
                out.push(Pat::Lit(pattern[i + 1].0));
                i += 2;
            }
            c => {
                out.push(Pat::Lit(c));
                i += 1;
            }
        }
    }
    out
}

fn parse_class(p: &[(char, bool)], mut i: usize) -> Option<(Pat, usize)> {
    let mut negate = false;
    if i < p.len() && !p[i].1 && (p[i].0 == '!' || p[i].0 == '^') {
        negate = true;
        i += 1;
    }
    let mut items = Vec::new();
    let mut first = true;
    while i < p.len() {
        let (c, quoted) = p[i];
        if c == ']' && !quoted && !first {
            return Some((Pat::Class { negate, items }, i + 1));
        }
        first = false;
        if c == '[' && !quoted && i + 1 < p.len() && p[i + 1].0 == ':' {
            // [:alpha:]
            let rest: String = p[i + 2..].iter().map(|x| x.0).collect();
            if let Some(end) = rest.find(":]") {
                items.push(ClassItem::Named(rest[..end].to_string()));
                i += 2 + end + 2;
                continue;
            }
        }
        if i + 2 < p.len() && p[i + 1].0 == '-' && !p[i + 1].1 && p[i + 2].0 != ']' {
            items.push(ClassItem::Range(c, p[i + 2].0));
            i += 3;
        } else {
            items.push(ClassItem::Char(c));
            i += 1;
        }
    }
    None
}

fn class_matches(items: &[ClassItem], c: char) -> bool {
    items.iter().any(|it| match it {
        ClassItem::Char(x) => *x == c,
        ClassItem::Range(a, b) => *a <= c && c <= *b,
        ClassItem::Named(n) => match n.as_str() {
            "alpha" => c.is_alphabetic(),
            "digit" => c.is_ascii_digit(),
            "alnum" => c.is_alphanumeric(),
            "upper" => c.is_uppercase(),
            "lower" => c.is_lowercase(),
            "space" => c.is_whitespace(),
            "blank" => c == ' ' || c == '\t',
            "punct" => c.is_ascii_punctuation(),
            "xdigit" => c.is_ascii_hexdigit(),
            "cntrl" => c.is_control(),
            "print" => !c.is_control(),
            "graph" => !c.is_control() && !c.is_whitespace(),
            _ => false,
        },
    })
}

fn match_compiled(p: &[Pat], t: &[char]) -> bool {
    // Iterative matcher with single-star backtracking (linear for typical patterns).
    let (mut pi, mut ti) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        let ok = match p.get(pi) {
            Some(Pat::Lit(c)) => *c == t[ti],
            Some(Pat::Any) => true,
            Some(Pat::Class { negate, items }) => class_matches(items, t[ti]) != *negate,
            Some(Pat::Star) => {
                star = Some((pi, ti));
                pi += 1;
                continue;
            }
            None => false,
        };
        if ok {
            pi += 1;
            ti += 1;
        } else if let Some((sp, st)) = star {
            pi = sp + 1;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    while p.get(pi) == Some(&Pat::Star) {
        pi += 1;
    }
    pi == p.len()
}

pub fn matches(pattern: &[(char, bool)], text: &str) -> bool {
    let p = compile(pattern);
    let t: Vec<char> = text.chars().collect();
    match_compiled(&p, &t)
}

/// Does the pattern contain unquoted glob metacharacters?
pub fn has_meta(pattern: &[(char, bool)]) -> bool {
    pattern.iter().any(|&(c, q)| !q && matches!(c, '*' | '?' | '['))
}

/// Remove the shortest/longest matching prefix or suffix.
pub fn remove_affix(value: &str, pattern: &[(char, bool)], suffix: bool, longest: bool) -> String {
    let p = compile(pattern);
    let chars: Vec<char> = value.chars().collect();
    let n = chars.len();
    let candidates: Box<dyn Iterator<Item = usize>> = if longest { Box::new((0..=n).rev()) } else { Box::new(0..=n) };
    for len in candidates {
        if suffix {
            if match_compiled(&p, &chars[n - len..]) {
                return chars[..n - len].iter().collect();
            }
        } else if match_compiled(&p, &chars[..len]) {
            return chars[len..].iter().collect();
        }
    }
    value.to_string()
}

/// Pathname expansion. Returns sorted matches (empty if none).
pub fn expand_path(pattern: &[(char, bool)]) -> Vec<String> {
    let absolute = pattern.first().map(|p| p.0) == Some('/');
    // Split into components at unquoted and quoted '/'.
    let mut comps: Vec<Vec<(char, bool)>> = vec![Vec::new()];
    for &(c, q) in pattern {
        if c == '/' {
            comps.push(Vec::new());
        } else {
            comps.last_mut().unwrap().push((c, q));
        }
    }
    let mut results: Vec<String> = vec![if absolute { "/".to_string() } else { String::new() }];
    let comp_count = comps.len();
    for (idx, comp) in comps.into_iter().enumerate() {
        if comp.is_empty() {
            if idx == 0 {
                continue; // leading '/'
            }
            // "dir/" trailing slash or "//": keep directories only.
            results = results
                .into_iter()
                .filter(|r| r.is_empty() || std::path::Path::new(r).is_dir())
                .map(|r| if r.ends_with('/') { r } else { format!("{r}/") })
                .collect();
            continue;
        }
        let last = idx + 1 == comp_count;
        let mut next = Vec::new();
        for base in &results {
            let join = |name: &str| -> String {
                if base.is_empty() {
                    name.to_string()
                } else if base.ends_with('/') {
                    format!("{base}{name}")
                } else {
                    format!("{base}/{name}")
                }
            };
            if !has_meta(&comp) {
                let name: String = comp.iter().map(|x| x.0).collect();
                let path = join(&name);
                if std::fs::symlink_metadata(&path).is_ok() && (last || std::path::Path::new(&path).is_dir()) {
                    next.push(path);
                }
                continue;
            }
            let dir = if base.is_empty() { "." } else { base.as_str() };
            let Ok(rd) = std::fs::read_dir(dir) else { continue };
            let p = compile(&comp);
            let dot_ok = comp.first().map(|x| x.0) == Some('.');
            let mut names: Vec<String> = rd.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
            names.sort();
            for name in names {
                if name.starts_with('.') && !dot_ok {
                    continue;
                }
                let t: Vec<char> = name.chars().collect();
                if match_compiled(&p, &t) {
                    let path = join(&name);
                    if last || std::path::Path::new(&path).is_dir() {
                        next.push(path);
                    }
                }
            }
        }
        results = next;
        if results.is_empty() {
            break;
        }
    }
    results.sort();
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Vec<(char, bool)> {
        s.chars().map(|c| (c, false)).collect()
    }

    #[test]
    fn matching() {
        assert!(matches(&p("*.rs"), "main.rs"));
        assert!(!matches(&p("*.rs"), "main.rc"));
        assert!(matches(&p("a?c"), "abc"));
        assert!(matches(&p("[a-c]x"), "bx"));
        assert!(!matches(&p("[!a-c]x"), "bx"));
        assert!(matches(&p("[[:digit:]]*"), "7up"));
        assert!(matches(&p("*"), ""));
        assert!(matches(&p("a*b*c"), "aXXbYYc"));
        assert!(!matches(&p("a*b*c"), "aXXbYY"));
        assert!(matches(&p("[]]"), "]"));
        assert!(matches(&p("[a"), "[a"));
        // Quoted metacharacters are literal.
        assert!(!matches(&[('*', true)], "x"));
        assert!(matches(&[('*', true)], "*"));
    }

    #[test]
    fn affixes() {
        assert_eq!(remove_affix("file.tar.gz", &p(".*"), true, false), "file.tar");
        assert_eq!(remove_affix("file.tar.gz", &p(".*"), true, true), "file");
        assert_eq!(remove_affix("/a/b/c", &p("*/"), false, false), "a/b/c");
        assert_eq!(remove_affix("/a/b/c", &p("*/"), false, true), "c");
        assert_eq!(remove_affix("abc", &p("x"), false, true), "abc");
    }

    #[test]
    fn pathname_expansion() {
        let dir = std::env::temp_dir().join(format!("lsh-glob-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        for f in ["a.txt", "b.txt", ".hidden.txt", "c.md", "sub/d.txt"] {
            std::fs::write(dir.join(f), "").unwrap();
        }
        let base = dir.to_string_lossy();
        let got = expand_path(&p(&format!("{base}/*.txt")));
        assert_eq!(got, vec![format!("{base}/a.txt"), format!("{base}/b.txt")]);
        let got = expand_path(&p(&format!("{base}/*/*.txt")));
        assert_eq!(got, vec![format!("{base}/sub/d.txt")]);
        let got = expand_path(&p(&format!("{base}/.*.txt")));
        assert_eq!(got, vec![format!("{base}/.hidden.txt")]);
        assert!(expand_path(&p(&format!("{base}/*.none"))).is_empty());
        let got = expand_path(&p(&format!("{base}/s*/")));
        assert_eq!(got, vec![format!("{base}/sub/")]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
