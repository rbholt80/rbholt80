//! What happened, and how to take it back.
//!
//! The journal is append-only and one JSON object per line, so it can be read
//! with `tail`, `grep` and `jq` by somebody who has never heard of this crate.
//! An audit log that needs its own tooling does not get audited.
//!
//! # Why an action is written down twice
//!
//! Each action produces an `act` line before it runs and an `end` line after.
//! That looks like overhead until you consider the case the whole crate exists
//! for: the process dies *during* the action. Journalling once, afterwards,
//! means the crash that most needs an undo record is the one that has none. So
//! the undo is written, and flushed, before the caller is allowed to act — and
//! an `act` with no `end` is a durable record of an action that may be half
//! done, which is exactly the thing a person needs to be told about.
//!
//! Reverting appends too; nothing is ever rewritten in place. Reading folds the
//! lines back into whole records.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::json::{json_obj, parse, Json};

/// How an action ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Begun, and never reported finished. The process died, or is still going.
    Attempted,
    /// Done.
    Ok,
    /// Tried and failed. May have partially taken effect — that is why it is
    /// distinct from `Refused`.
    Failed,
    /// Never started: policy said no.
    Refused,
    /// Done, and since taken back.
    Reverted,
}

impl Outcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Outcome::Attempted => "attempted",
            Outcome::Ok => "ok",
            Outcome::Failed => "failed",
            Outcome::Refused => "refused",
            Outcome::Reverted => "reverted",
        }
    }

    pub fn parse(s: &str) -> Outcome {
        match s {
            "ok" => Outcome::Ok,
            "failed" => Outcome::Failed,
            "refused" => Outcome::Refused,
            "reverted" => Outcome::Reverted,
            _ => Outcome::Attempted,
        }
    }

    /// True if this may have changed something. `Attempted` counts: an action
    /// that never reported back is exactly the one you cannot assume was a
    /// no-op.
    pub fn took_effect(&self) -> bool {
        matches!(self, Outcome::Ok | Outcome::Failed | Outcome::Attempted)
    }
}

/// How to reverse an action.
///
/// Warrant does not perform undos — it never touches anything but its own
/// journal, which is what lets it be trusted to adjudicate. It stores what the
/// host said would reverse the action and hands it back on request.
///
/// `note` is for a person: *"restore the previous contents of notes.md"*.
/// `data` is for the host: whatever it needs to actually do that.
#[derive(Debug, Clone, PartialEq)]
pub struct Undo {
    pub note: String,
    pub data: Json,
}

impl Undo {
    /// No undo, because there is nothing to undo — a read, or a genuine no-op.
    ///
    /// Note that this is *not* the same as "we don't know how": say that with
    /// [`Undo::manual`], so a person reading the journal can see the difference
    /// between nothing happened and nobody wrote the reversal.
    pub fn none() -> Undo {
        Undo {
            note: String::new(),
            data: Json::Null,
        }
    }

    pub fn new(note: &str, data: Json) -> Undo {
        Undo {
            note: note.to_string(),
            data,
        }
    }

    /// Something a person will have to reverse by hand.
    pub fn manual(note: &str) -> Undo {
        Undo {
            note: note.to_string(),
            data: json_obj([("manual", true.into())]),
        }
    }

    pub fn is_none(&self) -> bool {
        self.note.is_empty() && self.data.is_null()
    }

    /// True if the host gave machine-readable instructions, not just prose.
    pub fn is_automatic(&self) -> bool {
        !self.data.is_null() && !self.data.bool_or("manual", false)
    }

    pub fn describe(&self) -> String {
        if self.is_none() {
            "nothing to undo".to_string()
        } else {
            self.note.clone()
        }
    }

    fn to_json(&self) -> Json {
        if self.is_none() {
            return Json::Null;
        }
        json_obj([
            ("note", self.note.clone().into()),
            ("data", self.data.clone()),
        ])
    }

    fn from_json(v: &Json) -> Undo {
        if v.is_null() {
            return Undo::none();
        }
        Undo {
            note: v.str_or("note", "").to_string(),
            data: v.get("data").cloned().unwrap_or(Json::Null),
        }
    }
}

