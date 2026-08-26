//! Talking to the daemon, and turning what it says into something the panel
//! can show.
//!
//! The panel must never block. A model call takes seconds and an approval takes
//! as long as the user wants, so every request runs on a worker thread and the
//! event loop picks the reply up on its next tick. The translation from the
//! daemon's JSON into [`Body`] is pure and lives here, which is what makes it
//! testable without a daemon running.

use crate::context::Context;
use nous_core::journal::human_bytes as human;
use nous_core::json::{json_obj, Json};
use nous_core::proto::method;
use nous_ui::panel::{Body, Step};
use nous_ui::theme::{parse_risk, risk_of};

/// What the panel is waiting to have approved.
///
/// The two kinds go back to the daemon by different routes, and conflating them
/// is how an approved plan silently does nothing.
#[derive(Debug, Clone, PartialEq)]
pub enum Pending {
    /// A preflighted plan. Approving means submitting it with `approved`.
    Plan(Json),
    /// A curator proposal that came back inside a result. Approving means
    /// invoking `curate.apply` with the steps.
    Curate(Json),
}

/// A request from the panel to the worker.
#[derive(Debug, Clone)]
pub enum Job {
    /// Work out what an utterance means and what it would take to do it,
    /// against what the panel was summoned over.
    Ask(String, Context),
    Approve(Pending),
    Undo,
}

/// The worker's answer, ready to display.
#[derive(Debug, Clone)]
pub struct Reply {
    pub body: Body,
    /// Set when the body is a proposal, so approving it knows where to send it.
    pub pending: Option<Pending>,
}

impl Reply {
    fn error(msg: impl Into<String>) -> Reply {
        Reply {
            body: Body::Error {
                message: msg.into(),
            },
            pending: None,
        }
    }
}

/// Read the steps out of a preflight and describe them for the panel.
fn plan_steps(preflight: &Json) -> Vec<Step> {
    preflight
        .arr_or_empty("steps")
        .iter()
        .map(|s| Step {
            capability: s.str_or("capability", "?").to_string(),
            summary: s.str_or("summary", "").to_string(),
            // The daemon states the risk it evaluated. Recomputing it here
            // could disagree with what the policy actually decided.
            risk: parse_risk(s.str_or("risk", "")),
        })
        .collect()
}

/// Does this value look like a curator proposal: concrete moves rather than the
/// findings that led to them?
pub fn is_curate_proposal(v: &Json) -> bool {
    let Some(steps) = v.get("steps").and_then(|s| s.as_arr()) else {
        return false;
    };
    !steps.is_empty()
        && v.get("bytes").is_some()
        && steps
            .iter()
            .all(|s| s.get("capability").is_some() && s.get("summary").is_some())
}

fn curate_steps(v: &Json) -> Vec<Step> {
    v.arr_or_empty("steps")
        .iter()
        .map(|s| {
            let capability = s.str_or("capability", "?").to_string();
            Step {
                // Curator steps carry no risk field, so it comes from the same
                // table the policy engine uses rather than being guessed.
                risk: risk_of(&capability),
                summary: s.str_or("summary", "").to_string(),
                capability,
            }
        })
        .collect()
}

/// The reason a plan was refused, taken from the first denied step so the panel
/// can say *what* was refused rather than just that something was.
fn refusal(preflight: &Json) -> String {
    for s in preflight.arr_or_empty("steps") {
        if s.str_or("decision", "") == "deny" {
            let reason = s.str_or("reason", "");
            let what = s.str_or("capability", "");
            return if reason.is_empty() {
                format!("policy refuses {what}")
            } else {
                format!("policy refuses {what}: {reason}")
            };
        }
    }
    "policy refuses part of this".to_string()
}

