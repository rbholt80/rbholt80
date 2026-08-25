//! The journal: append-only audit trail and undo log.
//!
//! Every capability the broker adjudicates lands here, allowed or not. Two
//! properties matter and are worth the cost:
//!
//! 1. **Append-only.** Records are never rewritten in place, so an agent that
//!    misbehaves cannot erase the evidence through the same API it misused.
//! 2. **Reversible.** A mutating action records how to undo itself *before* it
//!    runs. An AI acting on your machine is only acceptable if you can put the
//!    machine back.

use crate::json::{json_obj, parse, Json};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Policy permitted it and it ran.
    Executed,
    /// Policy asked, the human said yes, it ran.
    Confirmed,
    /// Policy or the human refused.
    Refused,
    /// Permitted, but failed while running.
    Failed,
    /// Evaluated with side effects suppressed.
    DryRun,
}

impl Outcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Outcome::Executed => "executed",
            Outcome::Confirmed => "confirmed",
            Outcome::Refused => "refused",
            Outcome::Failed => "failed",
            Outcome::DryRun => "dry-run",
        }
    }

    pub fn parse(s: &str) -> Outcome {
        match s {
            "executed" => Outcome::Executed,
            "confirmed" => Outcome::Confirmed,
            "failed" => Outcome::Failed,
            "dry-run" => Outcome::DryRun,
            _ => Outcome::Refused,
        }
    }

    /// Did this actually change the machine?
    pub fn took_effect(&self) -> bool {
        matches!(self, Outcome::Executed | Outcome::Confirmed)
    }
}

/// How to reverse an action. Recorded before the action runs.
#[derive(Debug, Clone, PartialEq)]
pub enum Undo {
    /// Nothing to undo (a read, or an inherently reversible no-op).
    None,
    /// Restore `path` from the snapshot at `backup`. `existed: false` means the
    /// undo is a delete, because the file did not exist beforehand.
    RestoreFile { path: String, backup: Option<String>, existed: bool },
    /// Move `to` back to `from`.
    MovePath { from: String, to: String },
    /// Remove a directory the action created.
    RemoveDir { path: String },
    /// Put a service back the way it was.
    ServiceState { unit: String, was_active: bool },
    /// A human-readable instruction we cannot perform automatically.
    Manual { note: String },
}

impl Undo {
    pub fn is_none(&self) -> bool {
        matches!(self, Undo::None)
    }

    pub fn to_json(&self) -> Json {
        match self {
            Undo::None => Json::Null,
            Undo::RestoreFile { path, backup, existed } => json_obj([
                ("kind", "restore_file".into()),
                ("path", path.clone().into()),
                ("backup", backup.clone().map(Json::Str).unwrap_or(Json::Null)),
                ("existed", (*existed).into()),
            ]),
            Undo::MovePath { from, to } => json_obj([
                ("kind", "move_path".into()),
                ("from", from.clone().into()),
                ("to", to.clone().into()),
            ]),
            Undo::RemoveDir { path } => {
                json_obj([("kind", "remove_dir".into()), ("path", path.clone().into())])
            }
            Undo::ServiceState { unit, was_active } => json_obj([
                ("kind", "service_state".into()),
                ("unit", unit.clone().into()),
                ("was_active", (*was_active).into()),
            ]),
            Undo::Manual { note } => {
                json_obj([("kind", "manual".into()), ("note", note.clone().into())])
            }
        }
    }

    pub fn from_json(v: &Json) -> Undo {
        match v.str_or("kind", "") {
            "restore_file" => Undo::RestoreFile {
                path: v.str_or("path", "").to_string(),
                backup: v.get("backup").and_then(|b| b.as_str()).map(|s| s.to_string()),
                existed: v.bool_or("existed", false),
            },
            "move_path" => Undo::MovePath {
                from: v.str_or("from", "").to_string(),
                to: v.str_or("to", "").to_string(),
            },
            "remove_dir" => Undo::RemoveDir { path: v.str_or("path", "").to_string() },
            "service_state" => Undo::ServiceState {
                unit: v.str_or("unit", "").to_string(),
                was_active: v.bool_or("was_active", false),
            },
            "manual" => Undo::Manual { note: v.str_or("note", "").to_string() },
            _ => Undo::None,
        }
    }

