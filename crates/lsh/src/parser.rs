//! Character-level recursive-descent parser for the shell language.
//!
//! Tokenizing and parsing are interleaved: command substitutions `$(...)` are
//! parsed by recursing into the parser on the same buffer, which handles nested
//! quotes and parentheses correctly. Errors at end of input are flagged as
//! `incomplete` so the interactive shell can ask for a continuation line.

use std::collections::{HashMap, HashSet};

use crate::ast::*;

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub message: String,
    /// The input ended inside a construct (more lines would complete it).
    pub incomplete: bool,
}

type Result<T> = std::result::Result<T, ParseError>;

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Word(Word, String),
    Op(&'static str),
    IoNumber(i32),
    Newline,
    Eof,
}

const OPS: &[&str] = &[
    "&>>", "<<-", "&&", "||", ";;", "<<", ">>", "<&", ">&", "<>", ">|", "&>", "|", "&", ";", "<", ">", "(", ")",
];

const RESERVED: &[&str] = &[
    "if", "then", "else", "elif", "fi", "do", "done", "case", "esac", "while", "until", "for", "in", "{", "}", "!",
];

struct PendingHereDoc {
    id: usize,
    delimiter: String,
    strip_tabs: bool,
    quoted: bool,
}

pub struct Parser<'a> {
    src: Vec<char>,
    pos: usize,
    aliases: Option<&'a HashMap<String, String>>,
    /// Aliases already expanded at the current command position (recursion guard).
    alias_guard: HashSet<String>,
    alias_guard_end: usize,
    pending_heredocs: Vec<PendingHereDoc>,
    heredoc_bodies: Vec<Option<Word>>,
    peeked: Option<(Tok, usize, usize)>,
}

fn err<T>(message: impl Into<String>) -> Result<T> {
    Err(ParseError { message: message.into(), incomplete: false })
}

fn incomplete<T>(message: impl Into<String>) -> Result<T> {
    Err(ParseError { message: message.into(), incomplete: true })
}

