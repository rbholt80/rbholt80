//! The line to the daemon.
//!
//! The window can read the disk by itself — listing a folder needs no help. It
//! cannot *change* the disk by itself, and should not want to: every move,
//! rename and deletion goes through the broker, which adjudicates it against
//! policy, records how to undo it, and journals what happened. A file manager
//! that called `std::fs::rename` directly would be a file manager whose
//! mistakes are permanent.
//!
//! The connection is made when it is needed and dropped when it fails, so a
//! daemon started after the window is picked up without a restart, and one that
//! goes away leaves the window working for everything it can still do alone.

use nous_core::ipc::Client;
use nous_core::json::{json_obj, Json};
use nous_core::proto::method;
use std::time::{Duration, Instant};

/// Short: this runs on the keystroke that renames a file, and a daemon that has
/// wedged should surface as an error in the status bar rather than as a window
/// that has stopped repainting.
const TIMEOUT: Duration = Duration::from_millis(1500);

/// How long to wait before trying again after a failed connection. Without
/// this, every frame drawn while the daemon is down attempts a fresh connect.
const RETRY_AFTER: Duration = Duration::from_secs(3);

pub struct Link {
    client: Option<Client>,
    /// Where to connect. `None` means wherever the daemon normally listens.
    ///
    /// Named so a test can point at a socket that certainly has nothing behind
    /// it: a suite whose results depend on whether a daemon happens to be
    /// running on the machine is a suite that passes for the wrong reason.
    socket: Option<std::path::PathBuf>,
    last_try: Option<Instant>,
    /// What went wrong last, for the status bar to say.
    pub trouble: Option<String>,
    /// How many requests the daemon has carried out for this window.
    ///
    /// Anything watching the journal compares this against what it last saw,
    /// rather than each caller remembering to say it changed something. A
    /// ledger that is stale is worse than one that is missing: it says the
    /// thing you just did did not happen.
    pub changes: u64,
}

impl Default for Link {
    fn default() -> Link {
        Link::new()
    }
}