/// One adjudicated action, folded back together from its lines.
#[derive(Debug, Clone)]
pub struct Record {
    pub seq: u64,
    pub ts: u64,
    pub subject: String,
    pub capability: String,
    pub risk: String,
    pub decision: String,
    /// The policy line that decided, as `source:line`.
    pub matched: String,
    /// What the caller said it was for, in a person's words.
    pub intent: String,
    pub outcome: Outcome,
    pub detail: String,
    pub undo: Undo,
    /// Set once reverted, naming the record that did it.
    pub undone_by: Option<u64>,
}

impl Record {
    /// Can this be taken back right now?
    pub fn is_revertible(&self) -> bool {
        self.undone_by.is_none() && self.outcome.took_effect() && self.undo.is_automatic()
    }

    pub fn to_json(&self) -> Json {
        json_obj([
            ("seq", self.seq.into()),
            ("ts", self.ts.into()),
            ("subject", self.subject.clone().into()),
            ("capability", self.capability.clone().into()),
            ("risk", self.risk.clone().into()),
            ("decision", self.decision.clone().into()),
            ("matched", self.matched.clone().into()),
            ("intent", self.intent.clone().into()),
            ("outcome", self.outcome.as_str().into()),
            ("detail", self.detail.clone().into()),
            ("undo", self.undo.to_json()),
            (
                "undone_by",
                self.undone_by.map(Json::from).unwrap_or(Json::Null),
            ),
        ])
    }

    /// One line, for a person reading history.
    pub fn summary(&self) -> String {
        let when = format_ts(self.ts);
        let what = if self.intent.is_empty() {
            self.capability.clone()
        } else {
            self.intent.clone()
        };
        let tail = match self.undone_by {
            Some(by) => format!(" (undone by #{})", by),
            None => String::new(),
        };
        format!(
            "#{} {} {} — {} [{}]{}",
            self.seq,
            when,
            self.subject,
            what,
            self.outcome.as_str(),
            tail
        )
    }
}

/// What is about to happen, on its way to the journal.
///
/// Named fields rather than a row of positional strings: `subject`,
/// `capability`, `risk`, `decision` and `matched` are all `&str`, and two of
/// them transposed at a call site would compile, run, and quietly write a
/// wrong audit record.
#[derive(Debug)]
pub struct Act<'a> {
    pub subject: &'a str,
    pub capability: &'a str,
    pub risk: &'a str,
    pub decision: &'a str,
    /// The policy line that decided, as `source:line`.
    pub matched: &'a str,
    /// What the caller said it was for, in a person's words.
    pub intent: &'a str,
    pub undo: &'a Undo,
}

/// An append-only record of every adjudicated action.
pub struct Journal {
    path: PathBuf,
}

