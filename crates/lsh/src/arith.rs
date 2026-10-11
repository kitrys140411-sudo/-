//! Shell arithmetic: `$(( expr ))` with C-like integer semantics (i64).

pub trait Vars {
    fn get(&self, name: &str) -> Option<String>;
    fn set(&mut self, name: &str, value: String) -> Result<(), String>;
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(i64),
    Name(String),
    Op(&'static str),
}

const OPS: &[&str] = &[
    "<<=", ">>=", "**", "++", "--", "<<", ">>", "<=", ">=", "==", "!=", "&&", "||", "+=", "-=", "*=", "/=", "%=", "&=",
    "^=", "|=", "+", "-", "*", "/", "%", "<", ">", "=", "!", "~", "&", "^", "|", "?", ":", "(", ")", ",",
];

fn parse_number(s: &str) -> Result<i64, String> {
    let bad = || format!("invalid number '{s}'");
    if let Some((base, digits)) = s.split_once('#') {
        let base: u32 = base.parse().map_err(|_| bad())?;
        if !(2..=36).contains(&base) {
            return Err(bad());
        }
        return i64::from_str_radix(digits, base).map_err(|_| bad());
    }
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        return i64::from_str_radix(hex, 16).map_err(|_| bad());
    }
    if s.len() > 1 && s.starts_with('0') {
        return i64::from_str_radix(&s[1..], 8).map_err(|_| bad());
    }
    s.parse().map_err(|_| bad())
}

fn tokenize(src: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '#' || chars[i] == '_') {
                i += 1;
            }
            let s: String = chars[start..i].iter().collect();
            out.push(Tok::Num(parse_number(&s)?));
        } else if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            out.push(Tok::Name(chars[start..i].iter().collect()));
        } else {
            let rest: String = chars[i..chars.len().min(i + 3)].iter().collect();
            let op = OPS.iter().find(|op| rest.starts_with(**op)).ok_or_else(|| format!("syntax error near '{c}'"))?;
            out.push(Tok::Op(op));
            i += op.len();
        }
    }
    Ok(out)
}

struct Eval<'a, V: Vars> {
    toks: Vec<Tok>,
    pos: usize,
    vars: &'a mut V,
    /// When false, the expression is parsed but has no side effects (short circuit).
    live: bool,
    depth: usize,
}

/// Evaluate an arithmetic expression. Empty input evaluates to 0.
pub fn eval<V: Vars>(src: &str, vars: &mut V) -> Result<i64, String> {
    eval_depth(src, vars, 0)
}

fn eval_depth<V: Vars>(src: &str, vars: &mut V, depth: usize) -> Result<i64, String> {
    if depth > 64 {
        return Err("expression recursion too deep".into());
    }
    let toks = tokenize(src)?;
    if toks.is_empty() {
        return Ok(0);
    }
    let mut e = Eval { toks, pos: 0, vars, live: true, depth };
    let v = e.comma()?;
    if e.pos < e.toks.len() {
        return Err(format!("syntax error: unexpected {:?}", e.toks[e.pos]));
    }
    Ok(v)
}

