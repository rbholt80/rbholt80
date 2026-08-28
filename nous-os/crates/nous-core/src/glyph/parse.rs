//! GLYPH parser.

use super::ast::*;
use super::lex::{lex, Sym, Tok, Token};

pub fn parse(src: &str) -> Result<Program, String> {
    let tokens = lex(src)?;
    Parser { t: tokens, i: 0 }.program()
}

struct Parser {
    t: Vec<Token>,
    i: usize,
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.t[self.i.min(self.t.len() - 1)].tok
    }

    fn peek_at(&self, n: usize) -> &Tok {
        &self.t[(self.i + n).min(self.t.len() - 1)].tok
    }

    fn line(&self) -> usize {
        self.t[self.i.min(self.t.len() - 1)].line
    }

    fn bump(&mut self) -> Tok {
        let t = self.t[self.i.min(self.t.len() - 1)].tok.clone();
        if self.i < self.t.len() - 1 {
            self.i += 1;
        }
        t
    }

    fn err<T>(&self, msg: &str) -> Result<T, String> {
        Err(format!(
            "line {}: {} (found {})",
            self.line(),
            msg,
            self.peek()
        ))
    }

    fn expect_sym(&mut self, s: Sym) -> Result<(), String> {
        if *self.peek() == Tok::Sym(s) {
            self.bump();
            Ok(())
        } else {
            self.err(&format!("expected `{}`", s.as_str()))
        }
    }

    fn expect_ident(&mut self) -> Result<String, String> {
        match self.peek().clone() {
            Tok::Ident(s) => {
                self.bump();
                Ok(s)
            }
            _ => self.err("expected a name"),
        }
    }

    fn at_kw(&self, kw: &str) -> bool {
        matches!(self.peek(), Tok::Ident(s) if s == kw)
    }

    fn program(mut self) -> Result<Program, String> {
        let mut flows = Vec::new();
        loop {
            match self.peek() {
                Tok::Eof => break,
                Tok::Ident(s) if s == "flow" => flows.push(self.flow()?),
                _ => return self.err("expected `flow`"),
            }
        }
        if flows.is_empty() {
            return Err("this program declares no flows".to_string());
        }
        Ok(Program { flows })
    }

    fn flow(&mut self) -> Result<Flow, String> {
        let line = self.line();
        self.bump(); // `flow`
        let name = match self.peek().clone() {
            Tok::Ident(s) => {
                self.bump();
                s
            }
            Tok::Str(pieces) => {
                self.bump();
                super::render_literal(&pieces)
                    .ok_or_else(|| format!("line {}: a flow name cannot interpolate", line))?
            }
            _ => return self.err("expected a flow name"),
        };
        self.expect_sym(Sym::LBrace)?;

        let mut flow = Flow {
            name,
            meta: Default::default(),
            foreigns: Default::default(),
            stmts: Vec::new(),
            line,
        };
        while *self.peek() != Tok::Sym(Sym::RBrace) {
            if *self.peek() == Tok::Eof {
                return Err(format!(
                    "line {}: this flow is never closed with `}}`",
                    line
                ));
            }
            // `meta` and `use foreign` are flow-level declarations, not steps.
            if self.at_kw("meta") {
                self.bump();
                let key = self.expect_ident()?;
                let value = self.value()?;
                let rendered = match &value {
                    Value::Str(p) => super::render_literal(p).unwrap_or_else(|| value.render()),
                    other => other.render(),
                };
                flow.meta.insert(key, rendered);
                continue;
            }
            if self.at_kw("use") {
                let f = self.foreign()?;
                flow.foreigns.entry(f.name.clone()).or_default().push(f);
                continue;
            }
            let stmt = self.stmt()?;
            flow.stmts.push(stmt);
        }
        self.expect_sym(Sym::RBrace)?;
        Ok(flow)
    }

    fn foreign(&mut self) -> Result<Foreign, String> {
        let line = self.line();
        self.bump(); // `use`
        if !self.at_kw("foreign") {
            return self.err("expected `foreign` after `use`");
        }
        self.bump();
        let name = self.expect_ident()?;
        let args = self.arglist()?;
        let cmd = args
            .iter()
            .find(|(k, _)| k == "cmd")
            .and_then(|(_, v)| v.literal())
            .and_then(|j| j.as_str().map(String::from))
            .ok_or_else(|| {
                format!(
                    "line {}: `use foreign {}` needs a literal `cmd:`",
                    line, name
                )
            })?;
        let platforms = match args.iter().find(|(k, _)| k == "on").map(|(_, v)| v) {
            Some(Value::List(items)) => items.iter().map(|i| i.render()).collect(),
            Some(other) => vec![other.render()],
            None => Vec::new(),
        };
        Ok(Foreign {
            name,
            cmd,
            platforms,
            line,
        })
    }

    fn stmt(&mut self) -> Result<Stmt, String> {
        let line = self.line();
        if self.at_kw("gate") {
            self.bump();
            return Ok(Stmt::Gate {
                cond: self.cond()?,
                line,
            });
        }
        if self.at_kw("ask") {
            self.bump();
            return Ok(Stmt::Ask {
                prompt: self.value()?,
                line,
            });
        }
        if self.at_kw("on") {
            self.bump();
            let platform = self.expect_ident()?;
            self.expect_sym(Sym::LBrace)?;
            let mut body = Vec::new();
            while *self.peek() != Tok::Sym(Sym::RBrace) {
                if *self.peek() == Tok::Eof {
                    return Err(format!("line {}: this `on` block is never closed", line));
                }
                body.push(self.stmt()?);
            }
            self.expect_sym(Sym::RBrace)?;
            return Ok(Stmt::On {
                platform,
                body,
                line,
            });
        }

        // `name = call` or a bare call.
        let head = self.expect_ident()?;
        if *self.peek() == Tok::Sym(Sym::Assign) {
            self.bump();
            let call = self.call()?;
            return Ok(Stmt::Bind { name: head, call });
        }
        let args = self.arglist()?;
        Ok(Stmt::Effect(Call {
            target: head,
            args,
            line,
        }))
    }

    fn call(&mut self) -> Result<Call, String> {
        let line = self.line();
        let target = self.expect_ident()?;
        let args = self.arglist()?;
        Ok(Call { target, args, line })
    }

    /// Arguments are `name: value`, with commas optional. The list ends as soon
    /// as the next tokens are not `ident :`.
    fn arglist(&mut self) -> Result<Vec<(String, Value)>, String> {
        let mut out = Vec::new();
        loop {
            if *self.peek() == Tok::Sym(Sym::Comma) {
                self.bump();
                continue;
            }
            let is_arg =
                matches!(self.peek(), Tok::Ident(_)) && *self.peek_at(1) == Tok::Sym(Sym::Colon);
            if !is_arg {
                return Ok(out);
            }
            let key = self.expect_ident()?;
            self.expect_sym(Sym::Colon)?;
            let value = self.value()?;
            if out.iter().any(|(k, _): &(String, Value)| k == &key) {
                return Err(format!(
                    "line {}: argument `{}` is given twice",
                    self.line(),
                    key
                ));
            }
            out.push((key, value));
        }
    }

    fn value(&mut self) -> Result<Value, String> {
        match self.peek().clone() {
            Tok::Str(pieces) => {
                self.bump();
                Ok(Value::Str(pieces))
            }
            Tok::Num { value, .. } => {
                self.bump();
                Ok(Value::Num(value))
            }
            Tok::Path(p) => {
                self.bump();
                Ok(Value::Path(p))
            }
            Tok::Ident(s) => {
                self.bump();
                Ok(match s.as_str() {
                    "true" => Value::Bool(true),
                    "false" => Value::Bool(false),
                    // A dotted word in value position is always a reference;
                    // a bare word is resolved against bindings at check time.
                    _ if s.contains('.') => Value::Ref(s),
                    _ => Value::Word(s),
                })
            }
            Tok::Sym(Sym::LBracket) => {
                self.bump();
                let mut items = Vec::new();
                while *self.peek() != Tok::Sym(Sym::RBracket) {
                    if *self.peek() == Tok::Eof {
                        return self.err("this list is never closed with `]`");
                    }
                    if *self.peek() == Tok::Sym(Sym::Comma) {
                        self.bump();
                        continue;
                    }
                    items.push(self.value()?);
                }
                self.bump();
                Ok(Value::List(items))
            }
            _ => self.err("expected a value"),
        }
    }

    fn cond(&mut self) -> Result<Cond, String> {
        let left = self.value()?;
        let op = match self.peek() {
            Tok::Sym(Sym::Gt) => Some(CmpOp::Gt),
            Tok::Sym(Sym::Lt) => Some(CmpOp::Lt),
            Tok::Sym(Sym::Ge) => Some(CmpOp::Ge),
            Tok::Sym(Sym::Le) => Some(CmpOp::Le),
            Tok::Sym(Sym::Eq) => Some(CmpOp::Eq),
            Tok::Sym(Sym::Ne) => Some(CmpOp::Ne),
            _ => None,
        };
        match op {
            Some(o) => {
                self.bump();
                let right = self.value()?;
                Ok(Cond {
                    left,
                    op: Some(o),
                    right: Some(right),
                })
            }
            None => Ok(Cond {
                left,
                op: None,
                right: None,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_minimal_flow() {
        let p = parse("flow tidy { files = fs.list path: ~/Downloads }").unwrap();
        assert_eq!(p.flows.len(), 1);
        assert_eq!(p.flows[0].name, "tidy");
        match &p.flows[0].stmts[0] {
            Stmt::Bind { name, call } => {
                assert_eq!(name, "files");
                assert_eq!(call.target, "fs.list");
                assert_eq!(call.arg("path"), Some(&Value::Path("~/Downloads".into())));
            }
            other => panic!("expected a binding, got {other:?}"),
        }
    }

    #[test]
    fn commas_between_arguments_are_optional() {
        let with = parse("flow a { fs.write path: /tmp/x, content: \"hi\" }").unwrap();
        let without = parse("flow a { fs.write path: /tmp/x content: \"hi\" }").unwrap();
        assert_eq!(with.flows[0].stmts, without.flows[0].stmts);
    }

    #[test]
    fn parses_meta_and_description() {
        let p = parse(r#"flow t { meta description "Tidy the Downloads folder" }"#).unwrap();
        assert_eq!(p.flows[0].description(), "Tidy the Downloads folder");
        assert!(
            p.flows[0].stmts.is_empty(),
            "meta is a declaration, not a step"
        );
    }

    #[test]
    fn parses_gate_and_ask() {
        let p = parse(
            r#"flow t {
                 plan = curate.propose
                 gate plan.count > 0
                 ask "Move ${plan.count} files?"
                 curate.apply steps: plan.steps
               }"#,
        )
        .unwrap();
        let s = &p.flows[0].stmts;
        assert_eq!(s.len(), 4);
        match &s[1] {
            Stmt::Gate { cond, .. } => {
                assert_eq!(cond.op, Some(CmpOp::Gt));
                assert_eq!(cond.left, Value::Ref("plan.count".into()));
            }
            other => panic!("expected a gate, got {other:?}"),
        }
        assert!(matches!(s[2], Stmt::Ask { .. }));
    }

    #[test]
    fn parses_platform_blocks() {
        let p = parse(
            "flow install {
               on linux { pkg.install name: mpv }
               on macos { brew args: [install, mpv] }
             }",
        )
        .unwrap();
        match &p.flows[0].stmts[0] {
            Stmt::On { platform, body, .. } => {
                assert_eq!(platform, "linux");
                assert_eq!(body.len(), 1);
            }
            other => panic!("expected an `on` block, got {other:?}"),
        }
    }

    #[test]
    fn parses_foreign_tool_declarations() {
        let p = parse(
            r#"flow t {
                 use foreign handbrake cmd: "HandBrakeCLI" on: [linux, macos]
                 use foreign handbrake cmd: "HandBrakeCLI.exe" on: [windows]
                 handbrake args: [-i, ~/a.mkv]
               }"#,
        )
        .unwrap();
        let f = &p.flows[0];
        assert_eq!(f.foreigns["handbrake"].len(), 2);
        assert_eq!(
            f.foreign_for("handbrake", "windows").unwrap().cmd,
            "HandBrakeCLI.exe"
        );
        assert_eq!(
            f.foreign_for("handbrake", "linux").unwrap().cmd,
            "HandBrakeCLI"
        );
        assert!(f.foreign_for("handbrake", "plan9").is_none());
    }

    #[test]
    fn foreign_without_a_platform_is_the_fallback() {
        let p = parse(r#"flow t { use foreign ffmpeg cmd: "ffmpeg" }"#).unwrap();
        assert_eq!(
            p.flows[0].foreign_for("ffmpeg", "windows").unwrap().cmd,
            "ffmpeg"
        );
    }

    #[test]
    fn lists_and_units_parse_as_values() {
        let p =
            parse("flow t { curate.propose kinds: [duplicate, screenshots] min: 1GB }").unwrap();
        match &p.flows[0].stmts[0] {
            Stmt::Effect(c) => {
                assert_eq!(
                    c.arg("kinds"),
                    Some(&Value::List(vec![
                        Value::Word("duplicate".into()),
                        Value::Word("screenshots".into())
                    ]))
                );
                assert_eq!(c.arg("min"), Some(&Value::Num(1073741824.0)));
            }
            other => panic!("expected a call, got {other:?}"),
        }
    }

    #[test]
    fn scope_is_inferred_from_conventional_arguments() {
        let p = parse("flow t { fs.write path: /tmp/a content: \"x\" }").unwrap();
        match &p.flows[0].stmts[0] {
            Stmt::Effect(c) => assert_eq!(c.scope_value(), Some(&Value::Path("/tmp/a".into()))),
            other => panic!("expected a call, got {other:?}"),
        }
    }

    #[test]
    fn reports_unclosed_blocks_with_the_opening_line() {
        let err = parse("flow t {\n  fs.list path: /tmp\n").unwrap_err();
        assert!(err.contains("line 1"), "{err}");
        assert!(err.contains("never closed"), "{err}");
    }

    #[test]
    fn rejects_duplicate_arguments() {
        let err = parse("flow t { fs.list path: /a path: /b }").unwrap_err();
        assert!(err.contains("given twice"), "{err}");
    }

    #[test]
    fn rejects_a_program_with_no_flows() {
        assert!(parse("# just a comment\n").is_err());
        assert!(parse("fs.list path: /tmp").is_err());
    }

    #[test]
    fn literal_values_resolve_and_dynamic_ones_do_not() {
        assert_eq!(Value::Num(3.0).literal().unwrap().as_f64(), Some(3.0));
        assert!(Value::Ref("plan.steps".into()).literal().is_none());
        let interpolated = parse(r#"flow t { ask "n=${plan.count}" }"#).unwrap();
        match &interpolated.flows[0].stmts[0] {
            Stmt::Ask { prompt, .. } => {
                assert!(prompt.literal().is_none());
                assert_eq!(prompt.refs(), vec!["plan.count".to_string()]);
            }
            other => panic!("expected an ask, got {other:?}"),
        }
    }
}
