//! The capability broker.
//!
//! Every effect in NOUS OS passes through here. The broker adjudicates each
//! step against policy, asks the human when policy says to ask, executes what
//! survives, and journals all of it — including the refusals.
//!
//! Execution is deliberately *not* interactive mid-flight. A plan is shown in
//! full, approved in full, and then runs. The alternative — stopping halfway to
//! ask — trains people to click yes on a dialogue whose context they have
//! already lost, which is how consent becomes a formality.

use crate::bus::Bus;
use crate::exec::{self, ExecCtx};
use crate::index::Index;
use crate::resolve::{self, Env, HANDLER_FLOW};
use nous_core::glyph::ast::CmpOp;
use nous_core::journal::{now_secs, Outcome, Record, Undo};
use nous_core::json::{json_obj, Json};
use nous_core::proto::topic;
use nous_core::{Capability, Config, Decision, Event, Journal, Plan, Policy, Step, Subject};
use std::sync::Arc;

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    /// Every step ran.
    Completed,
    /// A step needs the human to say yes. Nothing after it ran.
    NeedsApproval,
    /// A `gate` condition was false. This is a success, not a failure.
    Stopped,
    /// Policy refused a step.
    Blocked,
    /// A step failed while executing.
    Failed,
}

impl RunStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            RunStatus::Completed => "completed",
            RunStatus::NeedsApproval => "needs_approval",
            RunStatus::Stopped => "stopped",
            RunStatus::Blocked => "blocked",
            RunStatus::Failed => "failed",
        }
    }
}

pub struct RunOptions {
    pub subject: Subject,
    /// Compute effects and describe them without applying any.
    pub dry_run: bool,
    /// The human has seen the plan and said yes.
    pub approved: bool,
}

impl Default for RunOptions {
    fn default() -> Self {
        RunOptions {
            subject: Subject::User,
            dry_run: false,
            approved: false,
        }
    }
}

pub struct Broker {
    pub cfg: Config,
    pub policy: Policy,
    pub journal: Journal,
    pub bus: Arc<Bus>,
}

impl Broker {
    pub fn new(cfg: Config, policy: Policy, journal: Journal, bus: Arc<Bus>) -> Broker {
        Broker {
            cfg,
            policy,
            journal,
            bus,
        }
    }

    fn publish(&self, topic: &str, data: Json) {
        self.bus.publish(Event::new(topic, data));
    }

    /// Adjudicate every step without running anything.
    ///
    /// This is what the shell shows before you press go: the complete list of
    /// what the plan will do and what policy makes of each part.
    pub fn preflight(&self, plan: &Plan, subject: &Subject) -> Json {
        let mut needs_approval = false;
        let mut blocked = false;
        let steps: Vec<Json> = plan
            .steps
            .iter()
            .map(|s| {
                if s.handler == HANDLER_FLOW {
                    if s.capability == "flow.ask" {
                        needs_approval = true;
                    }
                    return json_obj([
                        ("id", s.id.clone().into()),
                        ("capability", s.capability.clone().into()),
                        ("summary", s.summary.clone().into()),
                        ("decision", "control".into()),
                        ("risk", "read".into()),
                    ]);
                }
                let (decision, risk, reason) = match Capability::parse(&s.capability) {
                    Ok(cap) => {
                        let v = self.policy.evaluate(subject, &cap);
                        match &v.decision {
                            Decision::Allow => ("allow", v.risk.to_string(), String::new()),
                            Decision::Confirm(r) => {
                                needs_approval = true;
                                ("confirm", v.risk.to_string(), r.clone())
                            }
                            Decision::Deny(r) => {
                                blocked = true;
                                ("deny", v.risk.to_string(), r.clone())
                            }
                        }
                    }
                    Err(e) => {
                        blocked = true;
                        ("deny", "critical".to_string(), e)
                    }
                };
                json_obj([
                    ("id", s.id.clone().into()),
                    ("capability", s.capability.clone().into()),
                    ("summary", s.summary.clone().into()),
                    ("decision", decision.into()),
                    ("risk", risk.into()),
                    ("reason", reason.into()),
                ])
            })
            .collect();

        json_obj([
            ("intent_id", plan.intent_id.clone().into()),
            ("utterance", plan.utterance.clone().into()),
            ("origin", plan.origin.clone().into()),
            ("confidence", plan.confidence.into()),
            ("steps", Json::Arr(steps)),
            ("needs_approval", needs_approval.into()),
            ("blocked", blocked.into()),
        ])
    }

