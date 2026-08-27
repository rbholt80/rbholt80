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
    last_try: Option<Instant>,
    /// What went wrong last, for the status bar to say.
    pub trouble: Option<String>,
}

impl Default for Link {
    fn default() -> Link {
        Link::new()
    }
}

impl Link {
    pub fn new() -> Link {
        Link {
            client: None,
            last_try: None,
            trouble: None,
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
            match Client::connect() {
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

    /// The last few things that were done, for the view that lists them with
    /// a way to undo each. That view is not built yet; this is what it reads.
    #[allow(dead_code)]
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

/// Pull the reason out of a broker run report, if a step failed.
///
/// The report nests: a run holds steps, each with its own ok flag and message.
/// Reading only the top level reports success for a run whose every step failed.
pub fn first_failure(report: &Json) -> Option<String> {
    if report.bool_or("ok", true) {
        // Some replies carry no top-level flag at all, so the steps are still
        // worth walking.
    } else if let Some(e) = report.get("error").and_then(|v| v.as_str()) {
        return Some(e.to_string());
    }
    for s in report.arr_or_empty("steps") {
        if !s.bool_or("ok", true) {
            let msg = s
                .get("error")
                .or_else(|| s.get("detail"))
                .or_else(|| s.get("summary"))
                .and_then(|v| v.as_str())
                .unwrap_or("the daemon refused it");
            return Some(msg.to_string());
        }
    }
    if !report.bool_or("ok", true) {
        return Some("the daemon refused it".to_string());
    }
    None
}

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

    #[test]
    fn a_run_whose_step_failed_is_a_failure_however_the_call_went() {
        // The broker answers a refused operation with a perfectly good reply
        // describing the refusal. Reading only the outer envelope reports
        // success for a rename that did not happen.
        let r =
            parse(r#"{"ok":true,"steps":[{"ok":false,"error":"/home/j/b.txt already exists"}]}"#)
                .unwrap();
        assert_eq!(
            first_failure(&r).as_deref(),
            Some("/home/j/b.txt already exists")
        );
    }

    #[test]
    fn a_run_that_worked_reports_nothing_wrong() {
        let r = parse(r#"{"ok":true,"steps":[{"ok":true,"summary":"moved a to b"}]}"#).unwrap();
        assert_eq!(first_failure(&r), None);
    }

    #[test]
    fn a_refusal_with_no_steps_still_says_something() {
        let r = parse(r#"{"ok":false}"#).unwrap();
        assert!(first_failure(&r).is_some(), "a refusal read as success");
        let r = parse(r#"{"ok":false,"error":"policy forbids fs.delete here"}"#).unwrap();
        assert_eq!(
            first_failure(&r).as_deref(),
            Some("policy forbids fs.delete here")
        );
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