/// How the request was understood, said plainly. "Understood locally" matters
/// to a user deciding whether anything left the machine.
fn origin_note(preflight: &Json) -> String {
    let origin = preflight.str_or("origin", "");
    if let Some(name) = origin.strip_prefix("assistant:") {
        format!("asking {name}")
    } else if origin.starts_with("model") {
        format!("resolved by {origin}")
    } else {
        "understood on this machine".to_string()
    }
}

/// How many rows of a listing to show before summarising the rest. The panel
/// scrolls a proposal but not a listing, so this is a real limit and the count
/// of what was left out is always shown rather than silently dropped.
const LIST_LIMIT: usize = 12;

fn and_the_rest(shown: usize, total: usize) -> Option<String> {
    (total > shown).then(|| format!("… and {} more", total - shown))
}

/// Render whatever a step produced, so a result shows its content and not just
/// the fact that it happened.
///
/// Returns `None` for a shape with nothing to say, which keeps the panel from
/// growing a blank line under every action.
fn describe(v: &Json) -> Option<String> {
    if let Some(entries) = v.get("entries").and_then(|e| e.as_arr()) {
        if entries.is_empty() {
            return Some("nothing here".into());
        }
        let mut lines: Vec<String> = entries
            .iter()
            .take(LIST_LIMIT)
            .map(|e| {
                let name = e.str_or("name", "");
                if e.bool_or("is_dir", false) {
                    format!("{name}/")
                } else {
                    let size = e.get("size").and_then(|s| s.as_u64()).unwrap_or(0);
                    format!("{name}  ({})", human(size))
                }
            })
            .collect();
        lines.extend(and_the_rest(lines.len(), entries.len()));
        return Some(lines.join("\n"));
    }

    if let Some(findings) = v.get("findings").and_then(|f| f.as_arr()) {
        if findings.is_empty() {
            return None;
        }
        let mut lines: Vec<String> = findings
            .iter()
            .take(LIST_LIMIT)
            .map(|f| {
                let title = f.str_or("title", "");
                let detail = f.str_or("detail", "");
                if detail.is_empty() {
                    title.to_string()
                } else {
                    format!("{title} — {detail}")
                }
            })
            .collect();
        lines.extend(and_the_rest(lines.len(), findings.len()));
        return Some(lines.join("\n"));
    }

    if let Some(procs) = v.get("processes").and_then(|p| p.as_arr()) {
        if procs.is_empty() {
            return None;
        }
        let mut lines: Vec<String> = procs
            .iter()
            .take(LIST_LIMIT)
            .map(|p| {
                let rss = p.get("rss_kb").and_then(|x| x.as_u64()).unwrap_or(0) * 1024;
                format!("{}  {}", p.str_or("name", ""), human(rss))
            })
            .collect();
        lines.extend(and_the_rest(lines.len(), procs.len()));
        return Some(lines.join("\n"));
    }

    if v.get("load1").is_some() {
        return Some(format!(
            "load {:.2} · memory {:.0}% used · disk {:.0}% used",
            v.f64_or("load1", 0.0),
            v.f64_or("mem_used_pct", 0.0),
            v.f64_or("disk_used_pct", 0.0)
        ));
    }

    None
}

