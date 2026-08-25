//! GLYPH lexer.
//!
//! Tokens carry their line and column because the primary author of GLYPH is a
//! language model, and a model that gets told *where* it went wrong corrects
//! itself in one round instead of three.

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    /// A bare word: `flow`, `fs.list`, `true`, a variable name.
    Ident(String),
    /// A number, already scaled by any unit suffix, with the suffix retained
    /// so the checker can report `1GB` rather than `1073741824`.
    Num { value: f64, unit: Option<String> },
    /// A quoted string, split into literal and `${...}` interpolation pieces.
    Str(Vec<Piece>),
    /// A path literal: `~/Downloads`, `/etc/nous`, `./out.mp4`.
    Path(String),
    Sym(Sym),
    Eof,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Piece {
    Lit(String),
    /// A dotted reference, e.g. `plan.count`.
    Ref(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sym {
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Colon,
    Comma,
    Assign,
    Arrow,
    Gt,
    Lt,
    Ge,
    Le,
    Eq,
    Ne,
}

impl Sym {
    pub fn as_str(&self) -> &'static str {
        match self {
            Sym::LBrace => "{",
            Sym::RBrace => "}",
            Sym::LBracket => "[",
            Sym::RBracket => "]",
            Sym::Colon => ":",
            Sym::Comma => ",",
            Sym::Assign => "=",
            Sym::Arrow => "->",
            Sym::Gt => ">",
            Sym::Lt => "<",
            Sym::Ge => ">=",
            Sym::Le => "<=",
            Sym::Eq => "==",
            Sym::Ne => "!=",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub tok: Tok,
    pub line: usize,
    pub col: usize,
}

impl fmt::Display for Tok {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Tok::Ident(s) => write!(f, "`{}`", s),
            Tok::Num { value, unit } => match unit {
                Some(u) => write!(f, "{}{}", value, u),
                None => write!(f, "{}", value),
            },
            Tok::Str(_) => f.write_str("a string"),
            Tok::Path(p) => write!(f, "path {}", p),
            Tok::Sym(s) => write!(f, "`{}`", s.as_str()),
            Tok::Eof => f.write_str("end of input"),
        }
    }
}

/// Byte/time suffixes. GLYPH is written by people describing real machines, so
/// `1.5GB` and `30s` are worth having as literals.
fn unit_scale(unit: &str) -> Option<f64> {
    Some(match unit {
        "b" | "B" => 1.0,
        "kb" | "KB" => 1024.0,
        "mb" | "MB" => 1024.0 * 1024.0,
        "gb" | "GB" => 1024.0 * 1024.0 * 1024.0,
        "tb" | "TB" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        "s" => 1.0,
        "m" => 60.0,
        "h" => 3600.0,
        "d" => 86_400.0,
        "pct" | "%" => 1.0,
        _ => return None,
    })
}

pub struct Lexer<'a> {
    src: &'a [u8],
    i: usize,
    line: usize,
    col: usize,
}

pub fn lex(src: &str) -> Result<Vec<Token>, String> {
    Lexer { src: src.as_bytes(), i: 0, line: 1, col: 1 }.run()
}

impl<'a> Lexer<'a> {
    fn peek(&self) -> Option<u8> {
        self.src.get(self.i).copied()
    }
    fn peek_at(&self, n: usize) -> Option<u8> {
        self.src.get(self.i + n).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let c = self.peek()?;
        self.i += 1;
        if c == b'\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(c)
    }

    fn err(&self, msg: &str) -> String {
        format!("line {}, column {}: {}", self.line, self.col, msg)
    }

