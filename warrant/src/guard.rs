//! The thing you actually hold.
//!
//! A `Guard` sits between something that proposes actions and the code that
//! performs them. It answers whether an action may happen, and it will not let
//! you record that it happened without first recording how to take it back.
//!
//! ```no_run
//! use warrant::{Guard, Subject, Capability, Undo, Outcome, json_obj};
//!
//! # fn main() -> Result<(), String> {
//! # let guard: Guard = unimplemented!();
//! let cap = Capability::parse("fs.write:/home/robert/notes.md")?;
//! let ruling = guard.rule(&Subject::Agent("claude".into()), &cap);
//!
//! if !ruling.allowed() {
//!     println!("{}", ruling.explain());   // says which line refused, and why
//!     guard.refuse(&ruling, "asked to overwrite notes")?;
//!     return Ok(());
//! }
//!
//! // The undo goes to disk here, before anything is touched.
//! let pending = guard.begin(&ruling, "save the draft",
//!     Undo::new("restore notes.md", json_obj([("backup", "/var/b/7".into())])))?;
//!
//! // ... do the actual work ...
//!
//! pending.finish(Outcome::Ok, "wrote 412 bytes")?;
//! # Ok(())
//! # }
//! ```
//!
//! The ordering is not a convention you have to remember. There is no way to
//! get a [`Pending`] without the undo having been written and flushed, and no
//! way to close a record without a `Pending`.
//!
//! # What this crate will not do
//!
//! It never performs an action, and it never reverses one. It decides, it
//! records, and it hands the reversal back to you. A component that both judges
//! and acts is one bug away from doing neither properly, and a component that
//! cannot touch anything is one you can read in an afternoon and then stop
//! worrying about.

use std::path::Path;

use crate::cap::Capability;
use crate::grade::{Grades, Risk};
use crate::journal::{Act, Journal, Outcome, Record, Undo};
use crate::policy::{Decision, Policy, Subject, Verdict};

/// A decision about one request, and the request it was about.
#[derive(Debug, Clone)]
pub struct Ruling {
    pub subject: Subject,
    pub capability: Capability,
    pub verdict: Verdict,
}

impl Ruling {
    /// May this proceed with no further ceremony?
    pub fn allowed(&self) -> bool {
        self.verdict.is_allow()
    }

    /// May this proceed, but only after a human says so?
    pub fn needs_confirmation(&self) -> bool {
        matches!(self.verdict.decision, Decision::Confirm(_))
    }

    /// Is this refused outright?
    pub fn refused(&self) -> bool {
        matches!(self.verdict.decision, Decision::Deny(_))
    }

    /// Refused by a `never` line, which no confirmation can lift.
    pub fn forbidden(&self) -> bool {
        self.verdict.absolute
    }

    pub fn risk(&self) -> Risk {
        self.verdict.risk
    }

    /// Why, citing the line that decided.
    pub fn explain(&self) -> String {
        format!("{}: {}", self.capability, self.verdict.explain())
    }

    /// The sentence to put in front of a person when asking them to confirm.
    pub fn prompt(&self) -> String {
        format!(
            "{} wants to {} ({} risk){}",
            self.subject.label(),
            self.capability,
            self.verdict.risk,
            match self.verdict.decision.reason() {
                "" => String::new(),
                r => format!(" — {}", r),
            }
        )
    }
}

/// An action that has been authorised and written down, but not yet reported
/// finished.
///
/// Dropping this without calling [`Pending::finish`] is not an error and does
/// not panic — it leaves the journal saying the action was begun and never
/// completed, which is the honest description of a process that died holding
/// one. Those come back from [`Guard::unfinished`].
#[must_use = "an authorised action should be finished, or the journal will say it never completed"]
pub struct Pending<'a> {
    guard: &'a Guard,
    seq: u64,
}

impl std::fmt::Debug for Pending<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Pending(#{})", self.seq)
    }
}

