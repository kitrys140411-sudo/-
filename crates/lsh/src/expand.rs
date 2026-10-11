//! Word expansion: tilde, parameters, command substitution, arithmetic,
//! field splitting, pathname expansion and quote removal (POSIX order).

use crate::arith;
use crate::ast::{Param, ParamOp, Program, Word, WordPart};
use crate::glob;

/// What expansion needs from the shell.
pub trait Env {
    fn get_var(&self, name: &str) -> Option<String>;
    fn set_var(&mut self, name: &str, value: &str) -> std::result::Result<(), String>;
    fn positional(&self) -> Vec<String>;
    /// `$?`, `$$`, `$!`, `$-`, `$0`.
    fn special(&self, c: char) -> Option<String>;
    fn command_subst(&mut self, prog: &Program) -> String;
    fn home_dir(&self, user: &str) -> Option<String>;
    fn nounset(&self) -> bool;
    fn noglob(&self) -> bool;
}

#[derive(Debug)]
pub struct ExpandError(pub String);

type Result<T> = std::result::Result<T, ExpandError>;

/// One character of an expanded word with its provenance.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Ch {
    c: char,
    /// Quoted: no splitting, no globbing.
    quoted: bool,
    /// Produced by an unquoted expansion: subject to field splitting.
    split: bool,
}

#[derive(Debug, Clone)]
enum Piece {
    Ch(Ch),
    /// Hard field boundary ("$@" between parameters).
    Break,
    /// Marks that a quoted (possibly empty) string was present, so the field survives.
    QuotedEmpty,
    /// `"$@"` with no parameters: the enclosing quotes alone do not make a field.
    CancelKeep,
}

struct Expander<'a, E: Env> {
    env: &'a mut E,
}

struct ArithVars<'a, E: Env>(&'a mut E);

impl<E: Env> arith::Vars for ArithVars<'_, E> {
    fn get(&self, name: &str) -> Option<String> {
        self.0.get_var(name)
    }
    fn set(&mut self, name: &str, value: String) -> std::result::Result<(), String> {
        self.0.set_var(name, &value)
    }
}

fn push_str(out: &mut Vec<Piece>, s: &str, quoted: bool, split: bool) {
    if quoted && s.is_empty() {
        out.push(Piece::QuotedEmpty);
    }
    for c in s.chars() {
        out.push(Piece::Ch(Ch { c, quoted, split }));
    }
}