    fn run(mut self) -> Result<Vec<Token>, String> {
        let mut out = Vec::new();
        loop {
            self.skip_trivia();
            let (line, col) = (self.line, self.col);
            let c = match self.peek() {
                None => {
                    out.push(Token { tok: Tok::Eof, line, col });
                    return Ok(out);
                }
                Some(c) => c,
            };

            let tok = match c {
                b'{' => {
                    self.bump();
                    Tok::Sym(Sym::LBrace)
                }
                b'}' => {
                    self.bump();
                    Tok::Sym(Sym::RBrace)
                }
                b'[' => {
                    self.bump();
                    Tok::Sym(Sym::LBracket)
                }
                b']' => {
                    self.bump();
                    Tok::Sym(Sym::RBracket)
                }
                b':' => {
                    self.bump();
                    Tok::Sym(Sym::Colon)
                }
                b',' => {
                    self.bump();
                    Tok::Sym(Sym::Comma)
                }
                b'=' => {
                    self.bump();
                    if self.peek() == Some(b'=') {
                        self.bump();
                        Tok::Sym(Sym::Eq)
                    } else {
                        Tok::Sym(Sym::Assign)
                    }
                }
                b'!' if self.peek_at(1) == Some(b'=') => {
                    self.bump();
                    self.bump();
                    Tok::Sym(Sym::Ne)
                }
                b'>' => {
                    self.bump();
                    if self.peek() == Some(b'=') {
                        self.bump();
                        Tok::Sym(Sym::Ge)
                    } else {
                        Tok::Sym(Sym::Gt)
                    }
                }
                b'<' => {
                    self.bump();
                    if self.peek() == Some(b'=') {
                        self.bump();
                        Tok::Sym(Sym::Le)
                    } else {
                        Tok::Sym(Sym::Lt)
                    }
                }
                b'-' if self.peek_at(1) == Some(b'>') => {
                    self.bump();
                    self.bump();
                    Tok::Sym(Sym::Arrow)
                }
                b'"' | b'\'' => self.string(c)?,
                b'~' | b'/' | b'.' => self.path()?,
                c if c.is_ascii_digit() || (c == b'-' && self.peek_at(1).is_some_and(|d| d.is_ascii_digit())) => {
                    self.number()?
                }
                b'-' if self.peek_at(1).is_some_and(|d| is_ident_start(d) || d == b'-') => {
                    self.flag()
                }
                c if is_ident_start(c) => self.ident(),
                other => return Err(self.err(&format!("unexpected character '{}'", other as char))),
            };
            out.push(Token { tok, line, col });
        }
    }

    fn skip_trivia(&mut self) {
        loop {
            match self.peek() {
                Some(c) if c.is_ascii_whitespace() => {
                    self.bump();
                }
                Some(b'#') => {
                    while let Some(c) = self.peek() {
                        if c == b'\n' {
                            break;
                        }
                        self.bump();
                    }
                }
                _ => return,
            }
        }
    }

    fn ident(&mut self) -> Tok {
        let start = self.i;
        while let Some(c) = self.peek() {
            if is_ident_continue(c) {
                self.bump();
            } else {
                break;
            }
        }
        Tok::Ident(String::from_utf8_lossy(&self.src[start..self.i]).to_string())
    }

    /// A command-line style flag: `-i`, `--preset`. Lexed as a word so it can
    /// be passed straight through to a foreign tool.
    fn flag(&mut self) -> Tok {
        let start = self.i;
        while self.peek() == Some(b'-') {
            self.bump();
        }
        while let Some(c) = self.peek() {
            if is_ident_continue(c) {
                self.bump();
            } else {
                break;
            }
        }
        Tok::Ident(String::from_utf8_lossy(&self.src[start..self.i]).to_string())
    }

    fn number(&mut self) -> Result<Tok, String> {
        let start = self.i;
        if self.peek() == Some(b'-') {
            self.bump();
        }
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() || c == b'.' {
                // A trailing dot belongs to a path or a member access, not the
                // number: `1.` is not valid GLYPH.
                if c == b'.' && !self.peek_at(1).is_some_and(|d| d.is_ascii_digit()) {
                    break;
                }
                self.bump();
            } else {
                break;
            }
        }
        let text = String::from_utf8_lossy(&self.src[start..self.i]).to_string();
        let value: f64 = text.parse().map_err(|_| self.err(&format!("bad number '{}'", text)))?;