impl Pending<'_> {
    /// The journal sequence number of this action.
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// Record how it went.
    pub fn finish(self, outcome: Outcome, detail: &str) -> Result<u64, String> {
        self.guard.journal.end(self.seq, outcome, detail)?;
        Ok(self.seq)
    }
}

/// Policy, grades and journal, held together.
pub struct Guard {
    policy: Policy,
    grades: Grades,
    journal: Journal,
    home: String,
}

impl Guard {
    pub fn new(policy: Policy, grades: Grades, journal: Journal, home: &str) -> Guard {
        Guard {
            policy,
            grades,
            journal,
            home: home.to_string(),
        }
    }

    /// Open a guard with its journal in `dir`.
    ///
    /// `home` is read from the environment. Tests and daemons that serve more
    /// than one user should use [`Guard::new`] and say which home they mean,
    /// rather than depending on a process-global variable.
    pub fn open(dir: &Path, policy: Policy, grades: Grades) -> Result<Guard, String> {
        let journal = Journal::open(dir)?;
        let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
        Ok(Guard::new(policy, grades, journal, &home))
    }

    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    pub fn grades(&self) -> &Grades {
        &self.grades
    }

    pub fn journal(&self) -> &Journal {
        &self.journal
    }

    /// Adjudicate one request. Changes nothing and records nothing.
    ///
    /// Safe to call as often as you like — to grey out a button, to preflight a
    /// whole plan before running any of it, to show a person what an agent is
    /// about to attempt.
    pub fn rule(&self, subject: &Subject, capability: &Capability) -> Ruling {
        let verdict = self
            .policy
            .evaluate(subject, capability, &self.grades, &self.home);
        Ruling {
            subject: subject.clone(),
            capability: capability.clone(),
            verdict,
        }
    }

    /// Adjudicate every step of a plan before any of it runs.
    ///
    /// An agent's plan that will be refused at step 9 should be refused at step
    /// 0, before the first eight have half-changed the machine.
    pub fn preflight(&self, subject: &Subject, plan: &[Capability]) -> Vec<Ruling> {
        plan.iter().map(|c| self.rule(subject, c)).collect()
    }

