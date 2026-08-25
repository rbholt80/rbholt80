//! GLYPH syntax tree and values.

use super::lex::Piece;
use crate::json::{json_obj, Json};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// A string, possibly with `${ref}` interpolation.
    Str(Vec<Piece>),
    Num(f64),
    Bool(bool),
    Path(String),
    /// A bare word. Resolves to a reference if it names a binding, and to a
    /// literal symbol otherwise — so `kinds: [duplicate]` and `gate ok` can
    /// share a syntax without the author having to think about it.
    Word(String),
    /// A dotted reference into an earlier binding, e.g. `plan.steps`.
    Ref(String),
    List(Vec<Value>),
}

impl Value {
    /// The literal JSON for this value, where one exists without runtime
    /// bindings. `None` means the value depends on an earlier step's result.
    pub fn literal(&self) -> Option<Json> {
        match self {
            Value::Num(n) => Some(Json::Num(*n)),
            Value::Bool(b) => Some(Json::Bool(*b)),
            Value::Path(p) => Some(Json::Str(p.clone())),
            Value::Word(w) => Some(Json::Str(w.clone())),
            Value::Ref(_) => None,
            Value::Str(pieces) => {
                let mut out = String::new();
                for p in pieces {
                    match p {
                        Piece::Lit(s) => out.push_str(s),
                        Piece::Ref(_) => return None,
                    }
                }
                Some(Json::Str(out))
            }
            Value::List(items) => {
                let mut out = Vec::new();
                for i in items {
                    out.push(i.literal()?);
                }
                Some(Json::Arr(out))
            }
        }
    }

    /// Every binding name this value depends on.
    pub fn refs(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.collect_refs(&mut out);
        out
    }

    fn collect_refs(&self, out: &mut Vec<String>) {
        match self {
            Value::Ref(r) => out.push(r.clone()),
            Value::Word(w) => out.push(w.clone()),
            Value::Str(pieces) => {
                for p in pieces {
                    if let Piece::Ref(r) = p {
                        out.push(r.clone());
                    }
                }
            }
            Value::List(items) => {
                for i in items {
                    i.collect_refs(out);
                }
            }
            _ => {}
        }
    }

    /// Render for diagnostics and for the plan a user is shown.
    pub fn render(&self) -> String {
        match self {
            Value::Num(n) => {
                if n.fract() == 0.0 {
                    format!("{}", *n as i64)
                } else {
                    format!("{}", n)
                }
            }
            Value::Bool(b) => b.to_string(),
            Value::Path(p) => p.clone(),
            Value::Word(w) => w.clone(),
            Value::Ref(r) => r.clone(),
            Value::Str(pieces) => {
                let mut s = String::new();
                for p in pieces {
                    match p {
                        Piece::Lit(l) => s.push_str(l),
                        Piece::Ref(r) => s.push_str(&format!("${{{}}}", r)),
                    }
                }
                format!("\"{}\"", s)
            }
            Value::List(items) => {
                let inner: Vec<String> = items.iter().map(|i| i.render()).collect();
                format!("[{}]", inner.join(", "))
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    Gt,
    Lt,
    Ge,
    Le,
    Eq,
    Ne,
}

impl CmpOp {
    pub fn as_str(&self) -> &'static str {
        match self {
            CmpOp::Gt => ">",
            CmpOp::Lt => "<",
            CmpOp::Ge => ">=",
            CmpOp::Le => "<=",
            CmpOp::Eq => "==",
            CmpOp::Ne => "!=",
        }
    }

    pub fn apply_num(&self, a: f64, b: f64) -> bool {
        match self {
            CmpOp::Gt => a > b,
            CmpOp::Lt => a < b,
            CmpOp::Ge => a >= b,
            CmpOp::Le => a <= b,
            CmpOp::Eq => (a - b).abs() < f64::EPSILON,
            CmpOp::Ne => (a - b).abs() >= f64::EPSILON,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cond {
    pub left: Value,
    /// Absent means "is the left side truthy?".
    pub op: Option<CmpOp>,
    pub right: Option<Value>,
}

impl Cond {
    pub fn render(&self) -> String {
        match (&self.op, &self.right) {
            (Some(op), Some(r)) => format!("{} {} {}", self.left.render(), op.as_str(), r.render()),
            _ => self.left.render(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    /// `fs.list`, or the name of a declared foreign tool.
    pub target: String,
    pub args: Vec<(String, Value)>,
    pub line: usize,
}

impl Call {
    pub fn arg(&self, name: &str) -> Option<&Value> {
        self.args.iter().find(|(k, _)| k == name).map(|(_, v)| v)
    }

    /// The argument that determines the capability's scope.
    ///
    /// Convention, checked in order, so that `fs.write path: ~/a` yields
    /// `fs.write:~/a` without the author restating it.
    pub fn scope_value(&self) -> Option<&Value> {
        for key in ["path", "from", "target", "name", "unit", "output", "project"] {
            if let Some(v) = self.arg(key) {
                return Some(v);
            }
        }
        None
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    /// `name = domain.action args...`
    Bind { name: String, call: Call },
    /// A call whose result is not bound.
    Effect(Call),
    /// Stop the flow unless the condition holds.
    Gate { cond: Cond, line: usize },
    /// Require an explicit human yes before continuing.
    Ask { prompt: Value, line: usize },
    /// `on linux { ... }` — a platform-conditional block.
    On { platform: String, body: Vec<Stmt>, line: usize },
}

/// A foreign tool: an existing program made callable as a GLYPH node.
///
/// This is how GLYPH stays compatible with software that has never heard of it.
/// A foreign call still compiles to a `shell.exec` capability, so it is policed
/// and journalled exactly like everything else.
#[derive(Debug, Clone, PartialEq)]
pub struct Foreign {
    pub name: String,
    /// The program to run, e.g. `HandBrakeCLI`.
    pub cmd: String,
    /// Which platforms this binding applies to. Empty means all of them.
    pub platforms: Vec<String>,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Flow {
    pub name: String,
    pub meta: BTreeMap<String, String>,
    pub foreigns: BTreeMap<String, Vec<Foreign>>,
    pub stmts: Vec<Stmt>,
    pub line: usize,
}

impl Flow {
    pub fn description(&self) -> &str {
        self.meta.get("description").map(|s| s.as_str()).unwrap_or("")
    }

    /// Pick the foreign binding that applies to `platform`.
    pub fn foreign_for(&self, name: &str, platform: &str) -> Option<&Foreign> {
        let candidates = self.foreigns.get(name)?;
        candidates
            .iter()
            .find(|f| f.platforms.iter().any(|p| p == platform))
            .or_else(|| candidates.iter().find(|f| f.platforms.is_empty()))
    }

    pub fn to_json(&self) -> Json {
        json_obj([
            ("name", self.name.clone().into()),
            ("description", self.description().into()),
            ("statements", self.stmts.len().into()),
            (
                "foreign",
                Json::Arr(self.foreigns.keys().map(|k| Json::Str(k.clone())).collect()),
            ),
        ])
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Program {
    pub flows: Vec<Flow>,
}

impl Program {
    pub fn flow(&self, name: &str) -> Option<&Flow> {
        self.flows.iter().find(|f| f.name == name)
    }
}

/// The platform this build is running on, as GLYPH names it.
pub fn current_platform() -> &'static str {
    match std::env::consts::OS {
        "linux" => "linux",
        "macos" => "macos",
        "windows" => "windows",
        other => other,
    }
}