impl<V: Vars> Eval<'_, V> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn eat(&mut self, op: &str) -> bool {
        if matches!(self.peek(), Some(Tok::Op(o)) if *o == op) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn var_value(&mut self, name: &str) -> Result<i64, String> {
        match self.vars.get(name) {
            None => Ok(0),
            Some(s) if s.trim().is_empty() => Ok(0),
            Some(s) => eval_depth(&s, self.vars, self.depth + 1),
        }
    }

    fn assign(&mut self, name: &str, v: i64) -> Result<(), String> {
        if self.live { self.vars.set(name, v.to_string()) } else { Ok(()) }
    }

    fn comma(&mut self) -> Result<i64, String> {
        let mut v = self.assignment()?;
        while self.eat(",") {
            v = self.assignment()?;
        }
        Ok(v)
    }

    fn assignment(&mut self) -> Result<i64, String> {
        if let (Some(Tok::Name(name)), Some(Tok::Op(op))) = (self.toks.get(self.pos).cloned(), self.toks.get(self.pos + 1).cloned()) {
            let compound = match op {
                "=" => Some(None),
                "+=" => Some(Some("+")),
                "-=" => Some(Some("-")),
                "*=" => Some(Some("*")),
                "/=" => Some(Some("/")),
                "%=" => Some(Some("%")),
                "<<=" => Some(Some("<<")),
                ">>=" => Some(Some(">>")),
                "&=" => Some(Some("&")),
                "^=" => Some(Some("^")),
                "|=" => Some(Some("|")),
                _ => None,
            };
            if let Some(bin) = compound {
                self.pos += 2;
                let rhs = self.assignment()?;
                let v = match bin {
                    None => rhs,
                    Some(b) => {
                        let cur = self.var_value(&name)?;
                        binary(b, cur, rhs)?
                    }
                };
                self.assign(&name, v)?;
                return Ok(v);
            }
        }
        self.ternary()
    }

    fn ternary(&mut self) -> Result<i64, String> {
        let cond = self.binary(0)?;
        if !self.eat("?") {
            return Ok(cond);
        }
        let live = self.live;
        self.live = live && cond != 0;
        let a = self.comma()?;
        if !self.eat(":") {
            return Err("expected ':' in conditional expression".into());
        }
        self.live = live && cond == 0;
        let b = self.ternary()?;
        self.live = live;
        Ok(if cond != 0 { a } else { b })
    }

    fn binary(&mut self, min_prec: u8) -> Result<i64, String> {
        let mut lhs = self.unary()?;
        loop {
            let Some(Tok::Op(op)) = self.peek().cloned() else { break };
            let Some(prec) = precedence(op) else { break };
            if prec < min_prec {
                break;
            }
            self.pos += 1;
            if op == "&&" || op == "||" {
                let live = self.live;
                let short = if op == "&&" { lhs == 0 } else { lhs != 0 };
                self.live = live && !short;
                let rhs = self.binary(prec + 1)?;
                self.live = live;
                lhs = if op == "&&" { (lhs != 0 && rhs != 0) as i64 } else { (lhs != 0 || rhs != 0) as i64 };
                continue;
            }
            // ** is right associative.
            let next = if op == "**" { prec } else { prec + 1 };
            let rhs = self.binary(next)?;
            lhs = if self.live { binary(op, lhs, rhs)? } else { 0 };
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> Result<i64, String> {
        match self.peek().cloned() {
            Some(Tok::Op("-")) => {
                self.pos += 1;
                Ok(self.unary()?.wrapping_neg())
            }
            Some(Tok::Op("+")) => {
                self.pos += 1;
                self.unary()
            }
            Some(Tok::Op("!")) => {
                self.pos += 1;
                Ok((self.unary()? == 0) as i64)
            }
            Some(Tok::Op("~")) => {
                self.pos += 1;
                Ok(!self.unary()?)
            }
            Some(Tok::Op(op @ ("++" | "--"))) => {
                self.pos += 1;
                let Some(Tok::Name(name)) = self.peek().cloned() else {
                    return Err(format!("'{op}' needs a variable"));
                };
                self.pos += 1;
                let v = self.var_value(&name)? + if op == "++" { 1 } else { -1 };
                self.assign(&name, v)?;
                Ok(v)
            }
            _ => self.postfix(),
        }
    }

    fn postfix(&mut self) -> Result<i64, String> {
        match self.peek().cloned() {
            Some(Tok::Num(n)) => {
                self.pos += 1;
                Ok(n)
            }
            Some(Tok::Name(name)) => {
                self.pos += 1;
                let v = self.var_value(&name)?;
                if let Some(Tok::Op(op @ ("++" | "--"))) = self.peek().cloned() {
                    self.pos += 1;
                    self.assign(&name, v + if op == "++" { 1 } else { -1 })?;
                }
                Ok(v)
            }
            Some(Tok::Op("(")) => {
                self.pos += 1;
                let v = self.comma()?;
                if !self.eat(")") {
                    return Err("expected ')'".into());
                }
                Ok(v)
            }
            Some(t) => Err(format!("syntax error near {t:?}")),
            None => Err("syntax error: expression expected".into()),
        }
    }
}

fn precedence(op: &str) -> Option<u8> {
    Some(match op {
        "||" => 1,
        "&&" => 2,
        "|" => 3,
        "^" => 4,
        "&" => 5,
        "==" | "!=" => 6,
        "<" | "<=" | ">" | ">=" => 7,
        "<<" | ">>" => 8,
        "+" | "-" => 9,
        "*" | "/" | "%" => 10,
        "**" => 11,
        _ => return None,
    })
}

fn binary(op: &str, a: i64, b: i64) -> Result<i64, String> {
    Ok(match op {
        "+" => a.wrapping_add(b),
        "-" => a.wrapping_sub(b),
        "*" => a.wrapping_mul(b),
        "/" | "%" if b == 0 => return Err("division by zero".into()),
        "/" => a.wrapping_div(b),
        "%" => a.wrapping_rem(b),
        "**" => {
            if b < 0 {
                return Err("exponent less than 0".into());
            }
            a.wrapping_pow(b.min(u32::MAX as i64) as u32)
        }
        "<<" => a.wrapping_shl(b as u32),
        ">>" => a.wrapping_shr(b as u32),
        "&" => a & b,
        "^" => a ^ b,
        "|" => a | b,
        "<" => (a < b) as i64,
        "<=" => (a <= b) as i64,
        ">" => (a > b) as i64,
        ">=" => (a >= b) as i64,
        "==" => (a == b) as i64,
        "!=" => (a != b) as i64,
        _ => return Err(format!("unknown operator {op}")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    impl Vars for HashMap<String, String> {
        fn get(&self, name: &str) -> Option<String> {
            HashMap::get(self, name).cloned()
        }
        fn set(&mut self, name: &str, value: String) -> Result<(), String> {
            self.insert(name.to_string(), value);
            Ok(())
        }
    }

    fn ev(s: &str) -> i64 {
        eval(s, &mut HashMap::new()).unwrap()
    }

    #[test]
    fn precedence_and_operators() {
        assert_eq!(ev("1 + 2 * 3"), 7);
        assert_eq!(ev("(1 + 2) * 3"), 9);
        assert_eq!(ev("2 ** 3 ** 2"), 512);
        assert_eq!(ev("-2 + 5 % 3"), 0);
        assert_eq!(ev("7 / 2"), 3);
        assert_eq!(ev("1 < 2 && 3 > 4 || 1"), 1);
        assert_eq!(ev("0x1f + 010 + 2#101"), 31 + 8 + 5);
        assert_eq!(ev("1 ? 2 : 3"), 2);
        assert_eq!(ev("0 ? 2 : 0 ? 3 : 4"), 4);
        assert_eq!(ev("~0"), -1);
        assert_eq!(ev("!5"), 0);
        assert_eq!(ev("1 << 4 | 1"), 17);
        assert_eq!(ev(""), 0);
    }

    #[test]
    fn variables_and_assignment() {
        let mut v = HashMap::new();
        v.insert("x".to_string(), "5".to_string());
        v.insert("e".to_string(), "x * 2".to_string());
        assert_eq!(eval("x + 1", &mut v).unwrap(), 6);
        assert_eq!(eval("e + 1", &mut v).unwrap(), 11);
        assert_eq!(eval("y = x += 2", &mut v).unwrap(), 7);
        assert_eq!(v["y"], "7");
        assert_eq!(eval("x++ + ++x", &mut v).unwrap(), 7 + 9);
        assert_eq!(eval("unset_var + 1", &mut v).unwrap(), 1);
        // Short circuit has no side effects.
        assert_eq!(eval("0 && (z = 1)", &mut v).unwrap(), 0);
        assert!(!v.contains_key("z"));
        assert_eq!(eval("1 ? (a = 1) : (b = 2)", &mut v).unwrap(), 1);
        assert!(v.contains_key("a") && !v.contains_key("b"));
    }

    #[test]
    fn errors() {
        let mut v = HashMap::new();
        assert!(eval("1 / 0", &mut v).unwrap_err().contains("division by zero"));
        assert!(eval("1 +", &mut v).is_err());
        assert!(eval("(1", &mut v).is_err());
        assert!(eval("09", &mut v).is_err());
        v.insert("r".to_string(), "r".to_string());
        assert!(eval("r", &mut v).is_err());
    }
}