    /// Authorise an allowed action and write its undo to disk.
    ///
    /// Fails if the ruling was not a plain allow. A ruling that needs a human
    /// goes through [`Guard::begin_confirmed`] instead, so that "a person said
    /// yes" is a distinct, recorded fact rather than an inference.
    pub fn begin(&self, ruling: &Ruling, intent: &str, undo: Undo) -> Result<Pending<'_>, String> {
        if !ruling.allowed() {
            return Err(format!("not authorised — {}", ruling.explain()));
        }
        self.write_act(ruling, ruling.verdict.decision.kind(), intent, &undo)
    }

    /// Authorise an action that policy said needed a human, after one said yes.
    ///
    /// `who` is recorded, because "who approved this" is the first question
    /// asked about anything that goes wrong.
    ///
    /// A `never` line cannot be confirmed past. That is the entire difference
    /// between `never` and `deny`, and it is enforced here.
    pub fn begin_confirmed(
        &self,
        ruling: &Ruling,
        who: &str,
        intent: &str,
        undo: Undo,
    ) -> Result<Pending<'_>, String> {
        if ruling.forbidden() {
            return Err(format!(
                "cannot be confirmed — {}",
                ruling.verdict.explain()
            ));
        }
        if !ruling.needs_confirmation() && !ruling.allowed() {
            return Err(format!("not authorised — {}", ruling.explain()));
        }
        let decision = format!("confirmed by {}", who);
        self.write_act(ruling, &decision, intent, &undo)
    }

    /// Record that a request was refused, so the refusal is in the history too.
    ///
    /// Journalling only what succeeded produces a log that cannot answer "what
    /// has this thing been trying to do?", which is the question that catches a
    /// misbehaving agent early.
    pub fn refuse(&self, ruling: &Ruling, intent: &str) -> Result<u64, String> {
        let seq = self.write_seq(
            ruling,
            ruling.verdict.decision.kind(),
            intent,
            &Undo::none(),
        )?;
        self.journal
            .end(seq, Outcome::Refused, ruling.verdict.decision.reason())?;
        Ok(seq)
    }

    fn write_act(
        &self,
        ruling: &Ruling,
        decision: &str,
        intent: &str,
        undo: &Undo,
    ) -> Result<Pending<'_>, String> {
        let seq = self.write_seq(ruling, decision, intent, undo)?;
        Ok(Pending { guard: self, seq })
    }

    fn write_seq(
        &self,
        ruling: &Ruling,
        decision: &str,
        intent: &str,
        undo: &Undo,
    ) -> Result<u64, String> {
        self.journal.begin(&Act {
            subject: &ruling.subject.label(),
            capability: &ruling.capability.to_string(),
            risk: ruling.verdict.risk.as_str(),
            decision,
            matched: ruling.verdict.matched.as_deref().unwrap_or("default"),
            intent,
            undo,
        })
    }

    /// The newest action that can still be taken back.
    pub fn undoable(&self) -> Result<Option<Record>, String> {
        self.journal.last_revertible()
    }

    /// Claim the undo for `seq`, so a host can perform it.
    ///
    /// Returns the instructions the host stored. Marking it done is
    /// [`Guard::reverted`], called *after* the reversal actually worked — a
    /// failed undo must not leave the journal claiming the action was taken
    /// back.
    pub fn take_undo(&self, seq: u64) -> Result<Undo, String> {
        let rec = self
            .journal
            .get(seq)?
            .ok_or_else(|| format!("no action #{} in the journal", seq))?;
        if rec.undone_by.is_some() {
            return Err(format!("#{} was already taken back", seq));
        }
        if !rec.undo.is_automatic() {
            return Err(format!(
                "#{} cannot be undone: {}",
                seq,
                rec.undo.describe()
            ));
        }
        Ok(rec.undo)
    }

    /// Record that `seq` was successfully reversed, by action `by`.
    pub fn reverted(&self, seq: u64, by: u64) -> Result<(), String> {
        self.journal.mark_reverted(seq, by)
    }

    /// Actions begun and never reported finished. Show these on startup.
    pub fn unfinished(&self) -> Result<Vec<Record>, String> {
        self.journal.unfinished()
    }

    /// Recent history, oldest first.
    pub fn history(&self, n: usize) -> Result<Vec<Record>, String> {
        self.journal.tail(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::json_obj;
    use std::fs;
    use std::path::PathBuf;

    struct Dir(PathBuf);
    impl Dir {
        fn new(tag: &str) -> Dir {
            let p = std::env::temp_dir().join(format!(
                "warrant-guard-{}-{}-{:?}",
                tag,
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            Dir(p)
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const POLICY: &str = "\
never   *        fs.read:/**/.ssh/**   # keys stay on the disk
deny    agent:*  pkg.install           # a human installs software
confirm agent:*  fs.delete:~/**        # say it out loud
allow   agent:*  fs.write:~/**
allow   agent:*  fs.read:~/**
allow   user     fs.*:~/**
";

    const GRADES: &str = "\
read     fs.read fs.list
write    fs.write
elevated fs.delete pkg.install
";

    fn guard(d: &Dir) -> Guard {
        Guard::new(
            Policy::parse(POLICY, "policy").unwrap(),
            Grades::parse(GRADES, "grades").unwrap(),
            Journal::open(&d.0).unwrap(),
            "/home/robert",
        )
    }

    fn cap(s: &str) -> Capability {
        Capability::parse(s).unwrap()
    }

    fn agent() -> Subject {
        Subject::Agent("claude".into())
    }

    fn undo() -> Undo {
        Undo::new(
            "restore notes.md",
            json_obj([("backup", "/var/b/1".into())]),
        )
    }

    #[test]
    fn an_allowed_action_runs_and_is_recorded() {
        let d = Dir::new("allowed");
        let g = guard(&d);
        let r = g.rule(&agent(), &cap("fs.write:/home/robert/notes.md"));
        assert!(r.allowed(), "{}", r.explain());

        let p = g.begin(&r, "save the draft", undo()).unwrap();
        let seq = p.finish(Outcome::Ok, "wrote 412 bytes").unwrap();

        let rec = g.journal().get(seq).unwrap().unwrap();
        assert_eq!(rec.outcome, Outcome::Ok);
        assert_eq!(rec.subject, "agent:claude");
        assert_eq!(rec.intent, "save the draft");
        assert!(rec.is_revertible());
    }

    #[test]
    fn a_refused_action_cannot_be_begun() {
        let d = Dir::new("refused");
        let g = guard(&d);
        let r = g.rule(&agent(), &cap("pkg.install:htop"));
        assert!(r.refused(), "{}", r.explain());
        assert!(g.begin(&r, "install htop", undo()).is_err());
    }

    #[test]
    fn a_confirm_needs_the_confirmed_door_not_the_ordinary_one() {
        // begin() must not quietly accept a ruling that said "ask a human".
        // If it did, every caller that forgot to check would escalate silently.
        let d = Dir::new("confirm");
        let g = guard(&d);
        let r = g.rule(&agent(), &cap("fs.delete:/home/robert/old.txt"));
        assert!(r.needs_confirmation(), "{}", r.explain());

        assert!(g.begin(&r, "tidy up", undo()).is_err());

        let seq = g
            .begin_confirmed(&r, "robert", "tidy up", undo())
            .unwrap()
            .finish(Outcome::Ok, "deleted")
            .unwrap();
        assert_eq!(
            g.journal().get(seq).unwrap().unwrap().decision,
            "confirmed by robert"
        );
    }

    #[test]
    fn a_never_cannot_be_confirmed_past() {
        // The whole difference between `never` and `deny`. A human saying yes,
        // or an agent talking a human into saying yes, must not reach this.
        let d = Dir::new("never");
        let g = guard(&d);
        let r = g.rule(&agent(), &cap("fs.read:/home/robert/.ssh/id_rsa"));
        assert!(r.forbidden(), "{}", r.explain());

        assert!(g.begin(&r, "read the key", undo()).is_err());
        let err = g
            .begin_confirmed(&r, "robert", "read the key", undo())
            .unwrap_err();
        assert!(err.contains("cannot be confirmed"), "{}", err);
    }

    #[test]
    fn a_never_binds_the_human_as_well() {
        let d = Dir::new("neverhuman");
        let g = guard(&d);
        // The user has `allow fs.*:~/**`, written before the never in the file
        // — order does not save them.
        let r = g.rule(&Subject::User, &cap("fs.read:/home/robert/.ssh/id_rsa"));
        assert!(r.forbidden(), "{}", r.explain());
    }

    #[test]
    fn refusals_are_in_the_history_too() {
        // Otherwise the log cannot answer "what has this agent been trying to
        // do?", which is what catches one going wrong before it succeeds.
        let d = Dir::new("refusallog");
        let g = guard(&d);
        let r = g.rule(&agent(), &cap("pkg.install:htop"));
        g.refuse(&r, "wanted htop").unwrap();

        let h = g.history(10).unwrap();
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].outcome, Outcome::Refused);
        assert_eq!(h[0].capability, "pkg.install:htop");
        assert_eq!(h[0].intent, "wanted htop");
    }

    #[test]
    fn preflight_judges_the_whole_plan_before_any_of_it_runs() {
        let d = Dir::new("preflight");
        let g = guard(&d);
        let plan: Vec<Capability> = [
            "fs.read:/home/robert/a.md",
            "fs.write:/home/robert/b.md",
            "pkg.install:htop",
        ]
        .iter()
        .map(|s| cap(s))
        .collect();

        let rulings = g.preflight(&agent(), &plan);
        assert!(rulings[0].allowed() && rulings[1].allowed());
        assert!(rulings[2].refused());
        // And nothing was written: preflight is free of consequence.
        assert!(g.history(10).unwrap().is_empty());
    }

    #[test]
    fn the_undo_is_durable_before_the_caller_can_act() {
        // begin() returns only after the reversal is on disk. Read it back
        // through a second guard, which is what a recovering process is.
        let d = Dir::new("durable");
        let g = guard(&d);
        let r = g.rule(&agent(), &cap("fs.write:/home/robert/notes.md"));
        let p = g.begin(&r, "save", undo()).unwrap();

        let fresh = guard(&d);
        let rec = fresh.journal().get(p.seq()).unwrap().unwrap();
        assert_eq!(rec.outcome, Outcome::Attempted);
        assert_eq!(rec.undo.data.str_or("backup", ""), "/var/b/1");
        assert_eq!(fresh.unfinished().unwrap().len(), 1);
    }

    #[test]
    fn an_undo_is_handed_over_once() {
        let d = Dir::new("undoonce");
        let g = guard(&d);
        let r = g.rule(&agent(), &cap("fs.write:/home/robert/notes.md"));
        let seq = g
            .begin(&r, "save", undo())
            .unwrap()
            .finish(Outcome::Ok, "")
            .unwrap();

        let u = g.take_undo(seq).unwrap();
        assert_eq!(u.note, "restore notes.md");

        g.reverted(seq, 99).unwrap();
        assert!(g.take_undo(seq).is_err(), "must not undo twice");
        assert!(g.undoable().unwrap().is_none());
    }

    #[test]
    fn an_undo_is_only_marked_done_after_it_worked() {
        // take_undo() must not itself mark the record reverted. If it did, an
        // undo that then failed would leave the journal lying about the state
        // of the machine — and the journal is the only account there is.
        let d = Dir::new("undofail");
        let g = guard(&d);
        let r = g.rule(&agent(), &cap("fs.write:/home/robert/notes.md"));
        let seq = g
            .begin(&r, "save", undo())
            .unwrap()
            .finish(Outcome::Ok, "")
            .unwrap();

        let _ = g.take_undo(seq).unwrap(); // host tries, and suppose it fails
        assert!(
            g.journal().get(seq).unwrap().unwrap().is_revertible(),
            "a failed undo must leave the action still undoable"
        );
    }

    #[test]
    fn an_unknown_capability_is_not_handed_to_an_agent() {
        // Nothing grades `db.drop`, so it is Critical, so the agent ceiling
        // turns any allow into a confirmation. Two defaults, one outcome.
        let d = Dir::new("ungraded");
        let g = Guard::new(
            Policy::parse("allow agent:* db.*\n", "policy").unwrap(),
            Grades::parse(GRADES, "grades").unwrap(),
            Journal::open(&d.0).unwrap(),
            "/home/robert",
        );
        let r = g.rule(&agent(), &cap("db.drop:users"));
        assert!(r.needs_confirmation(), "{}", r.explain());
        assert_eq!(r.risk(), Risk::Critical);
    }

    #[test]
    fn a_ruling_can_explain_itself_to_a_person() {
        let d = Dir::new("prompt");
        let g = guard(&d);
        let r = g.rule(&agent(), &cap("fs.delete:/home/robert/old.txt"));
        let p = r.prompt();
        assert!(p.contains("agent:claude"), "{}", p);
        assert!(p.contains("fs.delete:/home/robert/old.txt"), "{}", p);
        assert!(p.contains("elevated"), "{}", p);
        assert!(p.contains("say it out loud"), "{}", p);
    }
}