impl<E: Env> Expander<'_, E> {
    fn param_value(&mut self, name: &str) -> Option<String> {
        match name {
            "@" | "*" => {
                let p = self.env.positional();
                if p.is_empty() {
                    None
                } else {
                    let sep = self.ifs().chars().next().map(String::from).unwrap_or_default();
                    Some(p.join(if name == "*" { &sep } else { " " }))
                }
            }
            "#" => Some(self.env.positional().len().to_string()),
            n if n.len() == 1 && "?$!-0".contains(n) => self.env.special(n.chars().next().unwrap()),
            n if n.chars().all(|c| c.is_ascii_digit()) => {
                let i: usize = n.parse().ok()?;
                if i == 0 {
                    return self.env.special('0');
                }
                self.env.positional().get(i - 1).cloned()
            }
            n => self.env.get_var(n),
        }
    }

    fn ifs(&self) -> String {
        self.env.get_var("IFS").unwrap_or_else(|| " \t\n".to_string())
    }

    /// Expand a word to a plain string (no splitting/globbing), e.g. for the
    /// operand of `${x:-word}` or an assignment.
    fn word_string(&mut self, w: &Word) -> Result<String> {
        let mut out = Vec::new();
        self.parts(&w.0, false, &mut out)?;
        Ok(out
            .iter()
            .filter_map(|p| match p {
                Piece::Ch(ch) => Some(ch.c),
                Piece::Break => Some(' '),
                Piece::QuotedEmpty | Piece::CancelKeep => None,
            })
            .collect())
    }

    /// Expand a word to a pattern (quoted chars stay literal).
    fn word_pattern(&mut self, w: &Word) -> Result<Vec<(char, bool)>> {
        let mut out = Vec::new();
        self.parts(&w.0, false, &mut out)?;
        Ok(out
            .iter()
            .filter_map(|p| match p {
                Piece::Ch(ch) => Some((ch.c, ch.quoted)),
                _ => None,
            })
            .collect())
    }

    fn parts(&mut self, parts: &[WordPart], in_dquote: bool, out: &mut Vec<Piece>) -> Result<()> {
        for part in parts {
            match part {
                WordPart::Literal(s) => push_str(out, s, in_dquote, false),
                WordPart::Quoted(s) => push_str(out, s, true, false),
                WordPart::DoubleQuoted(inner) => {
                    out.push(Piece::QuotedEmpty);
                    self.parts(inner, true, out)?;
                }
                WordPart::Tilde(user) => {
                    let home = if user.is_empty() { self.env.get_var("HOME") } else { self.env.home_dir(user) };
                    match home {
                        Some(h) => push_str(out, &h, true, false),
                        None => push_str(out, &format!("~{user}"), false, false),
                    }
                }
                WordPart::Param(p) => self.param(p, in_dquote, out)?,
                WordPart::CommandSubst(prog) => {
                    let s = self.env.command_subst(prog);
                    push_str(out, &s, in_dquote, !in_dquote);
                }
                WordPart::Arith(w) => {
                    let text = self.word_string(w)?;
                    let v = arith::eval(&text, &mut ArithVars(self.env)).map_err(|e| ExpandError(format!("{}: {e}", text.trim())))?;
                    push_str(out, &v.to_string(), in_dquote, !in_dquote);
                }
            }
        }
        Ok(())
    }

    fn param(&mut self, p: &Param, in_dquote: bool, out: &mut Vec<Piece>) -> Result<()> {
        let name = p.name.as_str();
        // "$@" expands to one field per positional parameter.
        if name == "@" && in_dquote && p.op == ParamOp::Plain {
            let params = self.env.positional();
            for (i, v) in params.iter().enumerate() {
                if i > 0 {
                    out.push(Piece::Break);
                }
                push_str(out, v, true, false);
            }
            if params.is_empty() {
                out.push(Piece::CancelKeep);
            }
            return Ok(());
        }
        if (name == "@" || name == "*") && !in_dquote && p.op == ParamOp::Plain {
            let params = self.env.positional();
            for (i, v) in params.iter().enumerate() {
                if i > 0 {
                    out.push(Piece::Break);
                }
                push_str(out, v, false, true);
            }
            return Ok(());
        }
        let value = self.param_value(name);
        let unset_or_null = |colon: bool, v: &Option<String>| match v {
            None => true,
            Some(s) => colon && s.is_empty(),
        };
        let emit = |out: &mut Vec<Piece>, s: &str| push_str(out, s, in_dquote, !in_dquote);
        match &p.op {
            ParamOp::Plain => match value {
                Some(v) => emit(out, &v),
                None => {
                    if self.env.nounset() && !matches!(name, "@" | "*") {
                        return Err(ExpandError(format!("{name}: parameter not set")));
                    }
                }
            },
            ParamOp::Length => {
                let n = match name {
                    "@" | "*" => self.env.positional().len(),
                    _ => value.map(|v| v.chars().count()).unwrap_or(0),
                };
                emit(out, &n.to_string());
            }
            ParamOp::Default { colon, word } => {
                if unset_or_null(*colon, &value) {
                    let mut tmp = Vec::new();
                    self.parts(&word.0, in_dquote, &mut tmp)?;
                    // The default word's own quoting applies.
                    for piece in tmp {
                        out.push(match piece {
                            Piece::Ch(ch) => Piece::Ch(Ch { split: ch.split || (!ch.quoted && !in_dquote), ..ch }),
                            other => other,
                        });
                    }
                } else {
                    emit(out, &value.unwrap());
                }
            }
            ParamOp::Assign { colon, word } => {
                if unset_or_null(*colon, &value) {
                    let v = self.word_string(word)?;
                    if !crate::parser::is_name(name) {
                        return Err(ExpandError(format!("{name}: cannot assign in this way")));
                    }
                    self.env.set_var(name, &v).map_err(ExpandError)?;
                    emit(out, &v);
                } else {
                    emit(out, &value.unwrap());
                }
            }
            ParamOp::Error { colon, word } => {
                if unset_or_null(*colon, &value) {
                    let msg = self.word_string(word)?;
                    let msg = if msg.is_empty() { "parameter null or not set".to_string() } else { msg };
                    return Err(ExpandError(format!("{name}: {msg}")));
                }
                emit(out, &value.unwrap());
            }
            ParamOp::Alternative { colon, word } => {
                if !unset_or_null(*colon, &value) {
                    let mut tmp = Vec::new();
                    self.parts(&word.0, in_dquote, &mut tmp)?;
                    out.extend(tmp);
                }
            }
            ParamOp::RemoveSuffix { longest, pattern } | ParamOp::RemovePrefix { longest, pattern } => {
                let suffix = matches!(p.op, ParamOp::RemoveSuffix { .. });
                let v = value.unwrap_or_default();
                let pat = self.word_pattern(pattern)?;
                emit(out, &glob::remove_affix(&v, &pat, suffix, *longest));
            }
        }
        Ok(())
    }
}