        let unit_start = self.i;
        if self.peek() == Some(b'%') {
            self.bump();
        } else {
            while let Some(c) = self.peek() {
                if c.is_ascii_alphabetic() {
                    self.bump();
                } else {
                    break;
                }
            }
        }
        if unit_start == self.i {
            return Ok(Tok::Num { value, unit: None });
        }
        let unit = String::from_utf8_lossy(&self.src[unit_start..self.i]).to_string();
        let scale = unit_scale(&unit)
            .ok_or_else(|| self.err(&format!("unknown unit '{}' (try KB, MB, GB, s, m, h, d)", unit)))?;
        Ok(Tok::Num { value: value * scale, unit: Some(unit) })
    }

    fn path(&mut self) -> Result<Tok, String> {
        let start = self.i;
        while let Some(c) = self.peek() {
            if c.is_ascii_whitespace() || matches!(c, b',' | b']' | b'}' | b')') {
                break;
            }
            self.bump();
        }
        let raw = String::from_utf8_lossy(&self.src[start..self.i]).to_string();
        if raw == "." || raw == ".." {
            return Err(self.err("a bare '.' is not a path"));
        }
        Ok(Tok::Path(raw))
    }

    fn string(&mut self, quote: u8) -> Result<Tok, String> {
        self.bump(); // opening quote
        let mut pieces = Vec::new();
        let mut lit = String::new();
        loop {
            let c = self.bump().ok_or_else(|| self.err("unterminated string"))?;
            if c == quote {
                break;
            }
            match c {
                b'\\' => {
                    let e = self.bump().ok_or_else(|| self.err("unterminated escape"))?;
                    lit.push(match e {
                        b'n' => '\n',
                        b't' => '\t',
                        b'r' => '\r',
                        b'\\' => '\\',
                        b'$' => '$',
                        other => other as char,
                    });
                }
                // `${ref}` interpolation.
                b'$' if self.peek() == Some(b'{') => {
                    self.bump();
                    if !lit.is_empty() {
                        pieces.push(Piece::Lit(std::mem::take(&mut lit)));
                    }
                    let mut name = String::new();
                    loop {
                        let n = self.bump().ok_or_else(|| self.err("unterminated ${...}"))?;
                        if n == b'}' {
                            break;
                        }
                        name.push(n as char);
                    }
                    let name = name.trim().to_string();
                    if name.is_empty() {
                        return Err(self.err("empty ${} interpolation"));
                    }
                    pieces.push(Piece::Ref(name));
                }
                // Multi-byte UTF-8 passes through intact.
                c if c < 0x80 => lit.push(c as char),
                c => {
                    let extra = if c >= 0xF0 {
                        3
                    } else if c >= 0xE0 {
                        2
                    } else {
                        1
                    };
                    let s = self.i - 1;
                    let e = (s + 1 + extra).min(self.src.len());
                    match std::str::from_utf8(&self.src[s..e]) {
                        Ok(txt) => {
                            lit.push_str(txt);
                            for _ in 0..extra {
                                self.bump();
                            }
                        }
                        Err(_) => return Err(self.err("invalid UTF-8 in string")),
                    }
                }
            }
        }
        if !lit.is_empty() || pieces.is_empty() {
            pieces.push(Piece::Lit(lit));
        }
        Ok(Tok::Str(pieces))
    }
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}

