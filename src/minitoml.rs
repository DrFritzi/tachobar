//! The small part of TOML that `config.toml` needs, so tachobar does not
//! depend on a full TOML crate.
//!
//! Supported: comments, `[table]` headers, bare keys, basic and literal
//! strings, booleans, integers, floats and (multi-line) arrays of those.
//! Anything else (dotted keys, inline tables, multi-line strings, dates) is
//! reported as unsupported rather than misread.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    Bool(bool),
    Int(i64),
    Float(f64),
    Array(Vec<Value>),
}

/// Keys of each table; top-level keys are in the table named `""`.
pub type Tables = BTreeMap<String, BTreeMap<String, Value>>;

struct Parser {
    c: Vec<char>,
    i: usize,
    line: usize,
}

type Res<T> = Result<T, String>;

pub fn parse(text: &str) -> Res<Tables> {
    let mut p = Parser {
        c: text.chars().collect(),
        i: 0,
        line: 1,
    };
    let mut tables = Tables::new();
    tables.insert(String::new(), BTreeMap::new());
    let mut current = String::new();
    loop {
        p.skip_blank();
        match p.peek() {
            None => return Ok(tables),
            Some('[') => {
                p.i += 1;
                if p.peek() == Some('[') {
                    return Err(p.err("arrays of tables are not supported"));
                }
                let name = p.bare_key()?;
                p.skip_spaces();
                if p.next() != Some(']') {
                    return Err(p.err("expected ']' after table name"));
                }
                if tables.contains_key(&name) {
                    return Err(p.err(&format!("table [{name}] defined twice")));
                }
                tables.insert(name.clone(), BTreeMap::new());
                current = name;
            }
            Some(_) => {
                let key = p.bare_key()?;
                p.skip_spaces();
                match p.next() {
                    Some('=') => {}
                    Some('.') => return Err(p.err("dotted keys are not supported")),
                    _ => return Err(p.err(&format!("expected '=' after {key}"))),
                }
                p.skip_spaces();
                let value = p.value()?;
                let table = tables.entry(current.clone()).or_default();
                if table.insert(key.clone(), value).is_some() {
                    return Err(p.err(&format!("{key} is set twice")));
                }
            }
        }
        p.end_of_line()?;
    }
}

impl Parser {
    fn err(&self, msg: &str) -> String {
        format!("line {}: {msg}", self.line)
    }

    fn peek(&self) -> Option<char> {
        self.c.get(self.i).copied()
    }

    fn next(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.i += 1;
        if c == '\n' {
            self.line += 1;
        }
        Some(c)
    }