    /// Run a plan.
    pub fn run(&self, plan: &Plan, opts: &RunOptions) -> Json {
        let mut env: Env = Env::new();
        let mut results: Vec<Json> = Vec::new();
        let mut status = RunStatus::Completed;
        let mut message = String::new();

        self.publish(
            topic::INTENT,
            json_obj([
                ("phase", "start".into()),
                ("intent_id", plan.intent_id.clone().into()),
                ("utterance", plan.utterance.clone().into()),
                ("steps", plan.steps.len().into()),
            ]),
        );

        for step in &plan.steps {
            // Resolve any values that depended on earlier steps.
            let args = match resolve::resolve_args(&step.args, &env) {
                Ok(a) => a,
                Err(e) => {
                    status = RunStatus::Failed;
                    message = e.clone();
                    results.push(step_result(step, "failed", Json::Null, &e));
                    break;
                }
            };
            let resolved = Step {
                args: args.clone(),
                ..step.clone()
            };

            // Control flow has no capability because it has no effect.
            if step.handler == HANDLER_FLOW {
                match self.run_control(&resolved, opts) {
                    ControlOutcome::Continue => {
                        results.push(step_result(step, "ok", Json::Null, ""));
                        continue;
                    }
                    ControlOutcome::Stop(reason) => {
                        status = RunStatus::Stopped;
                        message = reason.clone();
                        results.push(step_result(step, "stopped", Json::Null, &reason));
                        break;
                    }
                    ControlOutcome::NeedsApproval(prompt) => {
                        status = RunStatus::NeedsApproval;
                        message = prompt.clone();
                        results.push(step_result(step, "needs_approval", Json::Null, &prompt));
                        break;
                    }
                }
            }

            let cap = match Capability::parse(&step.capability) {
                Ok(c) => c,
                Err(e) => {
                    status = RunStatus::Failed;
                    message = e.clone();
                    results.push(step_result(step, "failed", Json::Null, &e));
                    break;
                }
            };

            let verdict = self.policy.evaluate(&opts.subject, &cap);
            let outcome_kind = match &verdict.decision {
                Decision::Deny(reason) => {
                    if let Err(e) = self.record(
                        &cap,
                        verdict.decision.kind(),
                        Outcome::Refused,
                        plan,
                        reason,
                        Undo::None,
                    ) {
                        // The audit trail is the point. A hole in it is not a
                        // detail to swallow.
                        nous_core::log_error!("broker", "could not journal a refusal: {}", e);
                    }
                    self.publish(
                        topic::CAPABILITY,
                        json_obj([
                            ("capability", cap.to_string().into()),
                            ("decision", "deny".into()),
                            ("reason", reason.clone().into()),
                        ]),
                    );
                    status = RunStatus::Blocked;
                    message = verdict.explain();
                    results.push(step_result(step, "denied", Json::Null, reason));
                    break;
                }
                Decision::Confirm(reason) => {
                    if !opts.approved && !opts.dry_run {
                        status = RunStatus::NeedsApproval;
                        message = format!("{} — {}", step.summary, reason);
                        results.push(step_result(step, "needs_approval", Json::Null, reason));
                        break;
                    }
                    Outcome::Confirmed
                }
                Decision::Allow => Outcome::Executed,
            };

            let ctx = ExecCtx::new(&self.cfg, &self.journal, opts.dry_run);
            let effect = match self.dispatch(&cap, &resolved, &ctx, plan, opts) {
                Ok(e) => e,
                Err(e) => {
                    if let Err(je) = self.record(
                        &cap,
                        verdict.decision.kind(),
                        Outcome::Failed,
                        plan,
                        &e,
                        Undo::None,
                    ) {
                        nous_core::log_error!("broker", "could not journal a failure: {}", je);
                    }
                    status = RunStatus::Failed;
                    message = e.clone();
                    results.push(step_result(step, "failed", Json::Null, &e));
                    break;
                }
            };

            let recorded = if opts.dry_run {
                Outcome::DryRun
            } else {
                outcome_kind
            };
            let seq = self
                .record(
                    &cap,
                    verdict.decision.kind(),
                    recorded,
                    plan,
                    &effect.detail,
                    effect.undo,
                )
                .unwrap_or(0);

            if let Some(name) = args.get("$bind").and_then(|v| v.as_str()) {
                env.insert(name.to_string(), effect.result.clone());
            }

            self.publish(
                topic::INTENT,
                json_obj([
                    ("phase", "step".into()),
                    ("intent_id", plan.intent_id.clone().into()),
                    ("step", step.id.clone().into()),
                    ("detail", effect.detail.clone().into()),
                    ("seq", seq.into()),
                ]),
            );

            let mut r = step_result(step, "ok", effect.result, &effect.detail);
            r.set("seq", seq.into());
            results.push(r);
        }

        self.publish(
            topic::INTENT,
            json_obj([
                ("phase", "end".into()),
                ("intent_id", plan.intent_id.clone().into()),
                ("status", status.as_str().into()),
            ]),
        );

        json_obj([
            ("intent_id", plan.intent_id.clone().into()),
            ("status", status.as_str().into()),
            ("message", message.into()),
            ("dry_run", opts.dry_run.into()),
            ("results", Json::Arr(results)),
            ("plan", plan.to_json()),
        ])
    }