/// Turn the response to a submitted intent into a body.
pub fn read_submission(out: &Json) -> Reply {
    let mut failures = Vec::new();
    let mut status = Vec::new();
    let mut produced = Vec::new();

    for r in out.arr_or_empty("results") {
        let value = r.get("value").cloned().unwrap_or(Json::Null);

        // An assistant answered. That is the whole response; show it as prose.
        if let Some(answer) = value.get("answer").and_then(|a| a.as_str()) {
            let source = if value.bool_or("local", false) {
                format!(
                    "{} · on this machine",
                    value.str_or("assistant", "assistant")
                )
            } else {
                format!(
                    "{} · via {}",
                    value.str_or("assistant", "assistant"),
                    value.str_or("backend", "?")
                )
            };
            return Reply {
                body: Body::Answer {
                    text: answer.to_string(),
                    source,
                },
                pending: None,
            };
        }

        // A curator proposal needs approving before anything moves. This is the
        // step that closes the loop: a plan with no way to say "go ahead" is
        // not useful.
        if is_curate_proposal(&value) {
            let steps = curate_steps(&value);
            let summary = value.str_or("summary", "");
            let headline = if summary.is_empty() {
                format!("{} changes proposed", steps.len())
            } else {
                format!("{} changes · {summary}", steps.len())
            };
            return Reply {
                body: Body::Proposal { headline, steps },
                pending: Some(Pending::Curate(value)),
            };
        }

        let state = r.str_or("state", "");
        let text = r.str_or("detail", "").to_string();
        if state == "ok" {
            if !text.is_empty() {
                status.push(text);
            }
            // "sampled machine metrics" is not an answer to "what is my disk
            // usage". Whatever the step actually produced goes in too, kept
            // apart from the status line because it is content, not an outcome.
            if let Some(shown) = describe(&value) {
                produced.push(shown);
            }
        } else {
            failures.push(if text.is_empty() {
                state.to_string()
            } else {
                format!("{state}: {text}")
            });
        }
    }

    if !failures.is_empty() {
        return Reply::error(failures.join("\n"));
    }

    let message = out.str_or("message", "");
    let headline = if !status.is_empty() {
        status.join("\n")
    } else if !message.is_empty() {
        message.to_string()
    } else {
        "done".to_string()
    };
    Reply {
        body: Body::Done {
            headline,
            detail: produced.join("\n"),
            // Only offer undo when the journal actually recorded something to
            // undo; offering it otherwise is a promise the system cannot keep.
            undo_hint: out.bool_or("revertible", false),
        },
        pending: None,
    }
}

/// Run one job against the daemon. Called on the worker thread.
pub fn run(client: &mut nous_core::ipc::Client, job: Job) -> Reply {
    match job {
        Job::Ask(text, context) => ask(client, &text, &context),
        Job::Approve(Pending::Plan(plan)) => {
            match client.call(
                method::INTENT_SUBMIT,
                json_obj([("plan", plan), ("approved", true.into())]),
            ) {
                Ok(out) => read_submission(&out),
                Err(e) => Reply::error(e),
            }
        }
        Job::Approve(Pending::Curate(proposal)) => {
            let steps = proposal.arr_or_empty("steps");
            let n = steps.len();
            match client.call(
                "cap.invoke",
                json_obj([
                    ("capability", "curate.apply".into()),
                    ("args", json_obj([("steps", Json::Arr(steps))])),
                    ("approved", true.into()),
                    ("why", "apply a tidy-up proposal".into()),
                ]),
            ) {
                Ok(out) => {
                    let bad: Vec<String> = out
                        .arr_or_empty("results")
                        .iter()
                        .filter(|r| r.str_or("state", "") != "ok")
                        .map(|r| r.str_or("detail", "failed").to_string())
                        .collect();
                    if bad.is_empty() {
                        Reply {
                            body: Body::Done {
                                headline: format!("applied {n} changes"),
                                detail: String::new(),
                                undo_hint: true,
                            },
                            pending: None,
                        }
                    } else {
                        Reply::error(bad.join("\n"))
                    }
                }
                Err(e) => Reply::error(e),
            }
        }
        Job::Undo => match client.call(method::JOURNAL_REVERT, Json::obj()) {
            Ok(out) => Reply {
                body: Body::Done {
                    headline: out.str_or("detail", "undone").to_string(),
                    detail: String::new(),
                    undo_hint: false,
                },
                pending: None,
            },
            Err(e) => Reply::error(e),
        },
    }
}

