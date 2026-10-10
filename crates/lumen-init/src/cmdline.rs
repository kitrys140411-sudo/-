//! Kernel command line parsing.

/// Split a kernel command line into `(key, value)` pairs. Bare words get an
/// empty value. Double quotes group words (`foo="a b"`) and are removed.
pub fn parse(cmdline: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut in_quotes = false;
    let chars = cmdline.trim().chars();
    let flush = |w: &mut String, out: &mut Vec<(String, String)>| {
        if !w.is_empty() {
            match w.split_once('=') {
                Some((k, v)) => out.push((k.to_string(), v.to_string())),
                None => out.push((w.clone(), String::new())),
            }
            w.clear();
        }
    };
    for c in chars {
        match c {
            '"' => in_quotes = !in_quotes,
            c if c.is_whitespace() && !in_quotes => flush(&mut word, &mut out),
            c => word.push(c),
        }
    }
    flush(&mut word, &mut out);
    out
}

/// Last value of `key` (later occurrences override earlier ones, like the kernel).
pub fn get<'a>(args: &'a [(String, String)], key: &str) -> Option<&'a str> {
    args.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

pub fn has(args: &[(String, String)], key: &str) -> bool {
    args.iter().any(|(k, _)| k == key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_and_overrides() {
        let a = parse("root=PARTUUID=abc rootwait quiet init=/sbin/x root=/dev/vda2 x=\"a b\"\n");
        assert_eq!(get(&a, "root"), Some("/dev/vda2"));
        assert_eq!(get(&a, "init"), Some("/sbin/x"));
        assert!(has(&a, "rootwait"));
        assert_eq!(get(&a, "quiet"), Some(""));
        assert_eq!(get(&a, "x"), Some("a b"));
        assert_eq!(get(&a, "missing"), None);
    }

    #[test]
    fn value_with_equals() {
        let a = parse("root=PARTUUID=8d3a-1");
        assert_eq!(get(&a, "root"), Some("PARTUUID=8d3a-1"));
    }
}