impl Link {
    /// A link to a named socket, for tests and for anyone running a daemon
    /// somewhere other than where one normally lives.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn to_socket(path: impl Into<std::path::PathBuf>) -> Link {
        Link {
            socket: Some(path.into()),
            ..Link::new()
        }
    }

    pub fn new() -> Link {
        Link {
            client: None,
            socket: None,
            last_try: None,
            trouble: None,
            changes: 0,
        }
    }

    /// Whether the daemon answered the last time anything was asked of it.
    ///
    /// Deliberately not a live probe: a status light that pings every frame
    /// costs a round trip per repaint to say something that only changes when a
    /// request is actually made.
    pub fn connected(&self) -> bool {
        self.client.is_some()
    }

    fn ensure(&mut self) -> Result<&mut Client, String> {
        if self.client.is_none() {
            if let Some(t) = self.last_try {
                if t.elapsed() < RETRY_AFTER {
                    return Err(self
                        .trouble
                        .clone()
                        .unwrap_or_else(|| "no daemon".to_string()));
                }
            }
            self.last_try = Some(Instant::now());
            let connected = match &self.socket {
                Some(p) => Client::connect_to(p),
                None => Client::connect(),
            };
            match connected {
                Ok(c) => {
                    let _ = c.set_timeout(Some(TIMEOUT));
                    self.client = Some(c);
                    self.trouble = None;
                }
                Err(e) => {
                    self.trouble = Some(short(&e));
                    return Err(short(&e));
                }
            }
        }
        Ok(self.client.as_mut().expect("just connected"))
    }

    /// Run one capability, approved, and give back what it said.
    ///
    /// `why` is the sentence the journal records, so an entry reads as
    /// something a person did rather than as a capability string.
    pub fn invoke(&mut self, cap: &str, args: Json, why: &str) -> Result<Json, String> {
        let params = json_obj([
            ("capability", cap.into()),
            ("args", args),
            ("why", why.into()),
            ("approved", Json::Bool(true)),
        ]);
        let out = match self.ensure()?.call("cap.invoke", params) {
            Ok(v) => v,
            Err(e) => {
                // A broken pipe is not a failed operation, it is a lost
                // connection: drop it so the next attempt reconnects instead of
                // failing forever against a dead socket.
                self.client = None;
                self.trouble = Some(short(&e));
                return Err(short(&e));
            }
        };
        // The broker answers with a run report. A step that failed is a failure
        // even though the call succeeded, and saying "done" here would be the
        // window's own lie rather than the daemon's.
        if let Some(msg) = first_failure(&out) {
            self.trouble = Some(msg.clone());
            return Err(msg);
        }
        self.trouble = None;
        self.changes += 1;
        Ok(out)
    }

    /// Ask for something and accept not getting it.
    ///
    /// For the things drawn every so often — what is playing, what the curator
    /// thinks — where the honest answer when the daemon is down is to draw
    /// nothing rather than to interrupt.
    pub fn ask(&mut self, cap: &str, args: Json) -> Option<Json> {
        self.invoke(cap, args, cap).ok()
    }

    /// Take one journal entry back.
    ///
    /// Its own method rather than a capability call: reverting is a daemon
    /// method, and it runs through the broker on the far side — so an undo is
    /// itself written down, which is how the ledger can show that something
    /// was undone and by what.
    pub fn call_journal_revert(&mut self, params: Json) -> Result<Json, String> {
        let out = match self.ensure()?.call(method::JOURNAL_REVERT, params) {
            Ok(v) => v,
            Err(e) => {
                self.client = None;
                self.trouble = Some(short(&e));
                return Err(short(&e));
            }
        };
        if let Some(msg) = first_failure(&out) {
            self.trouble = Some(msg.clone());
            return Err(msg);
        }
        self.trouble = None;
        self.changes += 1;
        Ok(out)
    }

    /// Ask what it would take to do something, without doing any of it.
    pub fn call_intent_plan(&mut self, params: Json) -> Result<Json, String> {
        self.method(method::INTENT_PLAN, params)
    }

    /// Run the plan that was shown. Takes the plan document rather than the
    /// words, so what runs is what was agreed to.
    pub fn call_intent_confirm(&mut self, params: Json) -> Result<Json, String> {
        self.method(method::INTENT_CONFIRM, params)
    }

    fn method(&mut self, name: &str, params: Json) -> Result<Json, String> {
        let out = match self.ensure()?.call(name, params) {
            Ok(v) => v,
            Err(e) => {
                self.client = None;
                self.trouble = Some(short(&e));
                return Err(short(&e));
            }
        };
        if let Some(msg) = first_failure(&out) {
            self.trouble = Some(msg.clone());
            return Err(msg);
        }
        self.trouble = None;
        self.changes += 1;
        Ok(out)
    }

    /// Is anyone there?
    ///
    /// A method rather than a capability, deliberately: capabilities go
    /// through policy, and asking "are you alive" in a way policy can refuse
    /// means a perfectly healthy daemon reports itself unreachable. Which it
    /// did — the first liveness check here asked for `sys.status` as a
    /// capability and got "no rule permits sys.status for user".
    pub fn ping(&mut self) -> bool {
        let Ok(c) = self.ensure() else { return false };
        match c.call(method::PING, Json::obj()) {
            Ok(_) => true,
            Err(e) => {
                self.client = None;
                self.trouble = Some(short(&e));
                false
            }
        }
    }

    /// Search the index the daemon keeps of every file it has been shown.
    ///
    /// Runs on every keystroke, so a failure is silent and the caller carries
    /// on with what it can find without help. Nothing is changed by looking,
    /// which is why this does not count as a change.
    pub fn search(&mut self, query: &str, limit: u64) -> Option<Json> {
        let params = json_obj([("query", query.into()), ("limit", limit.into())]);
        let c = self.ensure().ok()?;
        match c.call(method::FS_SEARCH, params) {
            Ok(v) => Some(v),
            Err(e) => {
                self.client = None;
                self.trouble = Some(short(&e));
                None
            }
        }
    }

    /// The last few things that were done, for the view that lists them.
    pub fn journal(&mut self, limit: u64) -> Option<Json> {
        let c = self.ensure().ok()?;
        match c.call(method::JOURNAL_TAIL, json_obj([("limit", limit.into())])) {
            Ok(v) => Some(v),
            Err(e) => {
                self.client = None;
                self.trouble = Some(short(&e));
                None
            }
        }
    }
}