    fn skip_spaces(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t')) {
            self.i += 1;
        }
    }

    fn skip_comment(&mut self) {
        if self.peek() == Some('#') {
            while !matches!(self.peek(), None | Some('\n')) {
                self.i += 1;
            }
        }
    }

    /// Whitespace, newlines and comments.
    fn skip_blank(&mut self) {
        loop {
            self.skip_spaces();
            self.skip_comment();
            if matches!(self.peek(), Some('\n' | '\r')) {
                self.next();
            } else {
                return;
            }
        }
    }

    fn end_of_line(&mut self) -> Res<()> {
        self.skip_spaces();
        self.skip_comment();
        match self.peek() {
            None => Ok(()),
            Some('\n' | '\r') => Ok(()),
            Some(_) => Err(self.err("unexpected text after value")),
        }
    }

    fn bare_key(&mut self) -> Res<String> {
        self.skip_spaces();
        let start = self.i;
        while matches!(self.peek(), Some(c) if c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            self.i += 1;
        }
        if start == self.i {
            return Err(match self.peek() {
                Some('"' | '\'') => self.err("quoted keys are not supported"),
                _ => self.err("expected a key"),
            });
        }
        Ok(self.c[start..self.i].iter().collect())
    }

    fn value(&mut self) -> Res<Value> {
        match self.peek() {
            Some('"') => self.basic_string().map(Value::Str),
            Some('\'') => self.literal_string().map(Value::Str),
            Some('[') => self.array(),
            Some('{') => Err(self.err("inline tables are not supported")),
            Some(_) => self.scalar(),
            None => Err(self.err("missing value")),
        }
    }

    fn scalar(&mut self) -> Res<Value> {
        let start = self.i;
        while matches!(self.peek(), Some(c) if !c.is_whitespace() && !matches!(c, ',' | ']' | '#'))
        {
            self.i += 1;
        }
        let word: String = self.c[start..self.i].iter().collect();
        match word.as_str() {
            "true" => return Ok(Value::Bool(true)),
            "false" => return Ok(Value::Bool(false)),
            _ => {}
        }
        let digits = word.replace('_', "");
        if let Ok(n) = digits.parse::<i64>() {
            return Ok(Value::Int(n));
        }
        if digits.bytes().all(|b| b"0123456789+-.eE".contains(&b)) {
            if let Ok(f) = digits.parse::<f64>() {
                return Ok(Value::Float(f));
            }
        }
        Err(self.err(&format!("cannot read value {word:?} (strings need quotes)")))
    }

    fn literal_string(&mut self) -> Res<String> {
        self.i += 1;
        if self.c[self.i..].starts_with(&['\'', '\'']) {
            return Err(self.err("multi-line strings are not supported"));
        }
        let start = self.i;
        while let Some(c) = self.peek() {
            if c == '\'' {
                let s = self.c[start..self.i].iter().collect();
                self.i += 1;
                return Ok(s);
            }
            if c == '\n' {
                break;
            }
            self.i += 1;
        }
        Err(self.err("unterminated string"))
    }

    fn basic_string(&mut self) -> Res<String> {
        self.i += 1;
        if self.c[self.i..].starts_with(&['"', '"']) {
            return Err(self.err("multi-line strings are not supported"));
        }
        let mut s = String::new();
        loop {
            match self.next() {
                None | Some('\n') => return Err(self.err("unterminated string")),
                Some('"') => return Ok(s),
                Some('\\') => {
                    let esc = self.next();
                    s.push(match esc {
                        Some('\\') => '\\',
                        Some('"') => '"',
                        Some('n') => '\n',
                        Some('t') => '\t',
                        Some('r') => '\r',
                        Some('b') => '\u{8}',
                        Some('f') => '\u{c}',
                        Some(u @ ('u' | 'U')) => {
                            let n = if u == 'u' { 4 } else { 8 };
                            let hex: String = (0..n).filter_map(|_| self.next()).collect();
                            u32::from_str_radix(&hex, 16)
                                .ok()
                                .and_then(char::from_u32)
                                .ok_or_else(|| self.err("invalid \\u escape"))?
                        }
                        _ => return Err(self.err("invalid escape in string")),
                    });
                }
                Some(c) => s.push(c),
            }
        }
    }

    fn array(&mut self) -> Res<Value> {
        self.i += 1;
        let mut items = Vec::new();
        loop {
            self.skip_blank();
            match self.peek() {
                Some(']') => {
                    self.i += 1;
                    return Ok(Value::Array(items));
                }
                None => return Err(self.err("unterminated array")),
                _ => {}
            }
            items.push(self.value()?);
            self.skip_blank();
            match self.peek() {
                Some(',') => self.i += 1,
                Some(']') => {}
                _ => return Err(self.err("expected ',' or ']' in array")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(text: &str) -> BTreeMap<String, Value> {
        parse(text).unwrap().remove("").unwrap()
    }

    #[test]
    fn scalars_and_comments() {
        let t = root(
            "# hi\na = \"x\\ty\\u00e9\" # c\nb = 'C:\\raw'\nc = true\nd = 1_000\ne = 2.5\nf = -3\n",
        );
        assert_eq!(t["a"], Value::Str("x\tyé".into()));
        assert_eq!(t["b"], Value::Str("C:\\raw".into()));
        assert_eq!(t["c"], Value::Bool(true));
        assert_eq!(t["d"], Value::Int(1000));
        assert_eq!(t["e"], Value::Float(2.5));
        assert_eq!(t["f"], Value::Int(-3));
    }

    #[test]
    fn arrays_span_lines() {
        let t = root("a = [\n  \"x\", # first\n  \"y\",\n]\nb = [1, 2.5]\nc = []\r\n");
        assert_eq!(
            t["a"],
            Value::Array(vec![Value::Str("x".into()), Value::Str("y".into())])
        );
        assert_eq!(t["b"], Value::Array(vec![Value::Int(1), Value::Float(2.5)]));
        assert_eq!(t["c"], Value::Array(vec![]));
    }

    #[test]
    fn tables() {
        let t = parse("x = 1\n[thresholds]\nstale_days = 7\n").unwrap();
        assert_eq!(t["thresholds"]["stale_days"], Value::Int(7));
        assert_eq!(t[""]["x"], Value::Int(1));
    }

    #[test]
    fn errors_carry_line_numbers() {
        for (text, want) in [
            ("a = 1\nb = \n", "line 2"),
            ("a = nope", "strings need quotes"),
            ("a = \"x", "unterminated"),
            ("a = 1\na = 2", "twice"),
            ("a.b = 1", "dotted"),
            ("a = { b = 1 }", "inline"),
            ("a = \"\"\"x\"\"\"", "multi-line"),
            ("[[a]]", "arrays of tables"),
            ("a = 1 2", "unexpected"),
            ("a = [1 2]", "expected ','"),
            ("[t]\n[t]", "twice"),
        ] {
            let e = parse(text).unwrap_err();
            assert!(e.contains(want), "{text:?} -> {e}");
        }
    }
}
