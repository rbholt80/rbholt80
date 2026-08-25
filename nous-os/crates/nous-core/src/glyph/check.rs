//! Static checking: what can this program possibly do?
//!
//! This is the point of GLYPH. Because every statement is a capability request
//! rather than arbitrary code, a flow can be checked against policy *before any
//! of it runs* — and the answer is complete, not a guess. The result is a
//! [`Manifest`]: every capability the flow may exercise, where the scope is
//! known, and where it is only decidable at run time.

use super::ast::*;
use crate::cap::{is_known, Capability, Risk};
use crate::json::{json_obj, Json};
use crate::policy::{Decision, Policy, Subject};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub severity: Severity,
    pub line: usize,
    pub message: String,
}

impl Diagnostic {
    pub fn render(&self, flow: &str) -> String {
        let tag = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        format!("{}: {}:{}: {}", tag, flow, self.line, self.message)
    }
}

/// One capability the flow may exercise.
#[derive(Debug, Clone)]
pub struct Requested {
    pub capability: Capability,
    /// False when the scope depends on an earlier step's result, and so can
    /// only be adjudicated at run time.
    pub scope_known: bool,
    pub line: usize,
    pub summary: String,
    /// Set when this call is only reached on a particular platform.
    pub platform: Option<String>,
}

impl Requested {
    pub fn to_json(&self) -> Json {
        json_obj([
            ("capability", self.capability.to_string().into()),
            ("risk", self.capability.risk().to_string().into()),
            ("scope_known", self.scope_known.into()),
            ("line", self.line.into()),
            ("summary", self.summary.clone().into()),
            ("platform", self.platform.clone().map(Json::Str).unwrap_or(Json::Null)),
        ])
    }
}

#[derive(Debug, Clone)]
pub struct Manifest {
    pub flow: String,
    pub description: String,
    pub requests: Vec<Requested>,
    pub diagnostics: Vec<Diagnostic>,
    /// How many explicit human confirmations the flow contains.
    pub asks: usize,
    /// How many conditions can stop it early.
    pub gates: usize,
}

impl Manifest {
    pub fn is_valid(&self) -> bool {
        !self.diagnostics.iter().any(|d| d.severity == Severity::Error)
    }

    pub fn errors(&self) -> Vec<&Diagnostic> {
        self.diagnostics.iter().filter(|d| d.severity == Severity::Error).collect()
    }

    /// The highest risk anything in this flow can reach.
    pub fn peak_risk(&self) -> Risk {
        self.requests.iter().map(|r| r.capability.risk()).max().unwrap_or(Risk::Read)
    }

    /// True when nothing in the flow can change the machine.
    pub fn is_read_only(&self) -> bool {
        self.peak_risk() == Risk::Read
    }

    /// Adjudicate the whole flow against policy up front.
    ///
    /// Returns one line per capability. Dynamic scopes are checked again at run
    /// time — this answers "could this be allowed at all?", which is the
    /// question worth asking before you press go.
    pub fn preflight(&self, policy: &Policy, subject: &Subject) -> Vec<(String, Decision)> {
        self.requests
            .iter()
            .map(|r| {
                let verdict = policy.evaluate(subject, &r.capability);
                (r.capability.to_string(), verdict.decision)
            })
            .collect()
    }

    /// A short human summary of the blast radius.
    pub fn blast_radius(&self) -> String {
        if self.requests.is_empty() {
            return "does nothing".to_string();
        }
        let mut domains: Vec<&str> =
            self.requests.iter().map(|r| r.capability.domain.as_str()).collect();
        domains.sort_unstable();
        domains.dedup();
        format!(
            "{} {} across {} ({} risk)",
            self.requests.len(),
            if self.requests.len() == 1 { "action" } else { "actions" },
            domains.join(", "),
            self.peak_risk()
        )
    }