    /// Dispatch to an executor, or to the subsystems the broker owns itself.
    fn dispatch(
        &self,
        cap: &Capability,
        step: &Step,
        ctx: &ExecCtx,
        plan: &Plan,
        opts: &RunOptions,
    ) -> Result<exec::Effect, String> {
        match (cap.domain.as_str(), cap.action.as_str()) {
            ("curate", "apply") => self.apply_proposal(step, ctx, plan, opts),
            ("journal", "read") => {
                let n = step
                    .args
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(25) as usize;
                let records = self.journal.tail(n)?;
                let items: Vec<Json> = records.iter().map(|r| r.to_json()).collect();
                Ok(exec::Effect::read_only(
                    json_obj([
                        ("records", Json::Arr(items.clone())),
                        ("count", items.len().into()),
                    ]),
                    format!("read {} journal records", items.len()),
                ))
            }
            ("journal", "revert") => self.revert(step, ctx),
            ("fs", "search") => {
                let q = step.args.str_or("query", "");
                let kind = step.args.get("kind").and_then(|v| v.as_str());
                let limit = step
                    .args
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(40) as usize;
                let idx = Index::load();
                let result = idx.search_json(q, kind, limit);
                let n = result.arr_or_empty("results").len();
                Ok(exec::Effect::read_only(
                    result,
                    format!("{} matches for '{}'", n, q),
                ))
            }
            ("fs", "index") => {
                let roots = self.cfg.paths("index.roots");
                let idx = Index::build(&roots, &self.cfg);
                let n = idx.docs.len();
                if !ctx.dry_run {
                    idx.save()?;
                }
                Ok(exec::Effect::read_only(
                    json_obj([("indexed", n.into())]),
                    format!("indexed {} files", n),
                ))
            }
            _ => exec::execute(step, ctx),
        }
    }