impl Journal {
    /// Open (creating if needed) the journal in `dir`.
    pub fn open(dir: &Path) -> Result<Journal, String> {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
        let path = dir.join("journal.ndjson");
        if !path.exists() {
            File::create(&path).map_err(|e| format!("{}: {}", path.display(), e))?;
        }
        Ok(Journal { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn append_line(&self, v: &Json) -> Result<(), String> {
        let mut f = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&self.path)
            .map_err(|e| format!("{}: {}", self.path.display(), e))?;
        // One write of one line: O_APPEND makes it atomic against other writers,
        // so two processes journalling at once cannot interleave halves.
        let line = format!("{}\n", v);
        f.write_all(line.as_bytes())
            .map_err(|e| format!("{}: {}", self.path.display(), e))?;
        // Flushed before we return, because the caller is about to act on the
        // strength of this being on disk.
        f.sync_data()
            .map_err(|e| format!("{}: {}", self.path.display(), e))?;
        Ok(())
    }

    /// Record an action *about* to happen, with the undo that would reverse it.
    /// Returns its sequence number.
    pub fn begin(&self, act: &Act<'_>) -> Result<u64, String> {
        let seq = self.next_seq()?;
        self.append_line(&json_obj([
            ("kind", "act".into()),
            ("seq", seq.into()),
            ("ts", now_secs().into()),
            ("subject", act.subject.into()),
            ("capability", act.capability.into()),
            ("risk", act.risk.into()),
            ("decision", act.decision.into()),
            ("matched", act.matched.into()),
            ("intent", act.intent.into()),
            ("undo", act.undo.to_json()),
        ]))?;
        Ok(seq)
    }

    /// Record how the action at `seq` ended.
    pub fn end(&self, seq: u64, outcome: Outcome, detail: &str) -> Result<(), String> {
        self.append_line(&json_obj([
            ("kind", "end".into()),
            ("seq", seq.into()),
            ("ts", now_secs().into()),
            ("outcome", outcome.as_str().into()),
            ("detail", detail.into()),
        ]))
    }

    /// Mark `seq` as reverted by `by`.
    pub fn mark_reverted(&self, seq: u64, by: u64) -> Result<(), String> {
        self.append_line(&json_obj([
            ("kind", "undone".into()),
            ("seq", seq.into()),
            ("ts", now_secs().into()),
            ("by", by.into()),
        ]))
    }

    /// Every record, oldest first, with its lines folded back together.
    ///
    /// A malformed line is skipped rather than fatal: a truncated last line
    /// after a power cut must not make the whole history unreadable.
    pub fn read_all(&self) -> Result<Vec<Record>, String> {
        let f = match File::open(&self.path) {
            Ok(f) => f,
            Err(_) => return Ok(Vec::new()),
        };
        let mut order: Vec<u64> = Vec::new();
        let mut by_seq: BTreeMap<u64, Record> = BTreeMap::new();

        for line in BufReader::new(f).lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => continue,
            };
            if line.trim().is_empty() {
                continue;
            }
            let v = match parse(&line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let seq = match v.get("seq").and_then(|s| s.as_u64()) {
                Some(s) => s,
                None => continue,
            };
            match v.str_or("kind", "") {
                "act" => {
                    if by_seq.contains_key(&seq) {
                        continue;
                    }
                    order.push(seq);
                    by_seq.insert(
                        seq,
                        Record {
                            seq,
                            ts: v.get("ts").and_then(|t| t.as_u64()).unwrap_or(0),
                            subject: v.str_or("subject", "").to_string(),
                            capability: v.str_or("capability", "").to_string(),
                            risk: v.str_or("risk", "").to_string(),
                            decision: v.str_or("decision", "").to_string(),
                            matched: v.str_or("matched", "").to_string(),
                            intent: v.str_or("intent", "").to_string(),
                            outcome: Outcome::Attempted,
                            detail: String::new(),
                            undo: Undo::from_json(v.get("undo").unwrap_or(&Json::Null)),
                            undone_by: None,
                        },
                    );
                }
                "end" => {
                    if let Some(rec) = by_seq.get_mut(&seq) {
                        rec.outcome = Outcome::parse(v.str_or("outcome", ""));
                        rec.detail = v.str_or("detail", "").to_string();
                    }
                }
                "undone" => {
                    if let Some(rec) = by_seq.get_mut(&seq) {
                        rec.undone_by = v.get("by").and_then(|b| b.as_u64());
                        rec.outcome = Outcome::Reverted;
                    }
                }
                _ => {}
            }
        }

        Ok(order
            .into_iter()
            .filter_map(|s| by_seq.remove(&s))
            .collect())
    }

    /// The most recent `n` records, oldest first.
    pub fn tail(&self, n: usize) -> Result<Vec<Record>, String> {
        let all = self.read_all()?;
        let start = all.len().saturating_sub(n);
        Ok(all[start..].to_vec())
    }

    pub fn get(&self, seq: u64) -> Result<Option<Record>, String> {
        Ok(self.read_all()?.into_iter().find(|r| r.seq == seq))
    }

    /// The newest record that can still be taken back.
    pub fn last_revertible(&self) -> Result<Option<Record>, String> {
        Ok(self
            .read_all()?
            .into_iter()
            .rev()
            .find(|r| r.is_revertible()))
    }

    /// Actions begun and never reported finished — the ones a crash left in
    /// the air. A host should show these on startup.
    pub fn unfinished(&self) -> Result<Vec<Record>, String> {
        Ok(self
            .read_all()?
            .into_iter()
            .filter(|r| r.outcome == Outcome::Attempted)
            .collect())
    }

    fn next_seq(&self) -> Result<u64, String> {
        Ok(self.read_all()?.iter().map(|r| r.seq).max().unwrap_or(0) + 1)
    }
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `YYYY-MM-DD HH:MM:SS` in UTC, without pulling in a date library.
pub fn format_ts(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let tod = secs % 86_400;
    let (h, m, s) = (tod / 3600, (tod % 3600) / 60, tod % 60);

    // Civil-from-days (Howard Hinnant's algorithm), shifted to a March-based year.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };

    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", y, mo, d, h, m, s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::json_obj;

    struct Dir(PathBuf);
    impl Dir {
        fn new(tag: &str) -> Dir {
            let p = std::env::temp_dir().join(format!(
                "warrant-test-{}-{}-{:?}",
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

    fn undo() -> Undo {
        Undo::new(
            "restore the previous contents of a.txt",
            json_obj([("backup", "/var/b/1".into())]),
        )
    }

    fn act<'a>(cap: &'a str, risk: &'a str, decision: &'a str, u: &'a Undo) -> Act<'a> {
        Act {
            subject: "agent:a",
            capability: cap,
            risk,
            decision,
            matched: "policy:4",
            intent: "save the draft",
            undo: u,
        }
    }

    fn write_one(j: &Journal) -> u64 {
        let u = undo();
        let seq = j
            .begin(&act("fs.write:/home/r/a.txt", "write", "allow", &u))
            .unwrap();
        j.end(seq, Outcome::Ok, "wrote 412 bytes").unwrap();
        seq
    }

    #[test]
    fn an_action_reads_back_whole() {
        let d = Dir::new("whole");
        let j = Journal::open(&d.0).unwrap();
        let seq = write_one(&j);

        let r = j.get(seq).unwrap().unwrap();
        assert_eq!(r.subject, "agent:a");
        assert_eq!(r.capability, "fs.write:/home/r/a.txt");
        assert_eq!(r.matched, "policy:4");
        assert_eq!(r.intent, "save the draft");
        assert_eq!(r.outcome, Outcome::Ok);
        assert_eq!(r.detail, "wrote 412 bytes");
        assert_eq!(r.undo.note, "restore the previous contents of a.txt");
    }

    #[test]
    fn the_undo_is_on_disk_before_the_action_is_allowed_to_run() {
        // The guarantee the crate exists for. After begin() and before end(),
        // the reversal must already be durable — because the crash we care
        // about happens exactly there.
        let d = Dir::new("durable");
        let j = Journal::open(&d.0).unwrap();
        let u = undo();
        let seq = j
            .begin(&Act {
                intent: "",
                matched: "p:1",
                ..act("fs.write:/x", "write", "allow", &u)
            })
            .unwrap();

        // Nothing has called end(). Read the file with a fresh handle, as a
        // separate process recovering from a crash would.
        let recovered = Journal::open(&d.0).unwrap().get(seq).unwrap().unwrap();
        assert_eq!(recovered.outcome, Outcome::Attempted);
        assert!(
            recovered.undo.is_automatic(),
            "an interrupted action must still be revertible"
        );
        assert_eq!(recovered.undo.data.str_or("backup", ""), "/var/b/1");
    }

    #[test]
    fn an_interrupted_action_is_reported_as_unfinished() {
        let d = Dir::new("unfinished");
        let j = Journal::open(&d.0).unwrap();
        write_one(&j); // completed
        let u = undo();
        let hung = j
            .begin(&Act {
                intent: "",
                matched: "p:1",
                ..act("fs.write:/y", "write", "allow", &u)
            })
            .unwrap();

        let open: Vec<u64> = j.unfinished().unwrap().iter().map(|r| r.seq).collect();
        assert_eq!(open, vec![hung]);
    }

    #[test]
    fn reverting_appends_and_never_rewrites() {
        let d = Dir::new("revert");
        let j = Journal::open(&d.0).unwrap();
        let seq = write_one(&j);
        let before = fs::read_to_string(j.path()).unwrap();

        j.mark_reverted(seq, 99).unwrap();
        let after = fs::read_to_string(j.path()).unwrap();

        assert!(after.starts_with(&before), "history was rewritten");
        let r = j.get(seq).unwrap().unwrap();
        assert_eq!(r.undone_by, Some(99));
        assert_eq!(r.outcome, Outcome::Reverted);
        assert!(!r.is_revertible(), "must not be undoable twice");
    }

    #[test]
    fn last_revertible_skips_reads_and_refusals() {
        let d = Dir::new("lastrev");
        let j = Journal::open(&d.0).unwrap();
        let good = write_one(&j);

        // A read: nothing to take back.
        let none = Undo::none();
        let s = j
            .begin(&Act {
                subject: "user",
                intent: "",
                matched: "p:1",
                ..act("fs.read:/x", "read", "allow", &none)
            })
            .unwrap();
        j.end(s, Outcome::Ok, "").unwrap();

        // A refusal: never happened.
        let u = undo();
        let s = j
            .begin(&Act {
                intent: "",
                matched: "p:2",
                ..act("fs.delete:/x", "elevated", "deny", &u)
            })
            .unwrap();
        j.end(s, Outcome::Refused, "policy").unwrap();

        assert_eq!(j.last_revertible().unwrap().unwrap().seq, good);
    }

    #[test]
    fn a_manual_undo_is_not_offered_as_automatic() {
        // "We don't know how to reverse this" must not read the same as
        // "there is nothing to reverse", and must not be offered as a button.
        let d = Dir::new("manual");
        let j = Journal::open(&d.0).unwrap();
        let manual = Undo::manual("write to Bob and apologise");
        let seq = j
            .begin(&Act {
                subject: "user",
                intent: "send the invoice",
                matched: "p:1",
                ..act("mail.send:bob@example.com", "elevated", "allow", &manual)
            })
            .unwrap();
        j.end(seq, Outcome::Ok, "").unwrap();

        let r = j.get(seq).unwrap().unwrap();
        assert!(!r.undo.is_none());
        assert!(!r.undo.is_automatic());
        assert!(!r.is_revertible());
        assert_eq!(r.undo.describe(), "write to Bob and apologise");
        assert!(j.last_revertible().unwrap().is_none());
    }

    #[test]
    fn a_truncated_last_line_does_not_lose_the_history() {
        // A power cut mid-write. The rest of the log must still read.
        let d = Dir::new("torn");
        let j = Journal::open(&d.0).unwrap();
        let seq = write_one(&j);
        {
            let mut f = OpenOptions::new().append(true).open(j.path()).unwrap();
            f.write_all(b"{\"kind\":\"act\",\"seq\":2,\"subj").unwrap();
        }
        let all = j.read_all().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].seq, seq);
    }

    #[test]
    fn sequence_numbers_survive_reopening() {
        let d = Dir::new("seq");
        let first = { write_one(&Journal::open(&d.0).unwrap()) };
        let second = { write_one(&Journal::open(&d.0).unwrap()) };
        assert_eq!((first, second), (1, 2));
    }

    #[test]
    fn tail_returns_the_newest_oldest_first() {
        let d = Dir::new("tail");
        let j = Journal::open(&d.0).unwrap();
        for _ in 0..5 {
            write_one(&j);
        }
        let got: Vec<u64> = j.tail(2).unwrap().iter().map(|r| r.seq).collect();
        assert_eq!(got, vec![4, 5]);
    }

    #[test]
    fn the_file_is_one_json_object_per_line() {
        // The promise that `jq` works on it. Worth a test, because a pretty
        // printer sneaking in would break every downstream reader silently.
        let d = Dir::new("ndjson");
        let j = Journal::open(&d.0).unwrap();
        write_one(&j);
        let text = fs::read_to_string(j.path()).unwrap();
        assert_eq!(text.lines().count(), 2, "one act line, one end line");
        for line in text.lines() {
            assert!(parse(line).is_ok(), "not valid JSON: {}", line);
        }
    }

    #[test]
    fn timestamps_format_without_a_date_crate() {
        assert_eq!(format_ts(0), "1970-01-01 00:00:00");
        assert_eq!(format_ts(1_000_000_000), "2001-09-09 01:46:40");
        // A leap day, which is where hand-rolled date maths goes wrong.
        assert_eq!(format_ts(1_709_164_800), "2024-02-29 00:00:00");
    }
}