fn is_ident_continue(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'-' || c == b'.'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(src: &str) -> Vec<Tok> {
        lex(src).unwrap().into_iter().map(|t| t.tok).collect()
    }

    #[test]
    fn lexes_a_capability_call() {
        let t = toks("files = fs.list path: ~/Downloads");
        assert_eq!(t[0], Tok::Ident("files".into()));
        assert_eq!(t[1], Tok::Sym(Sym::Assign));
        assert_eq!(t[2], Tok::Ident("fs.list".into()));
        assert_eq!(t[3], Tok::Ident("path".into()));
        assert_eq!(t[4], Tok::Sym(Sym::Colon));
        assert_eq!(t[5], Tok::Path("~/Downloads".into()));
    }

    #[test]
    fn scales_unit_suffixes() {
        assert_eq!(toks("1GB")[0], Tok::Num { value: 1073741824.0, unit: Some("GB".into()) });
        assert_eq!(toks("30s")[0], Tok::Num { value: 30.0, unit: Some("s".into()) });
        assert_eq!(toks("1.5m")[0], Tok::Num { value: 90.0, unit: Some("m".into()) });
        assert_eq!(toks("42")[0], Tok::Num { value: 42.0, unit: None });
    }

    #[test]
    fn rejects_unknown_units_rather_than_ignoring_them() {
        let err = lex("5parsecs").unwrap_err();
        assert!(err.contains("unknown unit"), "{err}");
        // With a space it is simply a number followed by a word.
        assert_eq!(toks("5 parsecs")[0], Tok::Num { value: 5.0, unit: None });
    }

    #[test]
    fn splits_string_interpolation_into_pieces() {
        let t = toks(r#""move ${plan.count} files""#);
        match &t[0] {
            Tok::Str(p) => {
                assert_eq!(p[0], Piece::Lit("move ".into()));
                assert_eq!(p[1], Piece::Ref("plan.count".into()));
                assert_eq!(p[2], Piece::Lit(" files".into()));
            }
            other => panic!("expected a string, got {other:?}"),
        }
    }

    #[test]
    fn handles_escapes_and_unicode_in_strings() {
        let t = toks(r#""a\nb \${literal} café""#);
        match &t[0] {
            Tok::Str(p) => assert_eq!(p[0], Piece::Lit("a\nb ${literal} café".into())),
            other => panic!("expected a string, got {other:?}"),
        }
    }

    #[test]
    fn comparison_operators_lex_as_single_tokens() {
        let t = toks(">= <= == != > <");
        assert_eq!(
            t[..6],
            [
                Tok::Sym(Sym::Ge),
                Tok::Sym(Sym::Le),
                Tok::Sym(Sym::Eq),
                Tok::Sym(Sym::Ne),
                Tok::Sym(Sym::Gt),
                Tok::Sym(Sym::Lt),
            ]
        );
    }

    #[test]
    fn comments_run_to_end_of_line() {
        let t = toks("# a note\nfs.list # trailing\npath: /tmp");
        assert_eq!(t[0], Tok::Ident("fs.list".into()));
        assert_eq!(t[1], Tok::Ident("path".into()));
    }

    #[test]
    fn errors_name_the_line_and_column() {
        let err = lex("flow a {\n  bad ^ token\n}").unwrap_err();
        assert!(err.starts_with("line 2"), "{err}");
    }

    #[test]
    fn unterminated_constructs_are_errors() {
        assert!(lex(r#""no closing quote"#).is_err());
        assert!(lex(r#""${unclosed"#).is_err());
    }

    #[test]
    fn paths_stop_at_list_and_block_delimiters() {
        let t = toks("[~/a, /b/c]");
        assert_eq!(t[1], Tok::Path("~/a".into()));
        assert_eq!(t[2], Tok::Sym(Sym::Comma));
        assert_eq!(t[3], Tok::Path("/b/c".into()));
        assert_eq!(t[4], Tok::Sym(Sym::RBracket));
    }

    #[test]
    fn command_line_flags_lex_as_words() {
        let t = toks("[-i, --preset, -vf]");
        assert_eq!(t[1], Tok::Ident("-i".into()));
        assert_eq!(t[3], Tok::Ident("--preset".into()));
        assert_eq!(t[5], Tok::Ident("-vf".into()));
        // Negative numbers are still numbers.
        assert_eq!(toks("-5")[0], Tok::Num { value: -5.0, unit: None });
    }

    #[test]
    fn dotted_identifiers_stay_whole() {
        assert_eq!(toks("curate.propose")[0], Tok::Ident("curate.propose".into()));
        assert_eq!(toks("plan.steps")[0], Tok::Ident("plan.steps".into()));
    }
}