    /// Expand a curator proposal into individually governed moves.
    ///
    /// Each move is adjudicated and journalled on its own, so the ledger shows
    /// nine entries rather than one, and each can be reversed independently.
    /// A partial failure therefore leaves a coherent trail instead of an
    /// unknown half-state.
    fn apply_proposal(
        &self,
        step: &Step,
        ctx: &ExecCtx,
        plan: &Plan,
        opts: &RunOptions,
    ) -> Result<exec::Effect, String> {
        let steps: Vec<Step> = step
            .args
            .arr_or_empty("steps")
            .iter()
            .map(Step::from_json)
            .collect();
        if steps.is_empty() {
            return Err("this proposal has no steps to apply".to_string());
        }

        let mut applied = 0usize;
        let mut seqs: Vec<Json> = Vec::new();
        let mut failed: Vec<Json> = Vec::new();

        for s in &steps {
            let sub = match Capability::parse(&s.capability) {
                Ok(c) => c,
                Err(e) => {
                    failed.push(json_obj([
                        ("step", s.id.clone().into()),
                        ("error", e.into()),
                    ]));
                    continue;
                }
            };
            let verdict = self.policy.evaluate(&opts.subject, &sub);
            match &verdict.decision {
                Decision::Deny(reason) => {
                    let _ = self.record(&sub, "deny", Outcome::Refused, plan, reason, Undo::None);
                    failed.push(json_obj([
                        ("step", s.id.clone().into()),
                        ("summary", s.summary.clone().into()),
                        ("error", reason.clone().into()),
                    ]));
                    continue;
                }
                // Approving the tidy-up approves the moves it is made of; it
                // would be theatre to ask again nine times.
                Decision::Confirm(reason) if !opts.approved && !opts.dry_run => {
                    failed.push(json_obj([
                        ("step", s.id.clone().into()),
                        ("error", format!("needs approval: {}", reason).into()),
                    ]));
                    continue;
                }
                _ => {}
            }

            match exec::execute(s, ctx) {
                Ok(effect) => {
                    let outcome = if ctx.dry_run {
                        Outcome::DryRun
                    } else if matches!(verdict.decision, Decision::Confirm(_)) {
                        Outcome::Confirmed
                    } else {
                        Outcome::Executed
                    };
                    match self.record(
                        &sub,
                        verdict.decision.kind(),
                        outcome,
                        plan,
                        &effect.detail,
                        effect.undo,
                    ) {
                        Ok(seq) => {
                            seqs.push(seq.into());
                            applied += 1;
                        }
                        Err(e) => failed.push(json_obj([
                            ("step", s.id.clone().into()),
                            ("error", e.into()),
                        ])),
                    }
                }
                Err(e) => {
                    let _ = self.record(
                        &sub,
                        verdict.decision.kind(),
                        Outcome::Failed,
                        plan,
                        &e,
                        Undo::None,
                    );
                    failed.push(json_obj([
                        ("step", s.id.clone().into()),
                        ("summary", s.summary.clone().into()),
                        ("error", e.into()),
                    ]));
                }
            }
        }

        Ok(exec::Effect::with_undo(
            json_obj([
                ("applied", applied.into()),
                ("entries", Json::Arr(seqs)),
                ("failed", Json::Arr(failed.clone())),
            ]),
            // The aggregate itself has nothing extra to reverse: the individual
            // moves each carry their own undo.
            Undo::None,
            format!(
                "tidied {} of {} items{}",
                applied,
                steps.len(),
                if failed.is_empty() {
                    String::new()
                } else {
                    format!(" ({} could not be moved)", failed.len())
                }
            ),
        ))
    }

    /// Reverse a journalled action.
    fn revert(&self, step: &Step, ctx: &ExecCtx) -> Result<exec::Effect, String> {
        let record = match step.args.get("seq").and_then(|v| v.as_u64()) {
            Some(seq) => self
                .journal
                .get(seq)?
                .ok_or_else(|| format!("there is no journal entry {}", seq))?,
            None => self
                .journal
                .last_revertible()?
                .ok_or_else(|| "there is nothing to undo".to_string())?,
        };
        if !record.is_revertible() {
            return Err(match record.undone_by {
                Some(by) => format!("entry {} was already undone by entry {}", record.seq, by),
                None => format!(
                    "entry {} cannot be undone ({})",
                    record.seq,
                    record.undo.describe()
                ),
            });
        }
        if ctx.dry_run {
            return Ok(exec::Effect::read_only(
                json_obj([("seq", record.seq.into())]),
                format!("would {}", record.undo.describe()),
            ));
        }
        let detail = exec::revert(&record.undo, ctx)?;
        // The revert is itself journalled, which is what marks the original as
        // undone and stops it being undone twice.
        self.journal.append(Record {
            seq: 0,
            ts: now_secs(),
            subject: "user".to_string(),
            capability: format!("journal.revert:{}", record.seq),
            risk: "write".to_string(),
            decision: "allow".to_string(),
            outcome: Outcome::Executed,
            intent: format!("undo entry {}", record.seq),
            detail: detail.clone(),
            undo: Undo::None,
            undone_by: None,
        })?;
        Ok(exec::Effect::read_only(
            json_obj([
                ("reverted", record.seq.into()),
                ("detail", detail.clone().into()),
            ]),
            detail,
        ))
    }

    fn run_control(&self, step: &Step, opts: &RunOptions) -> ControlOutcome {
        match step.capability.as_str() {
            "flow.gate" => {
                if evaluate_gate(&step.args) {
                    ControlOutcome::Continue
                } else {
                    ControlOutcome::Stop(format!("{} — nothing to do", step.summary))
                }
            }
            "flow.ask" => {
                if opts.approved || opts.dry_run {
                    ControlOutcome::Continue
                } else {
                    ControlOutcome::NeedsApproval(
                        step.args.str_or("prompt", &step.summary).to_string(),
                    )
                }
            }
            other => ControlOutcome::Stop(format!("unknown control step '{}'", other)),
        }
    }