    pub fn to_json(&self) -> Json {
        json_obj([
            ("flow", self.flow.clone().into()),
            ("description", self.description.clone().into()),
            ("valid", self.is_valid().into()),
            ("read_only", self.is_read_only().into()),
            ("peak_risk", self.peak_risk().to_string().into()),
            ("blast_radius", self.blast_radius().into()),
            ("asks", self.asks.into()),
            ("gates", self.gates.into()),
            ("requests", Json::Arr(self.requests.iter().map(|r| r.to_json()).collect())),
            (
                "diagnostics",
                Json::Arr(
                    self.diagnostics
                        .iter()
                        .map(|d| {
                            json_obj([
                                ("severity", if d.severity == Severity::Error { "error" } else { "warning" }.into()),
                                ("line", d.line.into()),
                                ("message", d.message.clone().into()),
                            ])
                        })
                        .collect(),
                ),
            ),
        ])
    }
}

/// Check a flow for the given platform.
pub fn check(flow: &Flow, platform: &str) -> Manifest {
    let mut m = Manifest {
        flow: flow.name.clone(),
        description: flow.description().to_string(),
        requests: Vec::new(),
        diagnostics: Vec::new(),
        asks: 0,
        gates: 0,
    };
    let mut bound: Vec<String> = Vec::new();
    walk(flow, &flow.stmts, platform, None, &mut bound, &mut m);
    m
}

fn walk(
    flow: &Flow,
    stmts: &[Stmt],
    platform: &str,
    active_platform: Option<String>,
    bound: &mut Vec<String>,
    m: &mut Manifest,
) {
    for stmt in stmts {
        match stmt {
            Stmt::On { platform: p, body, .. } => {
                // Blocks for other platforms are still checked — a flow that is
                // broken on Windows should say so when you lint it on Linux.
                walk(flow, body, platform, Some(p.clone()), bound, m);
            }
            Stmt::Gate { cond, line } => {
                m.gates += 1;
                check_refs(&cond.left, *line, bound, m);
                if let Some(r) = &cond.right {
                    check_refs(r, *line, bound, m);
                }
            }
            Stmt::Ask { prompt, line } => {
                m.asks += 1;
                check_refs(prompt, *line, bound, m);
            }
            Stmt::Bind { name, call } => {
                check_call(flow, call, platform, active_platform.clone(), bound, m);
                if bound.contains(name) {
                    m.diagnostics.push(Diagnostic {
                        severity: Severity::Warning,
                        line: call.line,
                        message: format!("`{}` shadows an earlier binding of the same name", name),
                    });
                }
                bound.push(name.clone());
            }
            Stmt::Effect(call) => {
                check_call(flow, call, platform, active_platform.clone(), bound, m)
            }
        }
    }
}

fn check_call(
    flow: &Flow,
    call: &Call,
    platform: &str,
    active_platform: Option<String>,
    bound: &[String],
    m: &mut Manifest,
) {
    for (_, v) in &call.args {
        check_refs(v, call.line, bound, m);
    }

    // A foreign tool compiles to shell.exec, and so is governed like any other
    // capability rather than escaping the model.
    if flow.foreigns.contains_key(&call.target) {
        let effective = active_platform.clone().unwrap_or_else(|| platform.to_string());
        match flow.foreign_for(&call.target, &effective) {
            Some(f) => {
                m.requests.push(Requested {
                    capability: Capability::new("shell", "exec", &f.cmd),
                    scope_known: true,
                    line: call.line,
                    summary: format!("run the external tool `{}`", f.cmd),
                    platform: active_platform,
                });
            }
            None => m.diagnostics.push(Diagnostic {
                severity: Severity::Error,
                line: call.line,
                message: format!(
                    "`{}` has no binding for {} — add `use foreign {} cmd: \"...\" on: [{}]`",
                    call.target, effective, call.target, effective
                ),
            }),
        }
        return;
    }

    if !call.target.contains('.') {
        m.diagnostics.push(Diagnostic {
            severity: Severity::Error,
            line: call.line,
            message: format!(
                "`{}` is neither a capability nor a declared foreign tool",
                call.target
            ),
        });
        return;
    }
    if !is_known(&call.target) {
        m.diagnostics.push(Diagnostic {
            severity: Severity::Error,
            line: call.line,
            message: format!("no such capability `{}`", call.target),
        });
        return;
    }

    let (scope, scope_known) = match call.scope_value() {
        Some(v) => match v.literal().and_then(|j| j.as_str().map(String::from)) {
            Some(s) => (s, true),
            None => ("*".to_string(), false),
        },
        None => ("*".to_string(), true),
    };

    let cap = match Capability::parse(&format!("{}:{}", call.target, scope)) {
        Ok(c) => c,
        Err(e) => {
            m.diagnostics.push(Diagnostic {
                severity: Severity::Error,
                line: call.line,
                message: e,
            });
            return;
        }
    };

    let summary = if call.args.is_empty() {
        call.target.clone()
    } else {
        let rendered: Vec<String> =
            call.args.iter().map(|(k, v)| format!("{}: {}", k, v.render())).collect();
        format!("{} {}", call.target, rendered.join(", "))
    };

    m.requests.push(Requested { capability: cap, scope_known, line: call.line, summary, platform: active_platform });
}