fn ask(client: &mut nous_core::ipc::Client, text: &str, context: &Context) -> Reply {
    let params = || json_obj([("text", text.into()), ("context", context.to_json())]);
    let preflight = match client.call(method::INTENT_PLAN, params()) {
        Ok(p) => p,
        Err(e) => return Reply::error(e),
    };

    if preflight.bool_or("blocked", false) {
        return Reply::error(refusal(&preflight));
    }

    // Anything that needs approval is shown before it runs, never after.
    if preflight.bool_or("needs_approval", false) {
        let steps = plan_steps(&preflight);
        return Reply {
            body: Body::Proposal {
                headline: origin_note(&preflight),
                steps,
            },
            pending: Some(Pending::Plan(
                preflight.get("plan").cloned().unwrap_or(Json::Null),
            )),
        };
    }

    match client.call(
        method::INTENT_SUBMIT,
        json_obj([
            ("text", text.into()),
            ("context", context.to_json()),
            ("plan", preflight.get("plan").cloned().unwrap_or(Json::Null)),
            ("approved", true.into()),
        ]),
    ) {
        Ok(out) => read_submission(&out),
        Err(e) => Reply::error(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nous_ui::theme::Risk;

    fn arr(v: Vec<Json>) -> Json {
        Json::Arr(v)
    }

    #[test]
    fn an_assistants_answer_becomes_prose_that_says_where_it_came_from() {
        let out = json_obj([(
            "results",
            arr(vec![json_obj([(
                "value",
                json_obj([
                    ("answer", "Paris.".into()),
                    ("assistant", "claude".into()),
                    ("backend", "anthropic".into()),
                    ("local", false.into()),
                ]),
            )])]),
        )]);
        let reply = read_submission(&out);
        match reply.body {
            Body::Answer { text, source } => {
                assert_eq!(text, "Paris.");
                assert!(source.contains("claude"));
                assert!(
                    source.contains("anthropic"),
                    "a user must be able to tell the question left the machine: {source}"
                );
            }
            other => panic!("expected an answer, got {other:?}"),
        }
        assert!(reply.pending.is_none(), "prose needs no approval");
    }

    #[test]
    fn a_local_answer_says_it_stayed_on_the_machine() {
        let out = json_obj([(
            "results",
            arr(vec![json_obj([(
                "value",
                json_obj([
                    ("answer", "42".into()),
                    ("assistant", "nous".into()),
                    ("local", true.into()),
                ]),
            )])]),
        )]);
        match read_submission(&out).body {
            Body::Answer { source, .. } => {
                assert!(source.contains("on this machine"), "got {source}")
            }
            other => panic!("expected an answer, got {other:?}"),
        }
    }

    #[test]
    fn a_curator_proposal_comes_back_with_a_way_to_approve_it() {
        let value = json_obj([
            ("bytes", 4096u64.into()),
            ("summary", "tidy Downloads".into()),
            (
                "steps",
                arr(vec![
                    json_obj([
                        ("capability", "fs.move:~/Downloads/a".into()),
                        ("summary", "move a".into()),
                    ]),
                    json_obj([
                        ("capability", "fs.delete:~/Downloads/b".into()),
                        ("summary", "remove b".into()),
                    ]),
                ]),
            ),
        ]);
        let out = json_obj([("results", arr(vec![json_obj([("value", value.clone())])]))]);

        let reply = read_submission(&out);
        match &reply.body {
            Body::Proposal { headline, steps } => {
                assert!(headline.contains("tidy Downloads"), "got {headline}");
                assert_eq!(steps.len(), 2);
                // Risk comes from the capability table, so a move and a delete
                // are not painted the same.
                assert_eq!(steps[0].risk, Risk::Write);
                assert_eq!(steps[1].risk, Risk::Elevated);
            }
            other => panic!("expected a proposal, got {other:?}"),
        }
        assert_eq!(
            reply.pending,
            Some(Pending::Curate(value)),
            "a proposal with no way to approve it is the bug this exists to prevent"
        );
    }

    #[test]
    fn findings_without_concrete_steps_are_not_mistaken_for_a_proposal() {
        // The curator reports what it found before it proposes anything. That
        // report has no `bytes` and no steps to apply.
        let findings = json_obj([("findings", arr(vec![json_obj([("path", "~/x".into())])]))]);
        assert!(!is_curate_proposal(&findings));
        // Steps without a byte count are a plan, not a proposal to apply.
        assert!(!is_curate_proposal(&json_obj([(
            "steps",
            arr(vec![json_obj([
                ("capability", "fs.move:~/a".into()),
                ("summary", "move".into()),
            ])])
        )])));
        // An empty step list is nothing to approve.
        assert!(!is_curate_proposal(&json_obj([
            ("steps", arr(vec![])),
            ("bytes", 0u64.into()),
        ])));
        // A step missing its summary cannot be shown honestly, so it is not
        // treated as approvable.
        assert!(!is_curate_proposal(&json_obj([
            ("bytes", 1u64.into()),
            (
                "steps",
                arr(vec![json_obj([("capability", "fs.move:~/a".into())])])
            ),
        ])));
    }

    #[test]
    fn a_failed_result_becomes_an_error_not_a_success() {
        let out = json_obj([(
            "results",
            arr(vec![
                json_obj([("state", "ok".into()), ("detail", "read 3 files".into())]),
                json_obj([("state", "denied".into()), ("detail", "not allowed".into())]),
            ]),
        )]);
        match read_submission(&out).body {
            Body::Error { message } => {
                assert!(message.contains("denied"), "got {message}");
                assert!(message.contains("not allowed"));
            }
            other => panic!("a denied step must not report success: {other:?}"),
        }
    }

    #[test]
    fn undo_is_only_offered_when_there_is_something_to_undo() {
        let ok = |revertible: bool| {
            json_obj([
                ("revertible", revertible.into()),
                (
                    "results",
                    arr(vec![json_obj([
                        ("state", "ok".into()),
                        ("detail", "moved 3 files".into()),
                    ])]),
                ),
            ])
        };
        match read_submission(&ok(true)).body {
            Body::Done { undo_hint, .. } => assert!(undo_hint),
            other => panic!("expected done, got {other:?}"),
        }
        match read_submission(&ok(false)).body {
            Body::Done { undo_hint, .. } => {
                assert!(!undo_hint, "offering an undo that cannot happen is a lie")
            }
            other => panic!("expected done, got {other:?}"),
        }
    }

    #[test]
    fn a_refusal_says_which_capability_was_refused() {
        let pf = json_obj([
            ("blocked", true.into()),
            (
                "steps",
                arr(vec![
                    json_obj([
                        ("capability", "fs.read:~/a".into()),
                        ("decision", "allow".into()),
                    ]),
                    json_obj([
                        ("capability", "fs.delete:/boot/vmlinuz".into()),
                        ("decision", "deny".into()),
                        ("reason", "protected path".into()),
                    ]),
                ]),
            ),
        ]);
        let msg = refusal(&pf);
        assert!(msg.contains("fs.delete:/boot/vmlinuz"), "got {msg}");
        assert!(msg.contains("protected path"), "got {msg}");
    }

    #[test]
    fn plan_steps_use_the_risk_the_daemon_evaluated() {
        let pf = json_obj([(
            "steps",
            arr(vec![json_obj([
                ("capability", "fs.move:~/a".into()),
                ("summary", "move a".into()),
                // The policy escalated this beyond the capability's own level;
                // the panel must show what was actually decided.
                ("risk", "critical".into()),
            ])]),
        )]);
        let steps = plan_steps(&pf);
        assert_eq!(steps[0].risk, Risk::Critical);
    }

    #[test]
    fn a_result_shows_what_it_produced_not_only_that_it_happened() {
        // The bug: asking "what is my disk usage" answered "sampled machine
        // metrics" and showed none of them.
        let out = json_obj([(
            "results",
            arr(vec![json_obj([
                ("state", "ok".into()),
                ("detail", "sampled machine metrics".into()),
                (
                    "value",
                    json_obj([
                        ("load1", 0.44.into()),
                        ("mem_used_pct", 4.0.into()),
                        ("disk_used_pct", 88.0.into()),
                    ]),
                ),
            ])]),
        )]);
        match read_submission(&out).body {
            Body::Done {
                headline, detail, ..
            } => {
                assert_eq!(headline, "sampled machine metrics");
                assert!(
                    detail.contains("88"),
                    "the disk figure is missing: {detail}"
                );
                assert!(
                    detail.contains("0.44"),
                    "the load figure is missing: {detail}"
                );
            }
            other => panic!("expected done, got {other:?}"),
        }
    }

    #[test]
    fn a_listing_shows_its_entries_and_says_what_it_left_out() {
        let entries: Vec<Json> = (0..30)
            .map(|i| {
                json_obj([
                    ("name", format!("file{i}.txt").into()),
                    ("size", 2048u64.into()),
                ])
            })
            .collect();
        let out = json_obj([(
            "results",
            arr(vec![json_obj([
                ("state", "ok".into()),
                ("value", json_obj([("entries", arr(entries))])),
            ])]),
        )]);
        match read_submission(&out).body {
            Body::Done { detail, .. } => {
                assert!(detail.contains("file0.txt"));
                assert!(detail.contains("2.0 KB"), "sizes are unreadable: {detail}");
                // Truncation that does not say it truncated reads as a
                // complete list.
                assert!(detail.contains("18 more"), "left out silently: {detail}");
            }
            other => panic!("expected done, got {other:?}"),
        }
    }

    #[test]
    fn a_directory_is_marked_as_one_and_carries_no_size() {
        let v = json_obj([(
            "entries",
            arr(vec![
                json_obj([("name", "Pictures".into()), ("is_dir", true.into())]),
                json_obj([("name", "a.txt".into()), ("size", 10u64.into())]),
            ]),
        )]);
        let shown = describe(&v).expect("described");
        assert!(shown.contains("Pictures/"), "got {shown}");
        assert!(
            !shown.contains("Pictures  ("),
            "a folder was given a size: {shown}"
        );
        assert!(shown.contains("a.txt  (10 B)"), "got {shown}");
    }

    #[test]
    fn an_empty_listing_says_so_rather_than_showing_nothing() {
        assert_eq!(
            describe(&json_obj([("entries", arr(vec![]))])).as_deref(),
            Some("nothing here")
        );
    }

    #[test]
    fn a_shape_with_nothing_to_say_adds_no_blank_line() {
        assert_eq!(describe(&Json::Null), None);
        assert_eq!(describe(&json_obj([("ok", true.into())])), None);
        // An empty findings list is not "nothing here": the step that produced
        // it already said what it did.
        assert_eq!(describe(&json_obj([("findings", arr(vec![]))])), None);
    }

    /// Sizes come from nous-core so every surface agrees. These cases pin the
    /// behaviour this module depends on, in particular that a very large number
    /// does not come out in the wrong unit.
    #[test]
    fn sizes_read_in_units_a_person_uses() {
        assert_eq!(human(0), "0 B");
        assert_eq!(human(512), "512 B");
        assert_eq!(human(1024), "1.0 KB");
        assert_eq!(human(1536), "1.5 KB");
        assert_eq!(human(1024 * 1024 * 3), "3.0 MB");
        // Past the last unit it must keep counting, not wrap or panic.
        assert_eq!(human(u64::MAX), "16.0 EB");
    }

    #[test]
    fn an_empty_response_is_reported_as_done_rather_than_as_nothing() {
        let reply = read_submission(&Json::obj());
        match reply.body {
            Body::Done { headline, .. } => assert!(!headline.is_empty()),
            other => panic!("expected done, got {other:?}"),
        }
    }
}