    fn record(
        &self,
        cap: &Capability,
        decision: &str,
        outcome: Outcome,
        plan: &Plan,
        detail: &str,
        undo: Undo,
    ) -> Result<u64, String> {
        self.journal.append(Record {
            seq: 0,
            ts: now_secs(),
            subject: "user".to_string(),
            capability: cap.to_string(),
            risk: cap.risk().to_string(),
            decision: decision.to_string(),
            outcome,
            intent: plan.utterance.clone(),
            detail: nous_core::secrets::redact(detail),
            undo,
            undone_by: None,
        })
    }
}

enum ControlOutcome {
    Continue,
    Stop(String),
    NeedsApproval(String),
}

/// Evaluate a lowered `gate`.
pub fn evaluate_gate(args: &Json) -> bool {
    let left = args.get("left").cloned().unwrap_or(Json::Null);
    let op = args.get("op").and_then(|v| v.as_str());
    let right = args.get("right").cloned().unwrap_or(Json::Null);

    let op = match op {
        None => return truthy(&left),
        Some(o) => o,
    };
    let cmp = match o_to_op(op) {
        Some(c) => c,
        None => return false,
    };
    match (&left, &right) {
        (Json::Num(a), Json::Num(b)) => cmp.apply_num(*a, *b),
        (Json::Str(a), Json::Str(b)) => match cmp {
            CmpOp::Eq => a == b,
            CmpOp::Ne => a != b,
            _ => false,
        },
        // Comparing a list to a number compares its length, which is what
        // `gate plan.steps > 0` obviously means.
        (Json::Arr(a), Json::Num(b)) => cmp.apply_num(a.len() as f64, *b),
        (Json::Bool(a), Json::Bool(b)) => match cmp {
            CmpOp::Eq => a == b,
            CmpOp::Ne => a != b,
            _ => false,
        },
        _ => false,
    }
}

fn o_to_op(s: &str) -> Option<CmpOp> {
    Some(match s {
        ">" => CmpOp::Gt,
        "<" => CmpOp::Lt,
        ">=" => CmpOp::Ge,
        "<=" => CmpOp::Le,
        "==" => CmpOp::Eq,
        "!=" => CmpOp::Ne,
        _ => return None,
    })
}

fn truthy(v: &Json) -> bool {
    match v {
        Json::Bool(b) => *b,
        Json::Num(n) => *n != 0.0,
        Json::Str(s) => !s.is_empty(),
        Json::Arr(a) => !a.is_empty(),
        Json::Obj(m) => !m.is_empty(),
        Json::Null => false,
    }
}