/// What the broker actually answers with.
///
/// A run report holds `results`, one per step, each carrying `state` — "ok",
/// or a word saying what went wrong — and `value`, the step's own answer.
///
/// This is written down once, in one place, because guessing it is exactly
/// what went wrong: three callers each invented a slightly different shape
/// (`steps[0].result`, a top-level `ok` flag) and every one of them silently
/// read nothing. Nothing failed, nothing was drawn, and no error said so.
pub mod report {
    use nous_core::json::Json;

    /// The answer the first step gave, or `Json::Null`.
    ///
    /// Falls back to the report itself, so a caller handed an already-unwrapped
    /// value still works.
    pub fn value(report: &Json) -> Json {
        report
            .arr_or_empty("results")
            .first()
            .and_then(|s| s.get("value"))
            .cloned()
            .unwrap_or_else(|| report.clone())
    }

    /// Whether every step succeeded.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn ok(report: &Json) -> bool {
        first_failure(report).is_none()
    }

    /// Why it did not, in the daemon's own words.
    ///
    /// A refused step comes back inside a perfectly good reply, so reading
    /// only the envelope calls a rename that did not happen a success.
    pub fn first_failure(report: &Json) -> Option<String> {
        if let Some(e) = report.get("error").and_then(|v| v.as_str()) {
            if !e.is_empty() {
                return Some(e.to_string());
            }
        }
        for s in report.arr_or_empty("results") {
            let state = s.str_or("state", "ok");
            if state == "ok" {
                continue;
            }
            // "detail" is the sentence a person should read; the state word is
            // the fallback when there is no sentence.
            let msg = s
                .get("detail")
                .and_then(|v| v.as_str())
                .filter(|d| !d.is_empty())
                .unwrap_or(state);
            return Some(msg.to_string());
        }
        // A run that was stopped before any step ran says so at the top.
        //
        // The words are the daemon's, not a guess at them: "completed" is what
        // a finished run says, and reading it as a failure — which the first
        // version of this did, having assumed "ok" — makes every successful
        // request look refused.
        let status = report.str_or("status", "");
        match status {
            "" | "completed" => None,
            "needs_approval" => Some("it needs approving first".to_string()),
            other => {
                let msg = report.str_or("message", "");
                Some(if msg.is_empty() {
                    format!("the daemon {other} the request")
                } else {
                    msg.to_string()
                })
            }
        }
    }
}

pub use report::first_failure;