    /// A one-line description of what undoing this would do.
    pub fn describe(&self) -> String {
        match self {
            Undo::None => "nothing to undo".to_string(),
            Undo::RestoreFile { path, existed: true, .. } => {
                format!("restore previous contents of {}", path)
            }
            Undo::RestoreFile { path, existed: false, .. } => format!("remove {}", path),
            Undo::MovePath { from, to } => format!("move {} back to {}", to, from),
            Undo::RemoveDir { path } => format!("remove directory {}", path),
            Undo::ServiceState { unit, was_active } => {
                format!("{} {}", if *was_active { "start" } else { "stop" }, unit)
            }
            Undo::Manual { note } => note.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Record {
    pub seq: u64,
    pub ts: u64,
    pub subject: String,
    pub capability: String,
    pub risk: String,
    pub decision: String,
    pub outcome: Outcome,
    pub intent: String,
    pub detail: String,
    pub undo: Undo,
    /// Set once this record has been reverted, naming the record that did it.
    pub undone_by: Option<u64>,
}

impl Record {
    pub fn to_json(&self) -> Json {
        json_obj([
            ("seq", self.seq.into()),
            ("ts", self.ts.into()),
            ("subject", self.subject.clone().into()),
            ("capability", self.capability.clone().into()),
            ("risk", self.risk.clone().into()),
            ("decision", self.decision.clone().into()),
            ("outcome", self.outcome.as_str().into()),
            ("intent", self.intent.clone().into()),
            ("detail", self.detail.clone().into()),
            ("undo", self.undo.to_json()),
            ("undone_by", self.undone_by.map(Json::from).unwrap_or(Json::Null)),
        ])
    }

    pub fn from_json(v: &Json) -> Record {
        Record {
            seq: v.get("seq").and_then(|x| x.as_u64()).unwrap_or(0),
            ts: v.get("ts").and_then(|x| x.as_u64()).unwrap_or(0),
            subject: v.str_or("subject", "?").to_string(),
            capability: v.str_or("capability", "?").to_string(),
            risk: v.str_or("risk", "?").to_string(),
            decision: v.str_or("decision", "?").to_string(),
            outcome: Outcome::parse(v.str_or("outcome", "refused")),
            intent: v.str_or("intent", "").to_string(),
            detail: v.str_or("detail", "").to_string(),
            undo: v.get("undo").map(Undo::from_json).unwrap_or(Undo::None),
            undone_by: v.get("undone_by").and_then(|x| x.as_u64()),
        }
    }

    /// Can this record still be reverted?
    pub fn is_revertible(&self) -> bool {
        self.outcome.took_effect() && !self.undo.is_none() && self.undone_by.is_none()
    }
}

pub struct Journal {
    path: PathBuf,
    /// Snapshots taken before mutations live here.
    backups: PathBuf,
    state: Mutex<JournalState>,
}

struct JournalState {
    next_seq: u64,
    /// seq -> the record that reverted it. Kept in memory and rebuilt on open,
    /// so the on-disk log stays strictly append-only.
    reverted: Vec<(u64, u64)>,
}

impl Journal {
    /// Open (or create) a journal rooted at `dir`.
    pub fn open(dir: &Path) -> Result<Journal, String> {
        let backups = dir.join("backups");
        fs::create_dir_all(&backups)
            .map_err(|e| format!("cannot create journal dir {}: {}", backups.display(), e))?;
        let path = dir.join("journal.jsonl");
        if !path.exists() {
            File::create(&path)
                .map_err(|e| format!("cannot create {}: {}", path.display(), e))?;
        }
        let j = Journal {
            path,
            backups,
            state: Mutex::new(JournalState { next_seq: 1, reverted: Vec::new() }),
        };
        let existing = j.read_all()?;
        let mut st = j.state.lock().unwrap();
        st.next_seq = existing.iter().map(|r| r.seq).max().unwrap_or(0) + 1;
        for r in &existing {
            if let Some(target) = revert_target(r) {
                st.reverted.push((target, r.seq));
            }
        }
        drop(st);
        Ok(j)
    }

    pub fn backup_dir(&self) -> &Path {
        &self.backups
    }

    /// Snapshot a file before it is modified, returning the backup path.
    /// Returns `Ok(None)` when the file does not exist yet.
    pub fn snapshot(&self, path: &Path) -> Result<Option<String>, String> {
        if !path.exists() {
            return Ok(None);
        }
        let seq = { self.state.lock().unwrap().next_seq };
        let stamp = path.file_name().and_then(|s| s.to_str()).unwrap_or("file");
        let dest = self.backups.join(format!("{:08}-{}", seq, stamp));
        fs::copy(path, &dest)
            .map_err(|e| format!("cannot snapshot {}: {}", path.display(), e))?;
        Ok(Some(dest.to_string_lossy().to_string()))
    }

    /// Append a record, assigning it a sequence number.
    pub fn append(&self, mut rec: Record) -> Result<u64, String> {
        let mut st = self.state.lock().unwrap();
        rec.seq = st.next_seq;
        st.next_seq += 1;
        if let Some(target) = revert_target(&rec) {
            st.reverted.push((target, rec.seq));
        }
        drop(st);

        let line = format!("{}\n", rec.to_json());
        let mut f = OpenOptions::new()
            .append(true)
            .open(&self.path)
            .map_err(|e| format!("cannot open journal: {}", e))?;
        f.write_all(line.as_bytes()).map_err(|e| format!("cannot write journal: {}", e))?;
        // Durability matters here: the undo record must survive the crash that
        // the action it describes might cause.
        f.sync_data().map_err(|e| format!("cannot sync journal: {}", e))?;
        Ok(rec.seq)
    }

    /// All records, oldest first, with `undone_by` resolved from the in-memory
    /// revert index.
    pub fn read_all(&self) -> Result<Vec<Record>, String> {
        let f = File::open(&self.path).map_err(|e| format!("cannot read journal: {}", e))?;
        let mut out = Vec::new();
        for line in BufReader::new(f).lines() {
            let line = line.map_err(|e| format!("cannot read journal: {}", e))?;
            if line.trim().is_empty() {
                continue;
            }
            // A corrupt line is skipped rather than fatal: a truncated final
            // write must not make the whole history unreadable.
            if let Ok(v) = parse(&line) {
                out.push(Record::from_json(&v));
            }
        }
        if let Ok(st) = self.state.lock() {
            for rec in out.iter_mut() {
                if let Some((_, by)) = st.reverted.iter().find(|(t, _)| *t == rec.seq) {
                    rec.undone_by = Some(*by);
                }
            }
        }
        Ok(out)
    }

    pub fn tail(&self, n: usize) -> Result<Vec<Record>, String> {
        let all = self.read_all()?;
        let start = all.len().saturating_sub(n);
        Ok(all[start..].to_vec())
    }

    pub fn get(&self, seq: u64) -> Result<Option<Record>, String> {
        Ok(self.read_all()?.into_iter().find(|r| r.seq == seq))
    }

    /// The most recent record that can still be reverted.
    pub fn last_revertible(&self) -> Result<Option<Record>, String> {
        Ok(self.read_all()?.into_iter().rev().find(|r| r.is_revertible()))
    }
}

/// If this record is itself a revert, which sequence did it revert?
fn revert_target(rec: &Record) -> Option<u64> {
    if !rec.capability.starts_with("journal.revert") {
        return None;
    }
    rec.capability.split(':').nth(1)?.parse().ok()
}

pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Format a unix timestamp as `YYYY-MM-DD HH:MM:SS` in UTC.
///
/// Hand-rolled because the core carries no dependencies, and because the only
/// alternative — shelling out to `date` for every journal line — is absurd.
pub fn format_ts(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let tod = secs % 86_400;
    let (h, m, s) = (tod / 3600, (tod % 3600) / 60, tod % 60);

    // Civil-from-days, Howard Hinnant's algorithm, shifted to a March-based year.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mth = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if mth <= 2 { y + 1 } else { y };

    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", year, mth, d, h, m, s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("nous-journal-test-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn rec(cap: &str, outcome: Outcome, undo: Undo) -> Record {
        Record {
            seq: 0,
            ts: now_secs(),
            subject: "user".into(),
            capability: cap.into(),
            risk: "write".into(),
            decision: "allow".into(),
            outcome,
            intent: "test".into(),
            detail: String::new(),
            undo,
            undone_by: None,
        }
    }

    #[test]
    fn assigns_monotonic_sequences_and_survives_reopen() {
        let dir = tmpdir("seq");
        {
            let j = Journal::open(&dir).unwrap();
            assert_eq!(j.append(rec("fs.read:/a", Outcome::Executed, Undo::None)).unwrap(), 1);
            assert_eq!(j.append(rec("fs.read:/b", Outcome::Executed, Undo::None)).unwrap(), 2);
        }
        let j2 = Journal::open(&dir).unwrap();
        assert_eq!(j2.append(rec("fs.read:/c", Outcome::Executed, Undo::None)).unwrap(), 3);
        assert_eq!(j2.read_all().unwrap().len(), 3);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn records_round_trip_through_json() {
        let dir = tmpdir("roundtrip");
        let j = Journal::open(&dir).unwrap();
        let undo = Undo::RestoreFile {
            path: "/home/joey/a.txt".into(),
            backup: Some("/var/lib/nous/backups/1-a.txt".into()),
            existed: true,
        };
        j.append(rec("fs.write:/home/joey/a.txt", Outcome::Confirmed, undo.clone())).unwrap();
        let back = &j.read_all().unwrap()[0];
        assert_eq!(back.undo, undo);
        assert_eq!(back.outcome, Outcome::Confirmed);
        assert!(back.is_revertible());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn refused_actions_are_not_revertible() {
        let dir = tmpdir("refused");
        let j = Journal::open(&dir).unwrap();
        let u = Undo::RemoveDir { path: "/tmp/x".into() };
        j.append(rec("fs.mkdir:/tmp/x", Outcome::Refused, u)).unwrap();
        assert!(!j.read_all().unwrap()[0].is_revertible());
        assert!(j.last_revertible().unwrap().is_none());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn reverting_marks_the_original_and_stops_double_undo() {
        let dir = tmpdir("revert");
        let j = Journal::open(&dir).unwrap();
        let target = j
            .append(rec(
                "fs.write:/tmp/a",
                Outcome::Executed,
                Undo::RestoreFile { path: "/tmp/a".into(), backup: None, existed: false },
            ))
            .unwrap();
        assert!(j.last_revertible().unwrap().is_some());

        j.append(rec(&format!("journal.revert:{}", target), Outcome::Executed, Undo::None)).unwrap();

        let original = j.get(target).unwrap().unwrap();
        assert_eq!(original.undone_by, Some(2));
        assert!(!original.is_revertible(), "an undone action must not be undoable twice");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn snapshot_copies_only_existing_files() {
        let dir = tmpdir("snap");
        let j = Journal::open(&dir).unwrap();
        let src = dir.join("subject.txt");
        assert_eq!(j.snapshot(&src).unwrap(), None);
        fs::write(&src, b"before").unwrap();
        let backup = j.snapshot(&src).unwrap().expect("existing file yields a backup");
        assert_eq!(fs::read_to_string(&backup).unwrap(), "before");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn corrupt_lines_do_not_poison_the_log() {
        let dir = tmpdir("corrupt");
        let j = Journal::open(&dir).unwrap();
        j.append(rec("fs.read:/a", Outcome::Executed, Undo::None)).unwrap();
        let mut f = OpenOptions::new().append(true).open(dir.join("journal.jsonl")).unwrap();
        f.write_all(b"{\"seq\": trunca\n").unwrap();
        drop(f);
        j.append(rec("fs.read:/b", Outcome::Executed, Undo::None)).unwrap();
        assert_eq!(j.read_all().unwrap().len(), 2);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn formats_timestamps_in_utc() {
        assert_eq!(format_ts(0), "1970-01-01 00:00:00");
        assert_eq!(format_ts(1_700_000_000), "2023-11-14 22:13:20");
        // A leap day, because the calendar arithmetic is the easy thing to get wrong.
        assert_eq!(format_ts(1_709_164_800), "2024-02-29 00:00:00");
    }
}