fn step_result(step: &Step, state: &str, value: Json, detail: &str) -> Json {
    json_obj([
        ("id", step.id.clone().into()),
        ("capability", step.capability.clone().into()),
        ("summary", step.summary.clone().into()),
        ("state", state.into()),
        ("detail", detail.into()),
        ("value", value),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    struct Fixture {
        root: PathBuf,
        broker: Broker,
    }

    /// A broker whose policy permits writing inside its own scratch directory,
    /// layered over the real builtin policy so the protected-path floor and the
    /// confirm rules are exactly the ones that ship.
    fn fixture(tag: &str) -> Fixture {
        let root = std::env::temp_dir().join(format!("nous-broker-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let work = root.join("work");
        std::fs::create_dir_all(&work).unwrap();
        let mut policy = Policy::parse(
            &format!(
                "allow user fs.write:{w}\nallow user fs.mkdir:{w}\nallow user fs.move:{w}",
                w = work.join("**").display()
            ),
            "test",
        )
        .unwrap();
        policy.extend(Policy::builtin());
        let journal = Journal::open(&root.join("journal")).unwrap();
        let broker = Broker::new(
            Config::with_defaults(),
            policy,
            journal,
            Arc::new(Bus::new()),
        );
        Fixture { root, broker }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn plan_of(steps: Vec<Step>) -> Plan {
        Plan {
            intent_id: "i1".into(),
            utterance: "test".into(),
            steps,
            origin: "test".into(),
            confidence: 1.0,
            clarification: None,
        }
    }

    fn write_step(path: &Path, content: &str) -> Step {
        Step::new(
            "s1",
            &format!("fs.write:{}", path.display()),
            "fs",
            "write a file",
            json_obj([
                ("path", path.to_string_lossy().to_string().into()),
                ("content", content.into()),
            ]),
        )
    }

    #[test]
    fn an_allowed_step_runs_and_is_journalled() {
        let f = fixture("allow");
        let target = f.root.join("work/notes.md");
        let broker = &f.broker;
        let out = broker.run(
            &plan_of(vec![write_step(&target, "hello")]),
            &RunOptions::default(),
        );
        assert_eq!(
            out.str_or("status", ""),
            "completed",
            "{}",
            out.to_string_pretty()
        );
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello");
        assert_eq!(broker.journal.read_all().unwrap().len(), 1);
    }

    #[test]
    fn a_denied_step_stops_the_run_and_is_still_recorded() {
        let f = fixture("deny");
        let step = write_step(&PathBuf::from("/boot/grub/grub.cfg"), "x");
        let out = f.broker.run(&plan_of(vec![step]), &RunOptions::default());

        assert_eq!(out.str_or("status", ""), "blocked");
        // A path under the protected floor that certainly does not exist. If
        // the broker ever honoured this write, the file would appear -- and in
        // a container running as root, it genuinely could.
        let forbidden = PathBuf::from("/boot/nous-must-never-write-this");
        let out2 = f.broker.run(
            &plan_of(vec![write_step(&forbidden, "x")]),
            &RunOptions::default(),
        );
        assert_eq!(out2.str_or("status", ""), "blocked");
        assert!(
            !forbidden.exists(),
            "a denied write must not reach the disk"
        );
        // Both refusals are journalled: an agent cannot erase the evidence
        // through the same API it misused.
        let records = f.broker.journal.read_all().unwrap();
        assert_eq!(records.len(), 2, "every refusal must leave a trace");
        assert!(records.iter().all(|r| r.outcome == Outcome::Refused));
    }

    #[test]
    fn a_step_needing_confirmation_does_not_run_unapproved() {
        let f = fixture("confirm");
        let victim = f.root.join("work/gone.txt");
        std::fs::write(&victim, b"still here").unwrap();
        let step = Step::new(
            "s1",
            &format!("fs.delete:{}", victim.display()),
            "fs",
            "delete a file",
            json_obj([("path", victim.to_string_lossy().to_string().into())]),
        );

        let out = f
            .broker
            .run(&plan_of(vec![step.clone()]), &RunOptions::default());
        assert_eq!(out.str_or("status", ""), "needs_approval");
        assert!(victim.exists(), "nothing may happen before approval");

        let approved = RunOptions {
            approved: true,
            ..Default::default()
        };
        let out2 = f.broker.run(&plan_of(vec![step]), &approved);
        assert_eq!(
            out2.str_or("status", ""),
            "completed",
            "{}",
            out2.to_string_pretty()
        );
        assert!(!victim.exists());
    }

    #[test]
    fn dry_run_reports_without_touching_anything() {
        let f = fixture("dry");
        let target = f.root.join("work/dry.txt");
        let opts = RunOptions {
            dry_run: true,
            ..Default::default()
        };
        let out = f
            .broker
            .run(&plan_of(vec![write_step(&target, "x")]), &opts);

        assert_eq!(out.str_or("status", ""), "completed");
        assert!(!target.exists());
        let records = f.broker.journal.read_all().unwrap();
        assert_eq!(records[0].outcome, Outcome::DryRun);
    }

    #[test]
    fn a_false_gate_stops_the_flow_as_a_success() {
        let f = fixture("gate");
        let gate = Step::new(
            "s1",
            "flow.gate",
            HANDLER_FLOW,
            "continue only if there is something to do",
            json_obj([
                ("left", 0i64.into()),
                ("op", ">".into()),
                ("right", 0i64.into()),
            ]),
        );
        let after = write_step(&f.root.join("work/never.txt"), "x");
        let out = f
            .broker
            .run(&plan_of(vec![gate, after]), &RunOptions::default());

        assert_eq!(out.str_or("status", ""), "stopped");
        assert!(!f.root.join("work/never.txt").exists());
    }

    #[test]
    fn an_ask_halts_until_approved() {
        let f = fixture("ask");
        let ask = Step::new(
            "s1",
            "flow.ask",
            HANDLER_FLOW,
            "confirm",
            json_obj([("prompt", "Move 12 files?".into())]),
        );
        let out = f
            .broker
            .run(&plan_of(vec![ask.clone()]), &RunOptions::default());
        assert_eq!(out.str_or("status", ""), "needs_approval");
        assert_eq!(out.str_or("message", ""), "Move 12 files?");

        let approved = RunOptions {
            approved: true,
            ..Default::default()
        };
        let out2 = f.broker.run(&plan_of(vec![ask]), &approved);
        assert_eq!(out2.str_or("status", ""), "completed");
    }

    #[test]
    fn preflight_shows_the_whole_plan_before_anything_runs() {
        let f = fixture("preflight");
        let steps = vec![
            Step::new("s1", "sys.metrics", "sys", "check metrics", Json::obj()),
            Step::new(
                "s2",
                "fs.delete:/home/joey/x",
                "fs",
                "delete a file",
                json_obj([("path", "/home/joey/x".into())]),
            ),
        ];
        let pf = f.broker.preflight(&plan_of(steps), &Subject::User);
        let listed = pf.arr_or_empty("steps");
        assert_eq!(listed[0].str_or("decision", ""), "allow");
        assert_eq!(listed[1].str_or("decision", ""), "confirm");
        assert!(pf.bool_or("needs_approval", false));
        assert!(!pf.bool_or("blocked", true));
    }

    #[test]
    fn undo_reverses_the_last_change() {
        let f = fixture("undo");
        let target = f.root.join("work/edited.txt");
        std::fs::write(&target, b"original").unwrap();
        let broker = &f.broker;
        broker.run(
            &plan_of(vec![write_step(&target, "changed")]),
            &RunOptions::default(),
        );
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "changed");

        let undo = Step::new("u1", "journal.revert", "journal", "undo", Json::obj());
        let out = broker.run(&plan_of(vec![undo]), &RunOptions::default());
        assert_eq!(
            out.str_or("status", ""),
            "completed",
            "{}",
            out.to_string_pretty()
        );
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "original");
    }

    #[test]
    fn the_same_change_cannot_be_undone_twice() {
        let f = fixture("undotwice");
        let target = f.root.join("work/x.txt");
        std::fs::write(&target, b"v1").unwrap();
        let broker = &f.broker;
        broker.run(
            &plan_of(vec![write_step(&target, "v2")]),
            &RunOptions::default(),
        );

        let undo = Step::new("u1", "journal.revert", "journal", "undo", Json::obj());
        assert_eq!(
            broker
                .run(&plan_of(vec![undo.clone()]), &RunOptions::default())
                .str_or("status", ""),
            "completed"
        );
        let second = broker.run(&plan_of(vec![undo]), &RunOptions::default());
        assert_eq!(second.str_or("status", ""), "failed");
        assert!(
            second.str_or("message", "").contains("nothing to undo"),
            "{}",
            second.str_or("message", "")
        );
    }

    #[test]
    fn results_bind_for_later_steps() {
        let f = fixture("bind");
        let mut metrics = Step::new("s1", "sys.metrics", "sys", "metrics", Json::obj());
        metrics.args.set("$bind", "m".into());
        let gate = Step::new(
            "s2",
            "flow.gate",
            HANDLER_FLOW,
            "only if there are cpus",
            json_obj([
                ("left", json_obj([(resolve::REF_KEY, "m.cpus".into())])),
                ("op", ">".into()),
                ("right", 0i64.into()),
            ]),
        );
        let out = f
            .broker
            .run(&plan_of(vec![metrics, gate]), &RunOptions::default());
        assert_eq!(
            out.str_or("status", ""),
            "completed",
            "{}",
            out.to_string_pretty()
        );
    }

    #[test]
    fn gate_comparisons_cover_the_shapes_flows_actually_use() {
        let g = |l: Json, op: &str, r: Json| {
            evaluate_gate(&json_obj([("left", l), ("op", op.into()), ("right", r)]))
        };
        assert!(g(Json::Num(5.0), ">", Json::Num(0.0)));
        assert!(!g(Json::Num(0.0), ">", Json::Num(0.0)));
        assert!(g(Json::Str("a".into()), "==", Json::Str("a".into())));
        // A list compared to a number compares its length.
        assert!(g(Json::Arr(vec![Json::Null]), ">", Json::Num(0.0)));
        // Truthiness with no operator.
        assert!(evaluate_gate(&json_obj([("left", Json::Bool(true))])));
        assert!(!evaluate_gate(&json_obj([("left", Json::Null)])));
    }

    #[test]
    fn applying_a_proposal_journals_every_move_individually() {
        // The regression this was written for: applying a tidy-up used to run
        // its moves inside the executor, so nine files moved and none of them
        // could be undone.
        let f = fixture("proposal-undo");
        let work = f.root.join("work");
        std::fs::create_dir_all(work.join("Downloads")).unwrap();
        let mut sub_steps = Vec::new();
        for name in ["a.mp3", "b.mp3", "c.mp4"] {
            let from = work.join("Downloads").join(name);
            std::fs::write(&from, b"x").unwrap();
            let to = work.join("Library").join(name);
            sub_steps.push(
                Step::new(
                    "m",
                    &format!("fs.move:{}", from.display()),
                    "fs",
                    &format!("move {}", name),
                    json_obj([
                        ("from", from.to_string_lossy().to_string().into()),
                        ("to", to.to_string_lossy().to_string().into()),
                    ]),
                )
                .to_json(),
            );
        }

        let apply = Step::new(
            "s1",
            "curate.apply",
            "curate",
            "apply the tidy-up",
            json_obj([("steps", Json::Arr(sub_steps))]),
        );
        let out = f.broker.run(
            &plan_of(vec![apply]),
            &RunOptions {
                approved: true,
                ..Default::default()
            },
        );
        assert_eq!(
            out.str_or("status", ""),
            "completed",
            "{}",
            out.to_string_pretty()
        );
        assert!(!work.join("Downloads/a.mp3").exists());
        assert!(work.join("Library/a.mp3").exists());

        // Three governed moves, each with its own undo, plus the aggregate.
        let moves: Vec<_> = f
            .broker
            .journal
            .read_all()
            .unwrap()
            .into_iter()
            .filter(|r| r.capability.starts_with("fs.move"))
            .collect();
        assert_eq!(moves.len(), 3, "each move must be journalled on its own");
        assert!(
            moves.iter().all(|m| m.is_revertible()),
            "and each must be reversible"
        );

        // Undoing three times puts every file back.
        for _ in 0..3 {
            let undo = Step::new("u", "journal.revert", "journal", "undo", Json::obj());
            let r = f.broker.run(
                &plan_of(vec![undo]),
                &RunOptions {
                    approved: true,
                    ..Default::default()
                },
            );
            assert_eq!(
                r.str_or("status", ""),
                "completed",
                "{}",
                r.to_string_pretty()
            );
        }
        for name in ["a.mp3", "b.mp3", "c.mp4"] {
            assert!(
                work.join("Downloads").join(name).exists(),
                "{} should be back",
                name
            );
        }
    }

    #[test]
    fn a_denied_move_inside_a_proposal_does_not_stop_the_others() {
        let f = fixture("proposal-partial");
        let work = f.root.join("work");
        std::fs::create_dir_all(&work).unwrap();
        let good_from = work.join("ok.txt");
        std::fs::write(&good_from, b"x").unwrap();

        let sub = vec![
            Step::new(
                "m1",
                "fs.move:/boot/vmlinuz",
                "fs",
                "move something protected",
                json_obj([("from", "/boot/vmlinuz".into()), ("to", "/tmp/x".into())]),
            )
            .to_json(),
            Step::new(
                "m2",
                &format!("fs.move:{}", good_from.display()),
                "fs",
                "move an ordinary file",
                json_obj([
                    ("from", good_from.to_string_lossy().to_string().into()),
                    (
                        "to",
                        work.join("moved.txt").to_string_lossy().to_string().into(),
                    ),
                ]),
            )
            .to_json(),
        ];
        let apply = Step::new(
            "s1",
            "curate.apply",
            "curate",
            "apply",
            json_obj([("steps", Json::Arr(sub))]),
        );
        let out = f.broker.run(
            &plan_of(vec![apply]),
            &RunOptions {
                approved: true,
                ..Default::default()
            },
        );

        let value = &out.arr_or_empty("results")[0];
        assert_eq!(
            value.path("value.applied").and_then(|v| v.as_u64()),
            Some(1)
        );
        assert_eq!(
            value
                .path("value.failed")
                .and_then(|v| v.as_arr())
                .map(|a| a.len()),
            Some(1)
        );
        assert!(
            work.join("moved.txt").exists(),
            "the permitted move should still happen"
        );
    }

    #[test]
    fn a_failing_step_stops_the_rest_of_the_plan() {
        let f = fixture("fail");
        let bad = Step::new(
            "s1",
            "fs.list:/definitely/not/here",
            "fs",
            "list a missing directory",
            json_obj([("path", "/definitely/not/here".into())]),
        );
        let after = Step::new("s2", "sys.metrics", "sys", "metrics", Json::obj());
        let out = f
            .broker
            .run(&plan_of(vec![bad, after]), &RunOptions::default());
        assert_eq!(out.str_or("status", ""), "failed");
        assert_eq!(
            out.arr_or_empty("results").len(),
            1,
            "later steps must not run"
        );
    }
}
