//! Zero-dependency JSON value, parser and serializer.
//!
//! NOUS OS deliberately ships no third-party crates in its core: the daemon is a
//! system component that must build on an air-gapped machine and must not carry a
//! supply chain. This module is the price of that decision, and it is a small one.

use std::collections::BTreeMap;
use std::fmt::{self, Write as _};

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(BTreeMap<String, Json>),
}

impl Json {
    pub fn obj() -> Json {
        Json::Obj(BTreeMap::new())
    }

    /// Look up a key on an object. Returns `None` for non-objects rather than
    /// panicking, so callers can walk untrusted documents without pre-checking.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(m) => m.get(key),
            _ => None,
        }
    }

    /// Walk a dotted path, e.g. `msg.params.intent`.
    pub fn path(&self, dotted: &str) -> Option<&Json> {
        let mut cur = self;
        for seg in dotted.split('.') {
            cur = cur.get(seg)?;
        }
        Some(cur)
    }

    pub fn set(&mut self, key: &str, val: Json) -> &mut Self {
        if let Json::Obj(m) = self {
            m.insert(key.to_string(), val);
        }
        self
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Num(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        self.as_f64().map(|f| f as i64)
    }

    pub fn as_u64(&self) -> Option<u64> {
        self.as_f64()
            .and_then(|f| if f < 0.0 { None } else { Some(f as u64) })
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_arr(&self) -> Option<&Vec<Json>> {
        match self {
            Json::Arr(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_obj(&self) -> Option<&BTreeMap<String, Json>> {
        match self {
            Json::Obj(m) => Some(m),
            _ => None,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Json::Null)
    }

    /// Convenience accessors that fall back instead of erroring. System code
    /// reads a lot of optional fields; this keeps call sites flat.
    pub fn str_or<'a>(&'a self, key: &str, default: &'a str) -> &'a str {
        self.get(key).and_then(|v| v.as_str()).unwrap_or(default)
    }

    pub fn f64_or(&self, key: &str, default: f64) -> f64 {
        self.get(key).and_then(|v| v.as_f64()).unwrap_or(default)
    }

    pub fn bool_or(&self, key: &str, default: bool) -> bool {
        self.get(key).and_then(|v| v.as_bool()).unwrap_or(default)
    }

    pub fn arr_or_empty(&self, key: &str) -> Vec<Json> {
        self.get(key)
            .and_then(|v| v.as_arr())
            .cloned()
            .unwrap_or_default()
    }

    /// Collect an array of strings, skipping non-string members.
    pub fn str_list(&self, key: &str) -> Vec<String> {
        self.arr_or_empty(key)
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect()
    }

    pub fn to_string_pretty(&self) -> String {
        let mut out = String::new();
        write_pretty(self, 0, &mut out);
        out
    }
}

impl fmt::Display for Json {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_compact(self, f)
    }
}

// ---------------------------------------------------------------- constructors

impl From<bool> for Json {
    fn from(b: bool) -> Json {
        Json::Bool(b)
    }
}
impl From<f64> for Json {
    fn from(n: f64) -> Json {
        Json::Num(n)
    }
}
impl From<i64> for Json {
    fn from(n: i64) -> Json {
        Json::Num(n as f64)
    }
}
impl From<u64> for Json {
    fn from(n: u64) -> Json {
        Json::Num(n as f64)
    }
}
impl From<usize> for Json {
    fn from(n: usize) -> Json {
        Json::Num(n as f64)
    }
}
impl From<&str> for Json {
    fn from(s: &str) -> Json {
        Json::Str(s.to_string())
    }
}
impl From<String> for Json {
    fn from(s: String) -> Json {
        Json::Str(s)
    }
}
impl<T: Into<Json>> From<Vec<T>> for Json {
    fn from(v: Vec<T>) -> Json {
        Json::Arr(v.into_iter().map(Into::into).collect())
    }
}
impl<T: Into<Json>> From<Option<T>> for Json {
    fn from(v: Option<T>) -> Json {
        match v {
            Some(x) => x.into(),
            None => Json::Null,
        }
    }
}

/// Build an object from pairs: `json_obj([("k", v.into())])`.
pub fn json_obj<const N: usize>(pairs: [(&str, Json); N]) -> Json {
    let mut m = BTreeMap::new();
    for (k, v) in pairs {
        m.insert(k.to_string(), v);
    }
    Json::Obj(m)
}

// ------------------------------------------------------------------ serializer

fn write_compact(v: &Json, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match v {
        Json::Null => f.write_str("null"),
        Json::Bool(true) => f.write_str("true"),
        Json::Bool(false) => f.write_str("false"),
        Json::Num(n) => f.write_str(&fmt_num(*n)),
        Json::Str(s) => f.write_str(&quote(s)),
        Json::Arr(a) => {
            f.write_char('[')?;
            for (i, item) in a.iter().enumerate() {
                if i > 0 {
                    f.write_char(',')?;
                }
                write_compact(item, f)?;
            }
            f.write_char(']')
        }
        Json::Obj(m) => {
            f.write_char('{')?;
            for (i, (k, val)) in m.iter().enumerate() {
                if i > 0 {
                    f.write_char(',')?;
                }
                f.write_str(&quote(k))?;
                f.write_char(':')?;
                write_compact(val, f)?;
            }
            f.write_char('}')
        }
    }
}

fn write_pretty(v: &Json, indent: usize, out: &mut String) {
    let pad = "  ".repeat(indent);
    let pad_in = "  ".repeat(indent + 1);
    match v {
        Json::Arr(a) if !a.is_empty() => {
            out.push_str("[\n");
            for (i, item) in a.iter().enumerate() {
                out.push_str(&pad_in);
                write_pretty(item, indent + 1, out);
                if i + 1 < a.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            out.push_str(&pad);
            out.push(']');
        }
        Json::Obj(m) if !m.is_empty() => {
            out.push_str("{\n");
            for (i, (k, val)) in m.iter().enumerate() {
                out.push_str(&pad_in);
                out.push_str(&quote(k));
                out.push_str(": ");
                write_pretty(val, indent + 1, out);
                if i + 1 < m.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            out.push_str(&pad);
            out.push('}');
        }
        other => {
            let _ = write!(out, "{}", other);
        }
    }
}

/// Render a number the way JSON expects: integers without a trailing `.0`, and
/// non-finite values as `null` (JSON has no NaN/Infinity).
fn fmt_num(n: f64) -> String {
    if !n.is_finite() {
        return "null".to_string();
    }
    if n.fract() == 0.0 && n.abs() < 9.0e15 {
        format!("{}", n as i64)
    } else {
        let mut s = format!("{}", n);
        if s.contains('e') && !s.contains('.') {
            // Rust prints 1e20; JSON accepts it, but keep it unambiguous.
            s = format!("{:?}", n);
        }
        s
    }
}

pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// --------------------------------------------------------------------- parser

pub struct Parser<'a> {
    b: &'a [u8],
    i: usize,
    depth: usize,
}

/// Guards against stack exhaustion from hostile input. IPC peers are local but
/// not necessarily trusted (an agent can be third-party code).
const MAX_DEPTH: usize = 128;

pub fn parse(s: &str) -> Result<Json, String> {
    let mut p = Parser {
        b: s.as_bytes(),
        i: 0,
        depth: 0,
    };
    p.ws();
    let v = p.value()?;
    p.ws();
    if p.i != p.b.len() {
        return Err(format!("trailing input at byte {}", p.i));
    }
    Ok(v)
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn ws(&mut self) {
        while let Some(c) = self.peek() {
            if c == b' ' || c == b'\t' || c == b'\n' || c == b'\r' {
                self.i += 1;
            } else {
                break;
            }
        }
    }

    fn eat(&mut self, c: u8) -> Result<(), String> {
        if self.peek() == Some(c) {
            self.i += 1;
            Ok(())
        } else {
            Err(format!("expected '{}' at byte {}", c as char, self.i))
        }
    }

    fn lit(&mut self, word: &str, val: Json) -> Result<Json, String> {
        if self.b[self.i..].starts_with(word.as_bytes()) {
            self.i += word.len();
            Ok(val)
        } else {
            Err(format!("bad literal at byte {}", self.i))
        }
    }

    fn value(&mut self) -> Result<Json, String> {
        self.ws();
        match self.peek() {
            None => Err("unexpected end of input".to_string()),
            Some(b'n') => self.lit("null", Json::Null),
            Some(b't') => self.lit("true", Json::Bool(true)),
            Some(b'f') => self.lit("false", Json::Bool(false)),
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b'[') => self.array(),
            Some(b'{') => self.object(),
            Some(_) => self.number(),
        }
    }

    fn enter(&mut self) -> Result<(), String> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err("maximum nesting depth exceeded".to_string());
        }
        Ok(())
    }

    fn array(&mut self) -> Result<Json, String> {
        self.enter()?;
        self.eat(b'[')?;
        let mut items = Vec::new();
        self.ws();
        if self.peek() == Some(b']') {
            self.i += 1;
            self.depth -= 1;
            return Ok(Json::Arr(items));
        }
        loop {
            items.push(self.value()?);
            self.ws();
            match self.peek() {
                Some(b',') => {
                    self.i += 1;
                }
                Some(b']') => {
                    self.i += 1;
                    break;
                }
                _ => return Err(format!("expected ',' or ']' at byte {}", self.i)),
            }
        }
        self.depth -= 1;
        Ok(Json::Arr(items))
    }

    fn object(&mut self) -> Result<Json, String> {
        self.enter()?;
        self.eat(b'{')?;
        let mut map = BTreeMap::new();
        self.ws();
        if self.peek() == Some(b'}') {
            self.i += 1;
            self.depth -= 1;
            return Ok(Json::Obj(map));
        }
        loop {
            self.ws();
            let k = self.string()?;
            self.ws();
            self.eat(b':')?;
            let v = self.value()?;
            map.insert(k, v);
            self.ws();
            match self.peek() {
                Some(b',') => {
                    self.i += 1;
                }
                Some(b'}') => {
                    self.i += 1;
                    break;
                }
                _ => return Err(format!("expected ',' or '}}' at byte {}", self.i)),
            }
        }
        self.depth -= 1;
        Ok(Json::Obj(map))
    }

    fn string(&mut self) -> Result<String, String> {
        self.eat(b'"')?;
        let mut out = String::new();
        loop {
            let c = self.peek().ok_or("unterminated string")?;
            self.i += 1;
            match c {
                b'"' => break,
                b'\\' => {
                    let e = self.peek().ok_or("unterminated escape")?;
                    self.i += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{08}'),
                        b'f' => out.push('\u{0c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => out.push(self.unicode_escape()?),
                        other => return Err(format!("bad escape '\\{}'", other as char)),
                    }
                }
                // Multi-byte UTF-8: copy the whole sequence through verbatim.
                c if c < 0x80 => out.push(c as char),
                c => {
                    let extra = if c >= 0xF0 {
                        3
                    } else if c >= 0xE0 {
                        2
                    } else {
                        1
                    };
                    let start = self.i - 1;
                    let end = (start + 1 + extra).min(self.b.len());
                    let slice = &self.b[start..end];
                    match std::str::from_utf8(slice) {
                        Ok(s) => {
                            out.push_str(s);
                            self.i = end;
                        }
                        Err(_) => return Err("invalid UTF-8 in string".to_string()),
                    }
                }
            }
        }
        Ok(out)
    }

    fn hex4(&mut self) -> Result<u32, String> {
        if self.i + 4 > self.b.len() {
            return Err("truncated \\u escape".to_string());
        }
        let s = std::str::from_utf8(&self.b[self.i..self.i + 4])
            .map_err(|_| "bad \\u escape".to_string())?;
        let n = u32::from_str_radix(s, 16).map_err(|_| "bad \\u escape".to_string())?;
        self.i += 4;
        Ok(n)
    }

    fn unicode_escape(&mut self) -> Result<char, String> {
        let hi = self.hex4()?;
        // Surrogate pair: 😀 style.
        if (0xD800..0xDC00).contains(&hi) {
            if self.peek() == Some(b'\\') && self.b.get(self.i + 1) == Some(&b'u') {
                self.i += 2;
                let lo = self.hex4()?;
                if (0xDC00..0xE000).contains(&lo) {
                    let cp = 0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00);
                    return char::from_u32(cp).ok_or_else(|| "bad surrogate pair".to_string());
                }
                return Err("unpaired high surrogate".to_string());
            }
            return Err("unpaired high surrogate".to_string());
        }
        char::from_u32(hi).ok_or_else(|| "invalid code point".to_string())
    }

    fn number(&mut self) -> Result<Json, String> {
        let start = self.i;
        if self.peek() == Some(b'-') {
            self.i += 1;
        }
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() || c == b'.' || c == b'e' || c == b'E' || c == b'+' || c == b'-' {
                self.i += 1;
            } else {
                break;
            }
        }
        if start == self.i {
            return Err(format!("expected value at byte {}", self.i));
        }
        let s = std::str::from_utf8(&self.b[start..self.i]).map_err(|_| "bad number")?;
        s.parse::<f64>()
            .map(Json::Num)
            .map_err(|_| format!("bad number '{}'", s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_scalars() {
        for src in ["null", "true", "false", "42", "-1.5", "\"hi\""] {
            let v = parse(src).unwrap();
            assert_eq!(parse(&v.to_string()).unwrap(), v, "{src}");
        }
    }

    #[test]
    fn integers_do_not_grow_decimal_tails() {
        assert_eq!(Json::Num(42.0).to_string(), "42");
        assert_eq!(Json::Num(-7.0).to_string(), "-7");
    }

    #[test]
    fn parses_nested_documents() {
        let v = parse(r#"{"a":[1,2,{"b":null}],"c":"x"}"#).unwrap();
        assert_eq!(v.path("a").unwrap().as_arr().unwrap().len(), 3);
        assert!(v.path("a").is_some());
        assert_eq!(v.str_or("c", ""), "x");
    }

    #[test]
    fn handles_escapes_and_unicode() {
        let v = parse(r#""line\nbreak é 😀""#).unwrap();
        assert_eq!(v.as_str().unwrap(), "line\nbreak é 😀");
        // and survives a round trip
        let again = parse(&v.to_string()).unwrap();
        assert_eq!(again, v);
    }

    #[test]
    fn rejects_malformed_input() {
        for bad in ["{", "[1,]", "{\"a\":}", "tru", "\"unterminated", "1 2"] {
            assert!(parse(bad).is_err(), "should reject: {bad}");
        }
    }

    #[test]
    fn rejects_deep_nesting() {
        let deep = format!("{}{}", "[".repeat(500), "]".repeat(500));
        assert!(parse(&deep).is_err());
    }

    #[test]
    fn dotted_path_walks_objects() {
        let v = parse(r#"{"m":{"p":{"intent":"open"}}}"#).unwrap();
        assert_eq!(v.path("m.p.intent").unwrap().as_str().unwrap(), "open");
        assert!(v.path("m.p.missing").is_none());
        assert!(v.path("m.p.intent.deeper").is_none());
    }
}