pub fn is_name(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

fn is_op_start(c: char) -> bool {
    matches!(c, '|' | '&' | ';' | '<' | '>' | '(' | ')')
}

/// Plain unquoted literal text of a word, if it has no quoting/expansions.
pub fn literal_text(w: &Word) -> Option<String> {
    let mut s = String::new();
    for p in &w.0 {
        match p {
            WordPart::Literal(t) => s.push_str(t),
            _ => return None,
        }
    }
    Some(s)
}

pub fn parse(src: &str) -> Result<Program> {
    Parser::new(src, None).parse_program()
}

pub fn parse_with_aliases(src: &str, aliases: &HashMap<String, String>) -> Result<Program> {
    Parser::new(src, Some(aliases)).parse_program()
}

impl<'a> Parser<'a> {
    pub fn new(src: &str, aliases: Option<&'a HashMap<String, String>>) -> Self {
        Parser {
            src: src.chars().collect(),
            pos: 0,
            aliases,
            alias_guard: HashSet::new(),
            alias_guard_end: 0,
            pending_heredocs: Vec::new(),
            heredoc_bodies: Vec::new(),
            peeked: None,
        }
    }

    pub fn parse_program(mut self) -> Result<Program> {
        let prog = self.parse_list(&[], false)?;
        match self.next_tok()? {
            Tok::Eof => {}
            Tok::Op(op) => return err(format!("syntax error near unexpected token '{op}'")),
            Tok::Word(_, raw) => return err(format!("syntax error near unexpected word '{raw}'")),
            t => return err(format!("syntax error near {t:?}")),
        }
        if !self.pending_heredocs.is_empty() {
            return incomplete("here-document not terminated");
        }
        let mut prog = prog;
        let bodies = std::mem::take(&mut self.heredoc_bodies);
        fill_heredocs_program(&mut prog, &bodies);
        Ok(prog)
    }

    // -- character level -------------------------------------------------------------------

    fn peek_char(&self) -> Option<char> {
        self.src.get(self.pos).copied()
    }

    fn peek_at(&self, off: usize) -> Option<char> {
        self.src.get(self.pos + off).copied()
    }

    fn starts_with(&self, s: &str) -> bool {
        s.chars().enumerate().all(|(i, c)| self.peek_at(i) == Some(c))
    }

    /// Skip blanks, line continuations and comments (not newlines).
    fn skip_blanks(&mut self) {
        loop {
            match self.peek_char() {
                Some(' ' | '\t') => self.pos += 1,
                Some('\\') if self.peek_at(1) == Some('\n') => self.pos += 2,
                Some('#') => {
                    while let Some(c) = self.peek_char() {
                        if c == '\n' {
                            break;
                        }
                        self.pos += 1;
                    }
                }
                _ => break,
            }
        }
    }

    // -- tokens ------------------------------------------------------------------------------

    fn next_tok(&mut self) -> Result<Tok> {
        if let Some((t, _, end)) = self.peeked.take() {
            self.pos = end;
            if t == Tok::Newline {
                self.read_heredoc_bodies()?;
            }
            return Ok(t);
        }
        let (t, _, end) = self.lex()?;
        self.pos = end;
        if t == Tok::Newline {
            self.read_heredoc_bodies()?;
        }
        Ok(t)
    }

    fn peek_tok(&mut self) -> Result<&Tok> {
        if self.peeked.is_none() {
            let save = self.pos;
            let lexed = self.lex()?;
            self.pos = save;
            self.peeked = Some(lexed);
        }
        Ok(&self.peeked.as_ref().unwrap().0)
    }

    fn drop_peek(&mut self) {
        self.peeked = None;
    }

    /// Lex one token starting at the current position; returns (token, start, end)
    /// and leaves `pos` wherever it ended up (callers restore or commit).
    fn lex(&mut self) -> Result<(Tok, usize, usize)> {
        self.skip_blanks();
        let start = self.pos;
        let Some(c) = self.peek_char() else {
            return Ok((Tok::Eof, start, start));
        };
        if c == '\n' {
            self.pos += 1;
            return Ok((Tok::Newline, start, self.pos));
        }
        if is_op_start(c) {
            for op in OPS {
                if self.starts_with(op) {
                    self.pos += op.chars().count();
                    return Ok((Tok::Op(op), start, self.pos));
                }
            }
        }
        // IO number: digits directly followed by < or >.
        if c.is_ascii_digit() {
            let mut i = 0;
            while self.peek_at(i).is_some_and(|c| c.is_ascii_digit()) {
                i += 1;
            }
            if matches!(self.peek_at(i), Some('<' | '>')) {
                let n: String = self.src[self.pos..self.pos + i].iter().collect();
                if let Ok(fd) = n.parse() {
                    self.pos += i;
                    return Ok((Tok::IoNumber(fd), start, self.pos));
                }
            }
        }
        let word = self.read_word(false)?;
        let raw: String = self.src[start..self.pos].iter().collect();
        Ok((Tok::Word(word, raw), start, self.pos))
    }

    // -- words ---------------------------------------------------------------------------

    /// Read a word. In `in_braces` mode (inside `${...}`) blanks do not end the word
    /// and `}` does.
    fn read_word(&mut self, in_braces: bool) -> Result<Word> {
        let mut parts: Vec<WordPart> = Vec::new();
        let mut lit = String::new();
        let word_start = self.pos;
        let flush = |lit: &mut String, parts: &mut Vec<WordPart>| {
            if !lit.is_empty() {
                parts.push(WordPart::Literal(std::mem::take(lit)));
            }
        };
        if !in_braces && self.peek_char() == Some('~') {
            // Tilde prefix: up to the first '/' (or end of word), unquoted only.
            let mut j = 1;
            let mut user = String::new();
            let mut ok = true;
            while let Some(c) = self.peek_at(j) {
                if c == '/' || c.is_whitespace() || is_op_start(c) || c == ':' {
                    break;
                }
                if !(c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.') {
                    ok = false;
                    break;
                }
                user.push(c);
                j += 1;
            }
            if ok {
                self.pos += j;
                parts.push(WordPart::Tilde(user));
            }
        }
        while let Some(c) = self.peek_char() {
            match c {
                '}' if in_braces => break,
                c if !in_braces && (c.is_whitespace() || is_op_start(c)) => break,
                '\\' => {
                    match self.peek_at(1) {
                        Some('\n') => self.pos += 2,
                        Some(e) => {
                            flush(&mut lit, &mut parts);
                            parts.push(WordPart::Quoted(e.to_string()));
                            self.pos += 2;
                        }
                        None => {
                            // A trailing backslash at end of input continues the line.
                            return incomplete("line continuation");
                        }
                    }
                }
                '\'' => {
                    flush(&mut lit, &mut parts);
                    self.pos += 1;
                    let mut s = String::new();
                    loop {
                        match self.peek_char() {
                            Some('\'') => {
                                self.pos += 1;
                                break;
                            }
                            Some(c) => {
                                s.push(c);
                                self.pos += 1;
                            }
                            None => return incomplete("unterminated single quote"),
                        }
                    }
                    parts.push(WordPart::Quoted(s));
                }
                '"' => {
                    flush(&mut lit, &mut parts);
                    self.pos += 1;
                    let inner = self.read_double_quoted()?;
                    parts.push(WordPart::DoubleQuoted(inner));
                }
                '$' => {
                    flush(&mut lit, &mut parts);
                    match self.read_dollar()? {
                        Some(p) => parts.push(p),
                        None => lit.push('$'),
                    }
                }
                '`' => {
                    flush(&mut lit, &mut parts);
                    parts.push(self.read_backquote()?);
                }
                c => {
                    lit.push(c);
                    self.pos += 1;
                }
            }
        }
        flush(&mut lit, &mut parts);
        if parts.is_empty() && self.pos == word_start && !in_braces {
            return err(format!("unexpected character {:?}", self.peek_char().unwrap_or(' ')));
        }
        Ok(Word(parts))
    }

    fn read_double_quoted(&mut self) -> Result<Vec<WordPart>> {
        let mut parts = Vec::new();
        let mut lit = String::new();
        loop {
            let Some(c) = self.peek_char() else {
                return incomplete("unterminated double quote");
            };
            match c {
                '"' => {
                    self.pos += 1;
                    break;
                }
                '\\' => match self.peek_at(1) {
                    Some('\n') => self.pos += 2,
                    Some(e @ ('$' | '`' | '"' | '\\')) => {
                        lit.push(e);
                        self.pos += 2;
                    }
                    Some(_) => {
                        lit.push('\\');
                        self.pos += 1;
                    }
                    None => return incomplete("unterminated double quote"),
                },
                '$' => {
                    if !lit.is_empty() {
                        parts.push(WordPart::Literal(std::mem::take(&mut lit)));
                    }
                    match self.read_dollar()? {
                        Some(p) => parts.push(p),
                        None => lit.push('$'),
                    }
                }
                '`' => {
                    if !lit.is_empty() {
                        parts.push(WordPart::Literal(std::mem::take(&mut lit)));
                    }
                    parts.push(self.read_backquote()?);
                }
                c => {
                    lit.push(c);
                    self.pos += 1;
                }
            }
        }
        if !lit.is_empty() {
            parts.push(WordPart::Literal(lit));
        }
        Ok(parts)
    }

    /// Parse after `$`. Returns None for a lone `$`.
    fn read_dollar(&mut self) -> Result<Option<WordPart>> {
        debug_assert_eq!(self.peek_char(), Some('$'));
        let Some(next) = self.peek_at(1) else {
            self.pos += 1;
            return Ok(None);
        };
        match next {
            '(' if self.peek_at(2) == Some('(') => {
                self.pos += 3;
                // Find the matching "))" at depth 0.
                let start = self.pos;
                let mut depth = 0i32;
                loop {
                    match self.peek_char() {
                        None => return incomplete("unterminated $(("),
                        Some('(') => depth += 1,
                        Some(')') if depth == 0 && self.peek_at(1) == Some(')') => break,
                        Some(')') => depth -= 1,
                        _ => {}
                    }
                    self.pos += 1;
                }
                let text: String = self.src[start..self.pos].iter().collect();
                self.pos += 2;
                let word = Parser::new(&text, None).read_arith_word()?;
                Ok(Some(WordPart::Arith(Box::new(word))))
            }
            '(' => {
                self.pos += 2;
                let saved_peek = self.peeked.take();
                let prog = self.parse_list(&[], true)?;
                self.skip_blanks_and_newlines();
                if self.peek_char() != Some(')') {
                    if self.peek_char().is_none() {
                        return incomplete("unterminated $(");
                    }
                    return err("expected ')' to close $(");
                }
                self.pos += 1;
                self.peeked = saved_peek;
                Ok(Some(WordPart::CommandSubst(Box::new(prog))))
            }
            '{' => {
                self.pos += 2;
                self.read_braced_param().map(Some)
            }
            c if c == '_' || c.is_ascii_alphabetic() => {
                self.pos += 1;
                let mut name = String::new();
                while let Some(c) = self.peek_char() {
                    if c == '_' || c.is_ascii_alphanumeric() {
                        name.push(c);
                        self.pos += 1;
                    } else {
                        break;
                    }
                }
                Ok(Some(WordPart::Param(Param { name, op: ParamOp::Plain })))
            }
            c if c.is_ascii_digit() || matches!(c, '?' | '$' | '!' | '#' | '@' | '*' | '-') => {
                self.pos += 2;
                Ok(Some(WordPart::Param(Param { name: c.to_string(), op: ParamOp::Plain })))
            }
            _ => {
                self.pos += 1;
                Ok(None)
            }
        }
    }

    fn read_braced_param(&mut self) -> Result<WordPart> {
        // ${#name} (length) unless it is ${#} or ${#-...} etc.
        let mut length = false;
        if self.peek_char() == Some('#') && !matches!(self.peek_at(1), Some('}') | None) {
            length = true;
            self.pos += 1;
        }
        let mut name = String::new();
        match self.peek_char() {
            Some(c) if c == '_' || c.is_ascii_alphabetic() => {
                while let Some(c) = self.peek_char() {
                    if c == '_' || c.is_ascii_alphanumeric() {
                        name.push(c);
                        self.pos += 1;
                    } else {
                        break;
                    }
                }
            }
            Some(c) if c.is_ascii_digit() => {
                while let Some(c) = self.peek_char() {
                    if c.is_ascii_digit() {
                        name.push(c);
                        self.pos += 1;
                    } else {
                        break;
                    }
                }
            }
            Some(c @ ('?' | '$' | '!' | '#' | '@' | '*' | '-')) => {
                name.push(c);
                self.pos += 1;
            }
            None => return incomplete("unterminated ${"),
            Some(c) => return err(format!("bad substitution near '{c}'")),
        }
        if length {
            if self.peek_char() != Some('}') {
                return err("bad substitution");
            }
            self.pos += 1;
            return Ok(WordPart::Param(Param { name, op: ParamOp::Length }));
        }
        let colon = self.peek_char() == Some(':');
        let op_char = if colon { self.peek_at(1) } else { self.peek_char() };
        let op = match op_char {
            Some('}') if !colon => {
                self.pos += 1;
                return Ok(WordPart::Param(Param { name, op: ParamOp::Plain }));
            }
            Some(c @ ('-' | '=' | '?' | '+')) => {
                self.pos += if colon { 2 } else { 1 };
                let word = self.read_word(true)?;
                match c {
                    '-' => ParamOp::Default { colon, word },
                    '=' => ParamOp::Assign { colon, word },
                    '?' => ParamOp::Error { colon, word },
                    _ => ParamOp::Alternative { colon, word },
                }
            }
            Some(c @ ('%' | '#')) if !colon => {
                self.pos += 1;
                let longest = self.peek_char() == Some(c);
                if longest {
                    self.pos += 1;
                }
                let pattern = self.read_word(true)?;
                if c == '%' {
                    ParamOp::RemoveSuffix { longest, pattern }
                } else {
                    ParamOp::RemovePrefix { longest, pattern }
                }
            }
            None => return incomplete("unterminated ${"),
            Some(c) => return err(format!("bad substitution near '{c}'")),
        };
        match self.peek_char() {
            Some('}') => self.pos += 1,
            None => return incomplete("unterminated ${"),
            Some(c) => return err(format!("bad substitution near '{c}'")),
        }
        Ok(WordPart::Param(Param { name, op }))
    }

    fn read_backquote(&mut self) -> Result<WordPart> {
        self.pos += 1;
        let mut inner = String::new();
        loop {
            match self.peek_char() {
                None => return incomplete("unterminated backquote"),
                Some('`') => {
                    self.pos += 1;
                    break;
                }
                Some('\\') if matches!(self.peek_at(1), Some('$' | '`' | '\\')) => {
                    inner.push(self.peek_at(1).unwrap());
                    self.pos += 2;
                }
                Some(c) => {
                    inner.push(c);
                    self.pos += 1;
                }
            }
        }
        let prog = Parser::new(&inner, self.aliases).parse_program()?;
        Ok(WordPart::CommandSubst(Box::new(prog)))
    }

    /// Arithmetic text: everything is literal except expansions and quotes.
    fn read_arith_word(mut self) -> Result<Word> {
        let mut parts = Vec::new();
        let mut lit = String::new();
        while let Some(c) = self.peek_char() {
            match c {
                '$' => {
                    if !lit.is_empty() {
                        parts.push(WordPart::Literal(std::mem::take(&mut lit)));
                    }
                    match self.read_dollar()? {
                        Some(p) => parts.push(p),
                        None => lit.push('$'),
                    }
                }
                '"' => {
                    self.pos += 1;
                    if !lit.is_empty() {
                        parts.push(WordPart::Literal(std::mem::take(&mut lit)));
                    }
                    let inner = self.read_double_quoted()?;
                    parts.push(WordPart::DoubleQuoted(inner));
                }
                '`' => {
                    if !lit.is_empty() {
                        parts.push(WordPart::Literal(std::mem::take(&mut lit)));
                    }
                    parts.push(self.read_backquote()?);
                }
                c => {
                    lit.push(c);
                    self.pos += 1;
                }
            }
        }
        if !lit.is_empty() {
            parts.push(WordPart::Literal(lit));
        }
        Ok(Word(parts))
    }

    // -- heredocs -------------------------------------------------------------------------

    fn read_heredoc_bodies(&mut self) -> Result<()> {
        let pending = std::mem::take(&mut self.pending_heredocs);
        for hd in pending {
            let mut body = String::new();
            loop {
                if self.pos >= self.src.len() {
                    self.pending_heredocs.push(hd);
                    return incomplete("here-document not terminated");
                }
                let start = self.pos;
                while self.pos < self.src.len() && self.src[self.pos] != '\n' {
                    self.pos += 1;
                }
                let mut line: String = self.src[start..self.pos].iter().collect();
                let had_newline = self.pos < self.src.len();
                if had_newline {
                    self.pos += 1;
                }
                if hd.strip_tabs {
                    line = line.trim_start_matches('\t').to_string();
                }
                if line == hd.delimiter {
                    break;
                }
                if !had_newline {
                    self.pending_heredocs.push(hd);
                    return incomplete("here-document not terminated");
                }
                body.push_str(&line);
                body.push('\n');
            }
            let word = if hd.quoted {
                Word(vec![WordPart::Quoted(body)])
            } else {
                // Only $, ` and \ are special inside an unquoted here-document.
                let mut p = Parser::new(&body, None);
                let mut parts = Vec::new();
                let mut lit = String::new();
                while let Some(c) = p.peek_char() {
                    match c {
                        '\\' if matches!(p.peek_at(1), Some('$' | '`' | '\\')) => {
                            lit.push(p.peek_at(1).unwrap());
                            p.pos += 2;
                        }
                        '\\' if p.peek_at(1) == Some('\n') => p.pos += 2,
                        '$' => {
                            if !lit.is_empty() {
                                parts.push(WordPart::Quoted(std::mem::take(&mut lit)));
                            }
                            match p.read_dollar()? {
                                Some(part) => parts.push(part),
                                None => lit.push('$'),
                            }
                        }
                        '`' => {
                            if !lit.is_empty() {
                                parts.push(WordPart::Quoted(std::mem::take(&mut lit)));
                            }
                            parts.push(p.read_backquote()?);
                        }
                        c => {
                            lit.push(c);
                            p.pos += 1;
                        }
                    }
                }
                if !lit.is_empty() {
                    parts.push(WordPart::Quoted(lit));
                }
                Word(vec![WordPart::DoubleQuoted(parts)])
            };
            self.heredoc_bodies[hd.id] = Some(word);
        }
        Ok(())
    }

    fn skip_blanks_and_newlines(&mut self) {
        loop {
            self.skip_blanks();
            if self.peek_char() == Some('\n') {
                self.pos += 1;
                if self.read_heredoc_bodies().is_err() {
                    return;
                }
            } else {
                break;
            }
        }
    }

    // -- grammar --------------------------------------------------------------------------

    fn is_reserved(tok: &Tok, word: &str) -> bool {
        matches!(tok, Tok::Word(w, _) if literal_text(w).as_deref() == Some(word))
    }

    fn skip_newlines(&mut self) -> Result<()> {
        while *self.peek_tok()? == Tok::Newline {
            self.next_tok()?;
        }
        Ok(())
    }

    /// Parse a list until EOF, `)` (when `in_subst` or inside a subshell), `;;`, or
    /// one of the reserved words in `stop` in command position.
    fn parse_list(&mut self, stop: &[&str], close_paren: bool) -> Result<Program> {
        let mut items = Vec::new();
        loop {
            self.skip_newlines()?;
            let tok = self.peek_tok()?.clone();
            match &tok {
                Tok::Eof => break,
                Tok::Op(")") if close_paren => break,
                Tok::Op(";;") => break,
                Tok::Op(op @ ("|" | "||" | "&&" | "&" | ";" | ")")) => {
                    return err(format!("syntax error near unexpected token '{op}'"));
                }
                t if stop.iter().any(|s| Self::is_reserved(t, s)) => break,
                _ => {}
            }
            self.drop_peek();
            let start = self.pos;
            self.skip_blanks();
            let text_start = self.pos.max(start);
            let and_or = self.parse_and_or()?;
            let text_end = self.pos;
            let mut background = false;
            match self.peek_tok()?.clone() {
                Tok::Op("&") => {
                    self.next_tok()?;
                    background = true;
                }
                Tok::Op(";") => {
                    self.next_tok()?;
                }
                Tok::Newline | Tok::Eof => {}
                Tok::Op(")") if close_paren => {}
                Tok::Op(";;") => {}
                t if stop.iter().any(|s| Self::is_reserved(&t, s)) => {}
                Tok::Op(op) => return err(format!("syntax error near unexpected token '{op}'")),
                Tok::Word(_, raw) => return err(format!("syntax error near unexpected word '{raw}'")),
                Tok::IoNumber(_) => return err("syntax error near redirection"),
            }
            let text: String = self.src[text_start.min(text_end)..text_end].iter().collect::<String>().trim().to_string();
            items.push(ListItem { and_or, background, text });
        }
        Ok(Program(items))
    }

    fn parse_and_or(&mut self) -> Result<AndOr> {
        let first = self.parse_pipeline()?;
        let mut rest = Vec::new();
        loop {
            let op = match self.peek_tok()? {
                Tok::Op("&&") => AndOrOp::And,
                Tok::Op("||") => AndOrOp::Or,
                _ => break,
            };
            self.next_tok()?;
            self.skip_newlines()?;
            if *self.peek_tok()? == Tok::Eof {
                return incomplete("expected a command after && or ||");
            }
            rest.push((op, self.parse_pipeline()?));
        }
        Ok(AndOr { first, rest })
    }

    fn parse_pipeline(&mut self) -> Result<Pipeline> {
        let mut negate = false;
        if Self::is_reserved(self.peek_tok()?, "!") {
            self.next_tok()?;
            negate = true;
        }
        let mut commands = vec![self.parse_command()?];
        while *self.peek_tok()? == Tok::Op("|") {
            self.next_tok()?;
            self.skip_newlines()?;
            if *self.peek_tok()? == Tok::Eof {
                return incomplete("expected a command after |");
            }
            commands.push(self.parse_command()?);
        }
        Ok(Pipeline { negate, commands })
    }

    fn expect_reserved(&mut self, word: &str) -> Result<()> {
        self.skip_newlines()?;
        let tok = self.next_tok()?;
        if Self::is_reserved(&tok, word) {
            Ok(())
        } else if tok == Tok::Eof {
            incomplete(format!("expected '{word}'"))
        } else {
            err(format!("syntax error: expected '{word}'"))
        }
    }

    /// Expand an alias at the current command position by splicing its text into
    /// the buffer. Returns true if something was expanded.
    fn try_alias(&mut self) -> Result<bool> {
        let Some(aliases) = self.aliases else { return Ok(false) };
        if self.pos >= self.alias_guard_end {
            self.alias_guard.clear();
        }
        let tok = self.peek_tok()?.clone();
        let Tok::Word(w, _) = &tok else { return Ok(false) };
        let Some(name) = literal_text(w) else { return Ok(false) };
        if self.alias_guard.contains(&name) || RESERVED.contains(&name.as_str()) {
            return Ok(false);
        }
        let Some(value) = aliases.get(&name) else { return Ok(false) };
        let (_, start, end) = self.peeked.take().unwrap();
        let replacement: Vec<char> = value.chars().collect();
        let delta = replacement.len() as isize - (end - start) as isize;
        self.src.splice(start..end, replacement);
        self.pos = start;
        self.alias_guard.insert(name);
        self.alias_guard_end = ((self.alias_guard_end.max(end)) as isize + delta).max(start as isize) as usize;
        Ok(true)
    }

    fn parse_command(&mut self) -> Result<Command> {
        let mut guard = 0;
        while self.try_alias()? {
            guard += 1;
            if guard > 32 {
                return err("alias expansion too deep");
            }
        }
        let tok = self.peek_tok()?.clone();
        let cmd = match &tok {
            Tok::Op("(") => {
                self.next_tok()?;
                let body = self.parse_list(&[], true)?;
                match self.next_tok()? {
                    Tok::Op(")") => {}
                    Tok::Eof => return incomplete("expected ')'"),
                    _ => return err("syntax error: expected ')'"),
                }
                Command::Subshell(body, Vec::new())
            }
            Tok::Eof => return incomplete("expected a command"),
            t if Self::is_reserved(t, "{") => {
                self.next_tok()?;
                let body = self.parse_list(&["}"], false)?;
                self.expect_reserved("}")?;
                Command::Group(body, Vec::new())
            }
            t if Self::is_reserved(t, "if") => {
                self.next_tok()?;
                let mut branches = Vec::new();
                let cond = self.parse_list(&["then"], false)?;
                self.expect_reserved("then")?;
                let body = self.parse_list(&["elif", "else", "fi"], false)?;
                branches.push((cond, body));
                let mut otherwise = None;
                loop {
                    self.skip_newlines()?;
                    let t = self.next_tok()?;
                    if Self::is_reserved(&t, "elif") {
                        let cond = self.parse_list(&["then"], false)?;
                        self.expect_reserved("then")?;
                        let body = self.parse_list(&["elif", "else", "fi"], false)?;
                        branches.push((cond, body));
                    } else if Self::is_reserved(&t, "else") {
                        otherwise = Some(self.parse_list(&["fi"], false)?);
                        self.expect_reserved("fi")?;
                        break;
                    } else if Self::is_reserved(&t, "fi") {
                        break;
                    } else if t == Tok::Eof {
                        return incomplete("expected 'fi'");
                    } else {
                        return err("syntax error: expected 'fi'");
                    }
                }
                Command::If { branches, otherwise, redirects: Vec::new() }
            }
            t if Self::is_reserved(t, "while") || Self::is_reserved(t, "until") => {
                let until = Self::is_reserved(t, "until");
                self.next_tok()?;
                let cond = self.parse_list(&["do"], false)?;
                self.expect_reserved("do")?;
                let body = self.parse_list(&["done"], false)?;
                self.expect_reserved("done")?;
                Command::While { until, cond, body, redirects: Vec::new() }
            }
            t if Self::is_reserved(t, "for") => {
                self.next_tok()?;
                let var = match self.next_tok()? {
                    Tok::Word(w, raw) if literal_text(&w).is_some_and(|n| is_name(&n)) => raw,
                    Tok::Eof => return incomplete("expected a variable name after 'for'"),
                    _ => return err("syntax error: bad for loop variable"),
                };
                // Optional "in words..." terminated by ; or newline.
                let mut items = None;
                self.skip_newlines()?;
                if Self::is_reserved(self.peek_tok()?, "in") {
                    self.next_tok()?;
                    let mut words = Vec::new();
                    loop {
                        match self.peek_tok()?.clone() {
                            Tok::Word(w, _) => {
                                self.next_tok()?;
                                words.push(w);
                            }
                            Tok::Op(";") | Tok::Newline => {
                                self.next_tok()?;
                                break;
                            }
                            Tok::Eof => return incomplete("expected 'do'"),
                            _ => return err("syntax error in for loop"),
                        }
                    }
                    items = Some(words);
                } else if *self.peek_tok()? == Tok::Op(";") {
                    self.next_tok()?;
                }
                self.expect_reserved("do")?;
                let body = self.parse_list(&["done"], false)?;
                self.expect_reserved("done")?;
                Command::For { var, items, body, redirects: Vec::new() }
            }
            t if Self::is_reserved(t, "case") => {
                self.next_tok()?;
                let word = match self.next_tok()? {
                    Tok::Word(w, _) => w,
                    Tok::Eof => return incomplete("expected a word after 'case'"),
                    _ => return err("syntax error after 'case'"),
                };
                self.expect_reserved("in")?;
                let mut items = Vec::new();
                loop {
                    self.skip_newlines()?;
                    let t = self.peek_tok()?.clone();
                    if Self::is_reserved(&t, "esac") {
                        self.next_tok()?;
                        break;
                    }
                    if t == Tok::Eof {
                        return incomplete("expected 'esac'");
                    }
                    if t == Tok::Op("(") {
                        self.next_tok()?;
                    }
                    let mut patterns = Vec::new();
                    loop {
                        match self.next_tok()? {
                            Tok::Word(w, _) => patterns.push(w),
                            Tok::Eof => return incomplete("expected a pattern"),
                            _ => return err("syntax error in case pattern"),
                        }
                        match self.next_tok()? {
                            Tok::Op("|") => continue,
                            Tok::Op(")") => break,
                            Tok::Eof => return incomplete("expected ')'"),
                            _ => return err("syntax error in case pattern"),
                        }
                    }
                    let body = self.parse_list(&["esac"], false)?;
                    items.push(CaseItem { patterns, body });
                    self.skip_newlines()?;
                    match self.peek_tok()?.clone() {
                        Tok::Op(";;") => {
                            self.next_tok()?;
                        }
                        t if Self::is_reserved(&t, "esac") => {}
                        Tok::Eof => return incomplete("expected ';;' or 'esac'"),
                        _ => return err("syntax error in case"),
                    }
                }
                Command::Case { word, items, redirects: Vec::new() }
            }
            Tok::Word(w, _) if ["then", "else", "elif", "fi", "do", "done", "esac", "}"].iter().any(|r| literal_text(w).as_deref() == Some(r)) => {
                return err(format!("syntax error near unexpected '{}'", literal_text(w).unwrap_or_default()));
            }
            Tok::Word(w, raw) => {
                // Function definition: name ( ) compound-command
                if literal_text(w).is_some_and(|n| is_name(&n)) {
                    let save_pos = self.pos;
                    let save_peek = self.peeked.clone();
                    self.next_tok()?;
                    if *self.peek_tok()? == Tok::Op("(") {
                        self.next_tok()?;
                        if *self.peek_tok()? == Tok::Op(")") {
                            self.next_tok()?;
                            self.skip_newlines()?;
                            let body = self.parse_command()?;
                            if matches!(body, Command::Simple(_) | Command::Function { .. }) {
                                return err("syntax error: function body must be a compound command");
                            }
                            return Ok(Command::Function { name: raw.clone(), body: Box::new(body) });
                        }
                        return err("syntax error: expected ')' in function definition");
                    }
                    self.pos = save_pos;
                    self.peeked = save_peek;
                }
                return self.parse_simple().map(Command::Simple);
            }
            Tok::IoNumber(_) | Tok::Op("<" | ">" | ">>" | "<&" | ">&" | "<>" | ">|" | "<<" | "<<-" | "&>" | "&>>") => {
                return self.parse_simple().map(Command::Simple);
            }
            Tok::Op(op) => return err(format!("syntax error near unexpected token '{op}'")),
            Tok::Newline => return err("syntax error: unexpected newline"),
        };
        // Redirections after a compound command.
        let mut redirs = Vec::new();
        while let Some(r) = self.try_redirect()? {
            redirs.push(r);
        }
        Ok(match cmd {
            Command::Group(b, _) => Command::Group(b, redirs),
            Command::Subshell(b, _) => Command::Subshell(b, redirs),
            Command::If { branches, otherwise, .. } => Command::If { branches, otherwise, redirects: redirs },
            Command::While { until, cond, body, .. } => Command::While { until, cond, body, redirects: redirs },
            Command::For { var, items, body, .. } => Command::For { var, items, body, redirects: redirs },
            Command::Case { word, items, .. } => Command::Case { word, items, redirects: redirs },
            other => other,
        })
    }

    fn try_redirect(&mut self) -> Result<Option<Redirect>> {
        let tok = self.peek_tok()?.clone();
        let (fd, op) = match tok {
            Tok::IoNumber(n) => {
                self.next_tok()?;
                match self.next_tok()? {
                    Tok::Op(op) => (Some(n), op),
                    _ => return err("syntax error after file descriptor number"),
                }
            }
            Tok::Op(op @ ("<" | ">" | ">>" | "<&" | ">&" | "<>" | ">|" | "<<" | "<<-" | "&>" | "&>>")) => {
                self.next_tok()?;
                (None, op)
            }
            _ => return Ok(None),
        };
        let target = match self.next_tok()? {
            Tok::Word(w, raw) => (w, raw),
            Tok::Eof | Tok::Newline => return incomplete(format!("expected a word after '{op}'")),
            _ => return err(format!("syntax error near '{op}'")),
        };
        let kind = match op {
            "<" => RedirKind::In,
            ">" => RedirKind::Out,
            ">|" => RedirKind::Clobber,
            ">>" => RedirKind::Append,
            "<>" => RedirKind::ReadWrite,
            "<&" => RedirKind::DupIn,
            ">&" => RedirKind::DupOut,
            "&>" => RedirKind::OutErr,
            "&>>" => RedirKind::AppendOutErr,
            "<<" | "<<-" => {
                let (w, raw) = &target;
                let quoted = w.0.iter().any(|p| !matches!(p, WordPart::Literal(_)));
                let delimiter: String = raw.chars().filter(|c| !matches!(c, '\'' | '"' | '\\')).collect();
                let id = self.heredoc_bodies.len();
                self.heredoc_bodies.push(None);
                self.pending_heredocs.push(PendingHereDoc { id, delimiter, strip_tabs: op == "<<-", quoted });
                RedirKind::HereDoc { body: Word(vec![WordPart::Literal(format!("\u{0}heredoc{id}"))]), expand: !quoted }
            }
            _ => unreachable!(),
        };
        Ok(Some(Redirect { fd, kind, target: target.0 }))
    }

    fn parse_simple(&mut self) -> Result<SimpleCommand> {
        let mut cmd = SimpleCommand { assignments: Vec::new(), words: Vec::new(), redirects: Vec::new() };
        loop {
            if let Some(r) = self.try_redirect()? {
                cmd.redirects.push(r);
                continue;
            }
            match self.peek_tok()?.clone() {
                Tok::Word(w, raw) => {
                    // NAME=value before the command name is an assignment.
                    if cmd.words.is_empty() {
                        if let Some(eq) = raw.find('=') {
                            if is_name(&raw[..eq]) {
                                if let Some(WordPart::Literal(first)) = w.0.first() {
                                    if first.len() > eq {
                                        self.next_tok()?;
                                        let mut parts = w.0.clone();
                                        let rest = first[eq + 1..].to_string();
                                        if rest.is_empty() {
                                            parts.remove(0);
                                        } else {
                                            parts[0] = WordPart::Literal(rest);
                                        }
                                        cmd.assignments.push(Assignment { name: raw[..eq].to_string(), value: Word(parts) });
                                        continue;
                                    }
                                }
                            }
                        }
                    }
                    self.next_tok()?;
                    cmd.words.push(w);
                }
                _ => break,
            }
        }
        if cmd.words.is_empty() && cmd.assignments.is_empty() && cmd.redirects.is_empty() {
            return err("syntax error: expected a command");
        }
        Ok(cmd)
    }
}

// -- filling in here-document bodies ------------------------------------------------------

fn fill_redirects(rs: &mut [Redirect], bodies: &[Option<Word>]) {
    for r in rs {
        if let RedirKind::HereDoc { body, .. } = &mut r.kind {
            if let Some(WordPart::Literal(marker)) = body.0.first() {
                if let Some(id) = marker.strip_prefix("\u{0}heredoc").and_then(|s| s.parse::<usize>().ok()) {
                    if let Some(Some(w)) = bodies.get(id) {
                        *body = w.clone();
                    }
                }
            }
        }
    }
}

fn fill_heredocs_command(c: &mut Command, bodies: &[Option<Word>]) {
    match c {
        Command::Simple(s) => fill_redirects(&mut s.redirects, bodies),
        Command::Group(p, r) | Command::Subshell(p, r) => {
            fill_heredocs_program(p, bodies);
            fill_redirects(r, bodies);
        }
        Command::If { branches, otherwise, redirects } => {
            for (a, b) in branches {
                fill_heredocs_program(a, bodies);
                fill_heredocs_program(b, bodies);
            }
            if let Some(o) = otherwise {
                fill_heredocs_program(o, bodies);
            }
            fill_redirects(redirects, bodies);
        }
        Command::While { cond, body, redirects, .. } => {
            fill_heredocs_program(cond, bodies);
            fill_heredocs_program(body, bodies);
            fill_redirects(redirects, bodies);
        }
        Command::For { body, redirects, .. } => {
            fill_heredocs_program(body, bodies);
            fill_redirects(redirects, bodies);
        }
        Command::Case { items, redirects, .. } => {
            for i in items {
                fill_heredocs_program(&mut i.body, bodies);
            }
            fill_redirects(redirects, bodies);
        }
        Command::Function { body, .. } => fill_heredocs_command(body, bodies),
    }
}

fn fill_heredocs_program(p: &mut Program, bodies: &[Option<Word>]) {
    for item in &mut p.0 {
        let ao = &mut item.and_or;
        for c in &mut ao.first.commands {
            fill_heredocs_command(c, bodies);
        }
        for (_, pl) in &mut ao.rest {
            for c in &mut pl.commands {
                fill_heredocs_command(c, bodies);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lit(s: &str) -> WordPart {
        WordPart::Literal(s.into())
    }

    fn simple(p: &Program) -> &SimpleCommand {
        match &p.0[0].and_or.first.commands[0] {
            Command::Simple(s) => s,
            c => panic!("not simple: {c:?}"),
        }
    }

    #[test]
    fn words_and_quotes() {
        let p = parse(r#"echo 'a b' "c $x d" e\ f ${y:-z} $(echo hi) `pwd` $((1+2))"#).unwrap();
        let s = simple(&p);
        assert_eq!(s.words.len(), 8);
        assert_eq!(s.words[1].0, vec![WordPart::Quoted("a b".into())]);
        assert_eq!(
            s.words[2].0,
            vec![WordPart::DoubleQuoted(vec![
                lit("c "),
                WordPart::Param(Param { name: "x".into(), op: ParamOp::Plain }),
                lit(" d")
            ])]
        );
        assert_eq!(s.words[3].0, vec![lit("e"), WordPart::Quoted(" ".into()), lit("f")]);
        assert!(matches!(&s.words[4].0[0], WordPart::Param(Param { op: ParamOp::Default { colon: true, .. }, .. })));
        assert!(matches!(&s.words[5].0[0], WordPart::CommandSubst(_)));
        assert!(matches!(&s.words[6].0[0], WordPart::CommandSubst(_)));
        assert!(matches!(&s.words[7].0[0], WordPart::Arith(_)));
    }

    #[test]
    fn lists_pipelines_redirects() {
        let p = parse("a | b && c || d; e & f > out 2>&1 <in").unwrap();
        assert_eq!(p.0.len(), 3);
        assert_eq!(p.0[0].and_or.first.commands.len(), 2);
        assert_eq!(p.0[0].and_or.rest.len(), 2);
        assert!(p.0[1].background);
        assert_eq!(p.0[1].text, "e");
        let f = match &p.0[2].and_or.first.commands[0] {
            Command::Simple(s) => s,
            _ => panic!(),
        };
        assert_eq!(f.redirects.len(), 3);
        assert_eq!(f.redirects[1].fd, Some(2));
        assert_eq!(f.redirects[1].kind, RedirKind::DupOut);
    }

    #[test]
    fn compound_commands() {
        let src = "if true; then echo a; elif false; then echo b; else echo c; fi\n\
                   while x; do y; done\nuntil x; do y; done\nfor i in 1 2 3; do echo $i; done\n\
                   for j; do :; done\ncase $x in a|b) echo ab;; *) echo other;; esac\n\
                   f() { echo f; }\n(cd /; ls) > o\n{ a; b; } 2>/dev/null";
        let p = parse(src).unwrap();
        assert_eq!(p.0.len(), 9);
        match &p.0[0].and_or.first.commands[0] {
            Command::If { branches, otherwise, .. } => {
                assert_eq!(branches.len(), 2);
                assert!(otherwise.is_some());
            }
            c => panic!("{c:?}"),
        }
        assert!(matches!(&p.0[3].and_or.first.commands[0], Command::For { items: Some(v), .. } if v.len() == 3));
        assert!(matches!(&p.0[4].and_or.first.commands[0], Command::For { items: None, .. }));
        assert!(matches!(&p.0[5].and_or.first.commands[0], Command::Case { items, .. } if items.len() == 2));
        assert!(matches!(&p.0[6].and_or.first.commands[0], Command::Function { name, .. } if name == "f"));
        assert!(matches!(&p.0[7].and_or.first.commands[0], Command::Subshell(_, r) if r.len() == 1));
        assert!(matches!(&p.0[8].and_or.first.commands[0], Command::Group(_, r) if r.len() == 1));
    }

    #[test]
    fn assignments() {
        let p = parse("A=1 B= C=\"x y\" cmd D=2").unwrap();
        let s = simple(&p);
        assert_eq!(s.assignments.len(), 3);
        assert_eq!(s.assignments[1].value, Word(vec![]));
        assert_eq!(s.words.len(), 2);
    }

    #[test]
    fn heredocs() {
        let p = parse("cat <<EOF\nhello $x\nEOF\ncat <<'E'\n$y\nE\ncat <<-T\n\tz\n\tT\n").unwrap();
        assert_eq!(p.0.len(), 3);
        match &simple(&p).redirects[0].kind {
            RedirKind::HereDoc { body, expand } => {
                assert!(expand);
                assert!(matches!(&body.0[0], WordPart::DoubleQuoted(parts) if parts.len() == 3));
            }
            k => panic!("{k:?}"),
        }
        let second = match &p.0[1].and_or.first.commands[0] {
            Command::Simple(s) => s,
            _ => panic!(),
        };
        assert_eq!(second.redirects[0].kind, RedirKind::HereDoc { body: Word(vec![WordPart::Quoted("$y\n".into())]), expand: false });
    }

    #[test]
    fn incomplete_input() {
        for src in ["echo 'abc", "echo \"x", "if true; then", "a |", "a &&", "for i in 1 2", "cat <<EOF\nx", "f() {", "echo $(ls", "case x in", "echo \\"] {
            let e = parse(src).unwrap_err();
            assert!(e.incomplete, "{src:?} should be incomplete: {e:?}");
        }
        assert!(parse(">out echo hi").is_ok());
        for src in ["fi", "a ;; b", "echo )", "| a", "if; then fi"] {
            let e = parse(src).unwrap_err();
            assert!(!e.incomplete, "{src:?} should be an error: {e:?}");
        }
    }

    #[test]
    fn comments_and_continuations() {
        let p = parse("echo a # comment\necho b\\\nc").unwrap();
        assert_eq!(p.0.len(), 2);
        assert_eq!(simple(&p).words.len(), 2);
        let p = parse("echo a#b").unwrap();
        assert_eq!(simple(&p).words[1].0, vec![lit("a#b")]);
    }

    #[test]
    fn nested_substitution() {
        let p = parse("echo \"$(echo \"inner $(echo deep)\")\"").unwrap();
        assert_eq!(simple(&p).words.len(), 2);
    }

    #[test]
    fn aliases_expand() {
        let mut a = HashMap::new();
        a.insert("ll".to_string(), "ls -l".to_string());
        a.insert("loop".to_string(), "loop2".to_string());
        a.insert("loop2".to_string(), "loop".to_string());
        let p = parse_with_aliases("ll /tmp; echo ll", &a).unwrap();
        assert_eq!(simple(&p).words.len(), 3);
        assert!(parse_with_aliases("loop", &a).is_ok());
    }
}