/// Split pieces into fields at IFS characters that came from unquoted
/// expansions (POSIX 2.6.5): IFS whitespace runs delimit fields and are ignored
/// at the ends; each non-whitespace IFS character delimits one field, absorbing
/// adjacent whitespace, so `a::b` yields an empty middle field.
fn split_fields(pieces: Vec<Piece>, ifs: &str) -> Vec<Vec<Ch>> {
    #[derive(PartialEq)]
    enum State {
        Start,
        InField,
        AfterWs,
        AfterNonWs,
    }
    let mut fields: Vec<Vec<Ch>> = Vec::new();
    let mut cur: Vec<Ch> = Vec::new();
    let mut keep = false;
    let mut state = State::Start;
    for piece in pieces {
        match piece {
            Piece::Break => {
                if state == State::InField || keep || !cur.is_empty() {
                    fields.push(std::mem::take(&mut cur));
                }
                keep = false;
                state = State::Start;
            }
            Piece::QuotedEmpty => {
                keep = true;
                state = State::InField;
            }
            Piece::CancelKeep => {
                if cur.is_empty() {
                    keep = false;
                    if state == State::InField {
                        state = State::Start;
                    }
                }
            }
            Piece::Ch(ch) if ch.split && ifs.contains(ch.c) => {
                let ws = ch.c.is_whitespace();
                match (&state, ws) {
                    (State::InField, true) => {
                        fields.push(std::mem::take(&mut cur));
                        keep = false;
                        state = State::AfterWs;
                    }
                    (State::InField, false) => {
                        fields.push(std::mem::take(&mut cur));
                        keep = false;
                        state = State::AfterNonWs;
                    }
                    (_, true) => {}
                    (State::AfterWs, false) => state = State::AfterNonWs,
                    (State::Start | State::AfterNonWs, false) => {
                        fields.push(Vec::new());
                        state = State::AfterNonWs;
                    }
                }
            }
            Piece::Ch(ch) => {
                cur.push(ch);
                state = State::InField;
            }
        }
    }
    if state == State::InField || keep || !cur.is_empty() {
        fields.push(cur);
    }
    fields
}