/// One line, for a status bar that has one line.
///
/// The connect error carries advice about running `nousctl status`, which is
/// right for a terminal and useless in a window.
fn short(e: &str) -> String {
    let first = e.split(" (").next().unwrap_or(e);
    let first = first.split(". ").next().unwrap_or(first);
    let mut s = first.trim().to_string();
    if s.starts_with("cannot reach nousd") {
        s = "no daemon running".to_string();
    }
    if s.len() > 90 {
        s.truncate(89);
        s.push('…');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use nous_core::json::parse;

    /// A real reply, copied off a running daemon rather than imagined.
    ///
    /// The tests that stood here passed against a shape I had made up —
    /// `steps[0].result`, a top-level `ok` flag — which the daemon has never
    /// sent. They passed, and the interface read nothing, and nothing said so.
    const REAL: &str = r#"{
        "dry_run": false, "intent_id": "i4", "message": "", "status": "completed",
        "plan": {"steps": [{"capability": "curate.scan", "id": "s1"}]},
        "results": [{
            "capability": "curate.scan", "id": "s1", "seq": 20, "state": "ok",
            "detail": "found 4 things to tidy (732.4 KB reclaimable)",
            "summary": "curate.scan",
            "value": {"count": 4, "reclaimable": "732.4 KB", "findings": [
                {"kind": "duplicate", "severity": 4, "paths": ["/d/a.zip", "/d/b.zip"]}
            ]}
        }]
    }"#;

    #[test]
    fn a_steps_answer_is_read_out_of_the_shape_the_daemon_actually_sends() {
        let r = parse(REAL).unwrap();
        let v = report::value(&r);
        assert_eq!(
            v.f64_or("count", -1.0),
            4.0,
            "read nothing out of a real reply: {v}"
        );
        assert_eq!(v.arr_or_empty("findings").len(), 1);
        assert!(report::ok(&r));
        assert_eq!(first_failure(&r), None);
    }

    #[test]
    fn a_run_whose_step_failed_is_a_failure_however_the_call_went() {
        // The broker answers a refused operation with a perfectly good reply
        // describing the refusal. Reading only the outer envelope reports
        // success for a rename that did not happen.
        let r = parse(
            r#"{"status":"ok","results":[{"state":"failed","detail":"/home/j/b.txt already exists"}]}"#,
        )
        .unwrap();
        assert_eq!(
            first_failure(&r).as_deref(),
            Some("/home/j/b.txt already exists")
        );
        assert!(!report::ok(&r));
    }

    #[test]
    fn a_step_that_failed_without_a_sentence_still_says_something() {
        let r = parse(r#"{"results":[{"state":"refused"}]}"#).unwrap();
        assert_eq!(first_failure(&r).as_deref(), Some("refused"));
    }

    #[test]
    fn a_finished_run_is_not_read_as_a_refusal() {
        // The daemon says "completed". Reading anything that is not "ok" as a
        // failure made every successful request look refused — including the
        // curator, whose findings then never reached the folder.
        let r = parse(r#"{"status":"completed","results":[{"state":"ok","value":{"count":4}}]}"#)
            .unwrap();
        assert_eq!(first_failure(&r), None, "a completed run read as a refusal");
        assert_eq!(report::value(&r).f64_or("count", -1.0), 4.0);
    }

    #[test]
    fn a_run_refused_before_any_step_ran_says_so() {
        let r =
            parse(r#"{"status":"blocked","message":"policy forbids fs.delete here","results":[]}"#)
                .unwrap();
        assert_eq!(
            first_failure(&r).as_deref(),
            Some("policy forbids fs.delete here")
        );
        let r = parse(r#"{"status":"blocked","results":[]}"#).unwrap();
        assert!(first_failure(&r).unwrap().contains("blocked"));
        // And a plan waiting on a person is not a failure of the daemon's.
        let r = parse(r#"{"status":"needs_approval","results":[]}"#).unwrap();
        assert!(first_failure(&r).unwrap().contains("approv"));
    }

    #[test]
    fn an_already_unwrapped_value_is_left_alone() {
        // Callers holding a step's own answer should not have to know whether
        // it arrived wrapped.
        let v = parse(r#"{"count":2,"findings":[]}"#).unwrap();
        assert_eq!(report::value(&v).f64_or("count", -1.0), 2.0);
    }

    #[test]
    fn the_connect_advice_meant_for_a_terminal_is_not_shown_in_a_window() {
        let e = "cannot reach nousd at /run/nous.sock (No such file). Is the daemon running? Try: nousctl status";
        assert_eq!(short(e), "no daemon running");
    }

    #[test]
    fn a_long_complaint_is_cut_to_something_a_status_bar_can_hold() {
        let e = "x".repeat(400);
        assert!(short(&e).chars().count() <= 90, "{}", short(&e).len());
    }

    #[test]
    fn a_link_with_no_daemon_is_not_connected_and_does_not_panic() {
        let mut l = Link::new();
        assert!(!l.connected());
        // Whatever is or is not listening, this must come back rather than hang.
        let _ = l.ask("desk.session_info", Json::obj());
        assert!(l.trouble.is_some() || l.connected());
    }
}
