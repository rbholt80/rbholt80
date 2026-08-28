//! # warrant
//!
//! **A model may propose. Only fixed, auditable code may decide.**
//!
//! This crate is the second half of that sentence. It sits between something
//! that proposes actions — a language model, an agent, a script, a rules engine
//! — and the code that carries them out, and it does four things:
//!
//! 1. Makes the request say what it is, as a [`Capability`]: `fs.delete:/x`.
//! 2. Grades it against a table the *host* wrote, not one it inferred
//!    ([`Grades`]).
//! 3. Answers allow / confirm / deny / never from ordered rules, citing the
//!    line that decided ([`Policy`], [`Verdict`]).
//! 4. Writes down how to reverse the action **before** letting it happen, in an
//!    append-only log a person can read with `jq` ([`Journal`]).
//!
//! It performs no actions and reverses none. It has no dependencies. It is
//! about two thousand lines, and the point is that you can read all of them.
//!
//! ## Why not just ask the model to be careful
//!
//! Because the request and the judgment would then come from the same place. A
//! model that has been talked into deleting your home directory has also been
//! talked into believing that deleting it is fine, and it will explain why
//! fluently. Every safety property worth having has to hold *even when the
//! thing asking is confidently wrong*, which means it cannot be the thing
//! asking that checks.
//!
//! So the model gets to say what it wants, in a form a rule can be applied to,
//! and something with no opinions applies the rule.
//!
//! ## The defaults, and why they are the way they are
//!
//! - **An ungraded capability is [`Risk::Critical`].** A capability nobody
//!   thought about is exactly the one to be asked about. The opposite default
//!   is wrong precisely once.
//! - **An unmatched request is denied.** No rule permitting it means nobody
//!   considered it.
//! - **An `allow` cannot lift an agent above [`Policy::agent_ceiling`].** Policy
//!   files are written once; capabilities are added forever.
//! - **A `never` line cannot be confirmed past.** Not by the policy's own
//!   ordering, not by an agent, not by a human clicking yes. It is for the
//!   things that stay true however good the argument for the exception sounds.
//!
//! ## Getting started
//!
//! ```no_run
//! use warrant::{Guard, Grades, Policy, Subject, Capability, Undo, Outcome, json_obj};
//! use std::path::Path;
//!
//! # fn main() -> Result<(), String> {
//! let guard = Guard::open(
//!     Path::new("/var/lib/myapp"),
//!     Policy::parse(&std::fs::read_to_string("policy").unwrap(), "policy")?,
//!     Grades::parse(&std::fs::read_to_string("grades").unwrap(), "grades")?,
//! )?;
//!
//! let cap = Capability::parse("fs.write:/home/robert/notes.md")?;
//! let ruling = guard.rule(&Subject::Agent("claude".into()), &cap);
//!
//! if ruling.allowed() {
//!     let pending = guard.begin(&ruling, "save the draft",
//!         Undo::new("restore notes.md", json_obj([("backup", "/var/b/7".into())])))?;
//!     // ... do the work ...
//!     pending.finish(Outcome::Ok, "wrote 412 bytes")?;
//! } else {
//!     println!("{}", ruling.explain());
//!     guard.refuse(&ruling, "asked to overwrite notes")?;
//! }
//! # Ok(())
//! # }
//! ```
//!
//! ## From other languages
//!
//! The `warrant` binary speaks line-delimited JSON on stdin and stdout, so a
//! Python or Kotlin host drives it over a pipe without an FFI boundary. See
//! `bindings/warrant.py` and the README.

pub mod cap;
pub mod grade;
pub mod guard;
pub mod journal;
pub mod json;
pub mod policy;

pub use cap::{glob_match, Capability};
pub use grade::{Grades, Risk};
pub use guard::{Guard, Pending, Ruling};
pub use journal::{format_ts, now_secs, Act, Journal, Outcome, Record, Undo};
pub use json::{json_obj, Json};
pub use policy::{Decision, Policy, Rule, Subject, Verdict};

/// The version of this crate.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// A starting policy for a host that has not written one yet.
///
/// It is deliberately not empty and deliberately not permissive: it denies
/// everything by omission, and demonstrates the four decision words so that the
/// first thing an operator does is edit a file that already reads correctly.
pub const EXAMPLE_POLICY: &str = "\
# decision  subject   capability              # reason
#
# Rules are ordered and the first match wins, so narrow exceptions go above
# broad defaults. `never` is different: it is a floor, checked before any of
# this, that nothing below and no human clicking yes can lift.

never       *         fs.read:/**/.ssh/**     # private keys stay on the disk
never       *         fs.read:/**/.gnupg/**
never       *         fs.write:/boot/**       # do not touch the boot path

deny        agent:*   pkg.install             # a person installs software
confirm     agent:*   fs.delete:~/**          # say it out loud first
allow       agent:*   fs.read:~/**
allow       agent:*   fs.write:~/**
allow       user      fs.*:~/**
";

/// A starting grades table, matching [`EXAMPLE_POLICY`].
pub const EXAMPLE_GRADES: &str = "\
# What each capability costs if the request was wrong. Anything not named here
# is treated as critical — which is why adding a capability and forgetting to
# grade it makes the system cautious rather than careless.

read      fs.read fs.list fs.stat
write     fs.write fs.move fs.mkdir
elevated  fs.delete fs.chmod pkg.install net.connect
critical  sys.firmware sys.mount
";