/// Full expansion of a list of words into fields (command arguments, `for` lists).
pub fn expand_words<E: Env>(env: &mut E, words: &[Word]) -> Result<Vec<String>> {
    let mut result = Vec::new();
    for w in words {
        let mut pieces = Vec::new();
        let ifs = {
            let mut ex = Expander { env };
            ex.parts(&w.0, false, &mut pieces)?;
            ex.ifs()
        };
        for field in split_fields(pieces, &ifs) {
            let pattern: Vec<(char, bool)> = field.iter().map(|ch| (ch.c, ch.quoted)).collect();
            if !env.noglob() && glob::has_meta(&pattern) {
                let matches = glob::expand_path(&pattern);
                if !matches.is_empty() {
                    result.extend(matches);
                    continue;
                }
            }
            result.push(field.iter().map(|ch| ch.c).collect());
        }
    }
    Ok(result)
}

/// Expansion without field splitting or globbing (assignments, redirect targets,
/// `case` word, here-documents).
pub fn expand_string<E: Env>(env: &mut E, w: &Word) -> Result<String> {
    Expander { env }.word_string(w)
}

/// Expansion to a pattern for `case`.
pub fn expand_pattern<E: Env>(env: &mut E, w: &Word) -> Result<Vec<(char, bool)>> {
    Expander { env }.word_pattern(w)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;
    use std::collections::HashMap;

    struct TestEnv {
        vars: HashMap<String, String>,
        args: Vec<String>,
    }

    impl Env for TestEnv {
        fn get_var(&self, name: &str) -> Option<String> {
            self.vars.get(name).cloned()
        }
        fn set_var(&mut self, name: &str, value: &str) -> std::result::Result<(), String> {
            self.vars.insert(name.into(), value.into());
            Ok(())
        }
        fn positional(&self) -> Vec<String> {
            self.args.clone()
        }
        fn special(&self, c: char) -> Option<String> {
            match c {
                '?' => Some("3".into()),
                '0' => Some("lsh".into()),
                _ => None,
            }
        }
        fn command_subst(&mut self, _prog: &Program) -> String {
            "sub out".into()
        }
        fn home_dir(&self, user: &str) -> Option<String> {
            (user == "bob").then(|| "/home/bob".into())
        }
        fn nounset(&self) -> bool {
            false
        }
        fn noglob(&self) -> bool {
            true
        }
    }

    fn env() -> TestEnv {
        let mut vars = HashMap::new();
        vars.insert("x".into(), "a b  c".into());
        vars.insert("e".into(), "".into());
        vars.insert("HOME".into(), "/home/me".into());
        vars.insert("path".into(), "/usr/lib/file.tar.gz".into());
        TestEnv { vars, args: vec!["one".into(), "two words".into()] }
    }

    fn fields(src: &str) -> Vec<String> {
        let prog = parser::parse(&format!("echo {src}")).unwrap();
        let crate::ast::Command::Simple(s) = &prog.0[0].and_or.first.commands[0] else { panic!() };
        expand_words(&mut env(), &s.words[1..]).unwrap()
    }

    #[test]
    fn splitting_and_quotes() {
        assert_eq!(fields("$x"), vec!["a", "b", "c"]);
        assert_eq!(fields("\"$x\""), vec!["a b  c"]);
        assert_eq!(fields("$e"), Vec::<String>::new());
        assert_eq!(fields("\"$e\""), vec![""]);
        assert_eq!(fields("''"), vec![""]);
        assert_eq!(fields("pre$x"), vec!["prea", "b", "c"]);
        assert_eq!(fields("'$x'"), vec!["$x"]);
        assert_eq!(fields("$(cmd)"), vec!["sub", "out"]);
        assert_eq!(fields("\"$(cmd)\""), vec!["sub out"]);
    }

    #[test]
    fn positional() {
        assert_eq!(fields("\"$@\""), vec!["one", "two words"]);
        assert_eq!(fields("$@"), vec!["one", "two", "words"]);
        assert_eq!(fields("\"$*\""), vec!["one two words"]);
        assert_eq!(fields("$#"), vec!["2"]);
        assert_eq!(fields("\"x$@y\""), vec!["xone", "two wordsy"]);
        assert_eq!(fields("$1"), vec!["one"]);
        assert_eq!(fields("$0 $?"), vec!["lsh", "3"]);
    }

    #[test]
    fn param_ops() {
        assert_eq!(fields("${missing:-def ault}"), vec!["def", "ault"]);
        assert_eq!(fields("\"${missing:-def ault}\""), vec!["def ault"]);
        assert_eq!(fields("${e-unset}"), Vec::<String>::new());
        assert_eq!(fields("${e:-null}"), vec!["null"]);
        assert_eq!(fields("${x:+alt}"), vec!["alt"]);
        assert_eq!(fields("${missing:+alt}"), Vec::<String>::new());
        assert_eq!(fields("${#x}"), vec!["6"]);
        assert_eq!(fields("${path##*/}"), vec!["file.tar.gz"]);
        assert_eq!(fields("${path%.*}"), vec!["/usr/lib/file.tar"]);
        assert_eq!(fields("${path%%.*}"), vec!["/usr/lib/file"]);
        assert_eq!(fields("${path#*/}"), vec!["usr/lib/file.tar.gz"]);
        let mut e = env();
        let prog = parser::parse("echo ${new:=val}").unwrap();
        let crate::ast::Command::Simple(s) = &prog.0[0].and_or.first.commands[0] else { panic!() };
        assert_eq!(expand_words(&mut e, &s.words[1..]).unwrap(), vec!["val"]);
        assert_eq!(e.vars["new"], "val");
        let prog = parser::parse("echo ${missing?boom}").unwrap();
        let crate::ast::Command::Simple(s) = &prog.0[0].and_or.first.commands[0] else { panic!() };
        assert!(expand_words(&mut env(), &s.words[1..]).unwrap_err().0.contains("boom"));
    }

    #[test]
    fn tilde_and_arith() {
        assert_eq!(fields("~ ~/x ~bob/y ~nobody"), vec!["/home/me", "/home/me/x", "/home/bob/y", "~nobody"]);
        assert_eq!(fields("$((1 + 2 * 3))"), vec!["7"]);
        assert_eq!(fields("$(( $# * 10 ))"), vec!["20"]);
    }

    #[test]
    fn custom_ifs() {
        let mut e = env();
        e.vars.insert("IFS".into(), ":".into());
        e.vars.insert("p".into(), "a::b:c".into());
        let prog = parser::parse("echo $p").unwrap();
        let crate::ast::Command::Simple(s) = &prog.0[0].and_or.first.commands[0] else { panic!() };
        assert_eq!(expand_words(&mut e, &s.words[1..]).unwrap(), vec!["a", "", "b", "c"]);
        for (value, want) in [(":a", vec!["", "a"]), ("a:", vec!["a"]), ("a", vec!["a"])] {
            e.vars.insert("p".into(), value.into());
            assert_eq!(expand_words(&mut e, &s.words[1..]).unwrap(), want, "{value:?}");
        }
        e.vars.insert("IFS".into(), " :".into());
        e.vars.insert("p".into(), " a : b  c: ".into());
        assert_eq!(expand_words(&mut e, &s.words[1..]).unwrap(), vec!["a", "b", "c"]);
    }

    #[test]
    fn empty_positional() {
        let mut e = env();
        e.args.clear();
        for (src, want) in [("\"$@\"", vec![]), ("\"x$@\"", vec!["x"]), ("\"$@\"''", vec![""]), ("$@", vec![])] {
            let prog = parser::parse(&format!("echo {src}")).unwrap();
            let crate::ast::Command::Simple(s) = &prog.0[0].and_or.first.commands[0] else { panic!() };
            let want: Vec<String> = want.into_iter().map(String::from).collect();
            assert_eq!(expand_words(&mut e, &s.words[1..]).unwrap(), want, "{src}");
        }
    }
}