fn check_refs(v: &Value, line: usize, bound: &[String], m: &mut Manifest) {
    for r in v.refs() {
        let head = r.split('.').next().unwrap_or(&r);
        // A bare word that names no binding is a literal symbol, not a mistake
        // — `kinds: [duplicate]` is the common case.
        let is_dotted = r.contains('.');
        if !bound.iter().any(|b| b == head) && is_dotted {
            m.diagnostics.push(Diagnostic {
                severity: Severity::Error,
                line,
                message: format!("`{}` refers to `{}`, which is not bound above it", r, head),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::parse::parse;
    use super::*;

    fn manifest(src: &str) -> Manifest {
        let p = parse(src).unwrap();
        check(&p.flows[0], "linux")
    }

    #[test]
    fn reports_every_capability_a_flow_can_reach() {
        let m = manifest(
            "flow tidy {
               found = curate.scan roots: [~/Downloads]
               plan  = curate.propose kinds: [duplicate]
               curate.apply steps: plan.steps
             }",
        );
        assert!(m.is_valid(), "{:?}", m.diagnostics);
        let names: Vec<String> = m.requests.iter().map(|r| r.capability.name()).collect();
        assert_eq!(names, ["curate.scan", "curate.propose", "curate.apply"]);
    }

    #[test]
    fn distinguishes_known_scopes_from_runtime_ones() {
        let m = manifest(
            "flow t {
               listing = fs.list path: ~/Downloads
               fs.write path: listing.first content: \"x\"
             }",
        );
        assert!(m.requests[0].scope_known, "a literal path is known statically");
        assert_eq!(m.requests[0].capability.scope, "~/Downloads");
        assert!(!m.requests[1].scope_known, "a path from a prior result is not");
    }

    #[test]
    fn a_read_only_flow_is_recognised_as_such() {
        let m = manifest("flow look { fs.list path: ~/Documents\n sys.metrics }");
        assert!(m.is_read_only());
        assert_eq!(m.peak_risk(), Risk::Read);
    }

    #[test]
    fn peak_risk_reflects_the_worst_step() {
        let m = manifest("flow t { fs.list path: /tmp\n pkg.install name: mpv }");
        assert_eq!(m.peak_risk(), Risk::Elevated);
        assert!(!m.is_read_only());
        assert!(m.blast_radius().contains("elevated"), "{}", m.blast_radius());
    }

    #[test]
    fn rejects_capabilities_that_do_not_exist() {
        let m = manifest("flow t { fs.incinerate path: /home }");
        assert!(!m.is_valid());
        assert!(m.errors()[0].message.contains("no such capability"), "{:?}", m.errors());
    }

    #[test]
    fn rejects_references_to_unbound_names() {
        let m = manifest("flow t { curate.apply steps: plan.steps }");
        assert!(!m.is_valid());
        assert!(m.errors()[0].message.contains("not bound above it"), "{:?}", m.errors());
    }

    #[test]
    fn bare_words_are_symbols_not_broken_references() {
        let m = manifest("flow t { curate.propose kinds: [duplicate, screenshots] }");
        assert!(m.is_valid(), "{:?}", m.diagnostics);
    }

    #[test]
    fn a_reference_must_be_bound_before_it_is_used() {
        let ok = manifest("flow t { plan = curate.propose\n curate.apply steps: plan.steps }");
        assert!(ok.is_valid(), "{:?}", ok.diagnostics);

        let backwards = manifest("flow t { curate.apply steps: plan.steps\n plan = curate.propose }");
        assert!(!backwards.is_valid(), "order matters: a flow is not a graph you can reorder");
    }

    #[test]
    fn foreign_tools_become_governed_shell_capabilities() {
        let m = manifest(
            r#"flow t {
                 use foreign handbrake cmd: "HandBrakeCLI"
                 handbrake args: [-i, ~/a.mkv]
               }"#,
        );
        assert!(m.is_valid(), "{:?}", m.diagnostics);
        assert_eq!(m.requests[0].capability.to_string(), "shell.exec:HandBrakeCLI");
        assert_eq!(m.requests[0].capability.risk(), Risk::Elevated);
    }

    #[test]
    fn a_foreign_tool_missing_a_platform_binding_is_an_error() {
        let m = manifest(
            r#"flow t {
                 use foreign winget cmd: "winget.exe" on: [windows]
                 on linux { winget args: [install] }
               }"#,
        );
        assert!(!m.is_valid());
        assert!(m.errors()[0].message.contains("no binding for linux"), "{:?}", m.errors());
    }

    #[test]
    fn platform_blocks_are_attributed_in_the_manifest() {
        let m = manifest(
            "flow t {
               on linux   { pkg.install name: mpv }
               on windows { pkg.install name: mpv }
             }",
        );
        assert!(m.is_valid(), "{:?}", m.diagnostics);
        assert_eq!(m.requests[0].platform.as_deref(), Some("linux"));
        assert_eq!(m.requests[1].platform.as_deref(), Some("windows"));
    }

    #[test]
    fn preflight_adjudicates_the_whole_flow_before_it_runs() {
        let m = manifest(
            "flow t {
               plan = curate.propose kinds: [duplicate]
               curate.apply steps: plan.steps
             }",
        );
        let verdicts = m.preflight(&Policy::builtin(), &Subject::User);
        assert_eq!(verdicts.len(), 2);
        assert_eq!(verdicts[0].1, Decision::Allow, "proposing is read-only");
        assert!(
            matches!(verdicts[1].1, Decision::Confirm(_)),
            "applying a tidy-up must ask first, and you can see that before running it"
        );
    }

    #[test]
    fn preflight_surfaces_a_denial_without_executing_anything() {
        let m = manifest("flow t { fs.write path: /boot/grub/grub.cfg content: \"x\" }");
        let verdicts = m.preflight(&Policy::builtin(), &Subject::User);
        assert!(matches!(verdicts[0].1, Decision::Deny(_)), "{:?}", verdicts);
    }

    #[test]
    fn counts_gates_and_asks() {
        let m = manifest(
            r#"flow t {
                 plan = curate.propose
                 gate plan.count > 0
                 ask "go ahead?"
                 curate.apply steps: plan.steps
               }"#,
        );
        assert_eq!(m.gates, 1);
        assert_eq!(m.asks, 1);
    }

    #[test]
    fn shadowed_bindings_warn_but_do_not_fail() {
        let m = manifest("flow t { a = sys.metrics\n a = sys.info }");
        assert!(m.is_valid());
        assert_eq!(m.diagnostics.len(), 1);
        assert_eq!(m.diagnostics[0].severity, Severity::Warning);
    }
}
