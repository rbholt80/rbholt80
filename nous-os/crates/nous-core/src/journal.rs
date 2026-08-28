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
    RestoreFile {
        path: String,
        backup: Option<String>,
        existed: bool,
    },
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
            Undo::RestoreFile {
                path,
                backup,
                existed,
            } => json_obj([
                ("kind", "restore_file".into()),
                ("path", path.clone().into()),
                (
                    "backup",
                    backup.clone().map(Json::Str).unwrap_or(Json::Null),
                ),
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
                backup: v
                    .get("backup")
                    .and_then(|b| b.as_str())
                    .map(|s| s.to_string()),
                existed: v.bool_or("existed", false),
            },
            "move_path" => Undo::MovePath {
                from: v.str_or("from", "").to_string(),
                to: v.str_or("to", "").to_string(),
            },
            "remove_dir" => Undo::RemoveDir {
                path: v.str_or("path", "").to_string(),
            },
            "service_state" => Undo::ServiceState {
                unit: v.str_or("unit", "").to_string(),
                was_active: v.bool_or("was_active", false),
            },
            "manual" => Undo::Manual {
                note: v.str_or("note", "").to_string(),
            },
            _ => Undo::None,
        }
    }

    /// A one-line description of what undoing this would do.
    pub fn describe(&self) -> String {
        match self {
            Undo::None => "nothing to undo".to_string(),
            Undo::RestoreFile {
                path,
                existed: true,
                ..
            } => {
                format!("restore previous contents of {}", path)
            }
            Undo::RestoreFile {
                path,
                existed: false,
                ..
            } => format!("remove {}", path),
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
            (
                "undone_by",
                self.undone_by.map(Json::from).unwrap_or(Json::Null),
            ),
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

/// How much history to keep.
///
/// A system that warns you your disk is filling up must not be the thing
/// filling it. Every mutation snapshots the file it is about to change, so an
/// unbounded journal is an unbounded copy of everything you have ever edited.
#[derive(Debug, Clone, Copy)]
pub struct Retention {
    /// Entries in the live journal before it is rotated.
    pub max_records: usize,
    /// How many rotated journals to keep. Older ones are deleted.
    pub max_archives: usize,
    /// Ceiling on the snapshot store. The oldest unreferenced snapshots go
    /// first.
    pub max_backup_bytes: u64,
}

impl Default for Retention {
    fn default() -> Self {
        Retention {
            max_records: 20_000,
            max_archives: 4,
            max_backup_bytes: 2 * 1024 * 1024 * 1024,
        }
    }
}

/// What a prune actually did. Reported, never silent: deleting history the user
/// did not ask to lose should be visible.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PruneReport {
    pub rotated: bool,
    pub archives_removed: usize,
    pub records_dropped: usize,
    pub backups_removed: usize,
    pub bytes_reclaimed: u64,
    /// Snapshots kept because an action that can still be undone needs them.
    pub kept_for_undo: usize,
}

impl PruneReport {
    pub fn is_empty(&self) -> bool {
        *self == PruneReport::default()
    }

    pub fn describe(&self) -> String {
        if self.is_empty() {
            return "nothing to prune".to_string();
        }
        let mut parts = Vec::new();
        if self.rotated {
            parts.push("rotated the journal".to_string());
        }
        if self.archives_removed > 0 {
            parts.push(format!("dropped {} old journal(s)", self.archives_removed));
        }
        if self.backups_removed > 0 {
            parts.push(format!("removed {} snapshot(s)", self.backups_removed));
        }
        if self.bytes_reclaimed > 0 {
            parts.push(format!("reclaimed {}", human_bytes(self.bytes_reclaimed)));
        }
        if self.kept_for_undo > 0 {
            parts.push(format!("kept {} still undoable", self.kept_for_undo));
        }
        parts.join(", ")
    }
}

/// A byte count in the units a person reads.
///
/// The table runs to exabytes so that a large number never comes out as
/// "16777216.0 TB", which is the shape a too-short table produces: correct,
/// and unreadable.
pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 7] = ["B", "KB", "MB", "GB", "TB", "PB", "EB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{} {}", n, UNITS[0])
    } else {
        format!("{:.1} {}", v, UNITS[i])
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
            File::create(&path).map_err(|e| format!("cannot create {}: {}", path.display(), e))?;
        }
        let j = Journal {
            path,
            backups,
            state: Mutex::new(JournalState {
                next_seq: 1,
                reverted: Vec::new(),
            }),
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

    fn dir(&self) -> PathBuf {
        self.path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
    }

    /// Rotated journals, oldest first.
    ///
    /// Rotation preserves the append-only property that makes the journal worth
    /// having: a rotated file is moved whole and never rewritten. History ages
    /// out by whole files, and everything still on disk is still readable and
    /// still undoable.
    fn archives(&self) -> Vec<PathBuf> {
        let mut found: Vec<(u64, PathBuf)> = Vec::new();
        if let Ok(entries) = fs::read_dir(self.dir()) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if let Some(n) = name
                    .strip_prefix("journal.")
                    .and_then(|r| r.strip_suffix(".jsonl"))
                    .and_then(|n| n.parse::<u64>().ok())
                {
                    found.push((n, e.path()));
                }
            }
        }
        // Higher index means older, so oldest first is descending.
        found.sort_by(|a, b| b.0.cmp(&a.0));
        found.into_iter().map(|(_, p)| p).collect()
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
        fs::copy(path, &dest).map_err(|e| format!("cannot snapshot {}: {}", path.display(), e))?;
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
        f.write_all(line.as_bytes())
            .map_err(|e| format!("cannot write journal: {}", e))?;
        // Durability matters here: the undo record must survive the crash that
        // the action it describes might cause.
        f.sync_data()
            .map_err(|e| format!("cannot sync journal: {}", e))?;
        Ok(rec.seq)
    }

    /// All records, oldest first, across rotated journals as well as the live
    /// one, with `undone_by` resolved from the in-memory revert index.
    pub fn read_all(&self) -> Result<Vec<Record>, String> {
        let mut out = Vec::new();
        let mut files = self.archives();
        files.push(self.path.clone());
        for file in files {
            let f = match File::open(&file) {
                Ok(f) => f,
                // An archive that vanished between listing and opening is not
                // an error; a missing live journal is handled by `open`.
                Err(_) => continue,
            };
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
        Ok(self
            .read_all()?
            .into_iter()
            .rev()
            .find(|r| r.is_revertible()))
    }
}

impl Journal {
    /// Age out history and reclaim snapshot space.
    ///
    /// The invariant, and the only one that matters: **a snapshot is never
    /// removed while an action that can still be undone depends on it.** Undo
    /// degrades by losing the oldest history, never by finding a journal entry
    /// whose backup has gone.
    pub fn prune(&self, keep: Retention) -> Result<PruneReport, String> {
        self.prune_with(keep, false)
    }

    /// Same, but `dry_run` computes the report and removes nothing.
    ///
    /// This exists because a preview that under-reports is worse than no
    /// preview: it teaches you the operation is harmless and then it is not.
    pub fn prune_with(&self, keep: Retention, dry_run: bool) -> Result<PruneReport, String> {
        let mut report = PruneReport::default();

        // 1. Rotate the live journal if it has grown past the limit.
        let live = count_lines(&self.path);
        if live > keep.max_records {
            report.rotated = true;
            if !dry_run {
                // Shift the existing archives down one, then move the live
                // journal into slot 1. Nothing is ever rewritten in place --
                // that is what keeps the log append-only.
                let mut existing = self.archives();
                existing.reverse(); // newest first, so shifting starts at the top
                for path in existing {
                    let n: u64 = path
                        .file_name()
                        .and_then(|s| s.to_str())
                        .and_then(|s| s.strip_prefix("journal."))
                        .and_then(|s| s.strip_suffix(".jsonl"))
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0);
                    let next = self.dir().join(format!("journal.{}.jsonl", n + 1));
                    let _ = fs::rename(&path, &next);
                }
                let first = self.dir().join("journal.1.jsonl");
                fs::rename(&self.path, &first)
                    .map_err(|e| format!("cannot rotate the journal: {}", e))?;
                File::create(&self.path)
                    .map_err(|e| format!("cannot start a new journal: {}", e))?;
            }
        }

        // 2. Drop archives beyond the limit, oldest first.
        let archives = self.archives();
        if archives.len() > keep.max_archives {
            for path in &archives[..archives.len() - keep.max_archives] {
                report.records_dropped += count_lines(path);
                report.bytes_reclaimed += fs::metadata(path).map(|m| m.len()).unwrap_or(0);
                if dry_run || fs::remove_file(path).is_ok() {
                    report.archives_removed += 1;
                }
            }
        }

        // 3. Remove snapshots nothing can still restore from.
        let records = self.read_all()?;
        let mut needed: Vec<String> = Vec::new();
        for r in &records {
            if let Undo::RestoreFile {
                backup: Some(b), ..
            } = &r.undo
            {
                if r.is_revertible() {
                    needed.push(b.clone());
                }
            }
        }
        report.kept_for_undo = needed.len();

        let mut orphans: Vec<(u64, u64, PathBuf)> = Vec::new(); // (mtime, size, path)
        if let Ok(entries) = fs::read_dir(&self.backups) {
            for e in entries.flatten() {
                let path = e.path();
                let as_str = path.to_string_lossy().to_string();
                if needed.iter().any(|n| n == &as_str) {
                    continue;
                }
                let md = match e.metadata() {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                let mtime = md
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                orphans.push((mtime, md.len(), path));
            }
        }
        // Unreferenced snapshots go regardless of size: nothing can restore
        // from them, so they are pure cost.
        orphans.sort_by_key(|(mtime, _, _)| *mtime);
        for (_, size, path) in &orphans {
            if dry_run || fs::remove_file(path).is_ok() {
                report.backups_removed += 1;
                report.bytes_reclaimed += size;
            }
        }

        // 4. If the snapshots that *are* still needed exceed the ceiling, drop
        //    the oldest of them too, and accept that those actions can no
        //    longer be undone. Running out of disk is the worse failure.
        let mut live_backups: Vec<(u64, u64, PathBuf)> = Vec::new();
        let mut total = 0u64;
        for b in &needed {
            let path = PathBuf::from(b);
            if let Ok(md) = fs::metadata(&path) {
                let mtime = md
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                total += md.len();
                live_backups.push((mtime, md.len(), path));
            }
        }
        if total > keep.max_backup_bytes {
            live_backups.sort_by_key(|(mtime, _, _)| *mtime);
            for (_, size, path) in &live_backups {
                if total <= keep.max_backup_bytes {
                    break;
                }
                if dry_run || fs::remove_file(path).is_ok() {
                    report.backups_removed += 1;
                    report.bytes_reclaimed += size;
                    report.kept_for_undo = report.kept_for_undo.saturating_sub(1);
                    total = total.saturating_sub(*size);
                }
            }
        }

        Ok(report)
    }

    /// Total bytes the journal and its snapshots occupy.
    pub fn disk_usage(&self) -> u64 {
        let mut total = fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0);
        for a in self.archives() {
            total += fs::metadata(&a).map(|m| m.len()).unwrap_or(0);
        }
        if let Ok(entries) = fs::read_dir(&self.backups) {
            for e in entries.flatten() {
                total += e.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
        total
    }
}

fn count_lines(path: &Path) -> usize {
    File::open(path)
        .map(|f| {
            BufReader::new(f)
                .lines()
                .map_while(Result::ok)
                .filter(|l| !l.trim().is_empty())
                .count()
        })
        .unwrap_or(0)
}

/// If this record is itself a revert, which sequence did it revert?
fn revert_target(rec: &Record) -> Option<u64> {
    if !rec.capability.starts_with("journal.revert") {
        return None;
    }
    rec.capability.split(':').nth(1)?.parse().ok()
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
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
        let p =
            std::env::temp_dir().join(format!("nous-journal-test-{}-{}", tag, std::process::id()));
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
            assert_eq!(
                j.append(rec("fs.read:/a", Outcome::Executed, Undo::None))
                    .unwrap(),
                1
            );
            assert_eq!(
                j.append(rec("fs.read:/b", Outcome::Executed, Undo::None))
                    .unwrap(),
                2
            );
        }
        let j2 = Journal::open(&dir).unwrap();
        assert_eq!(
            j2.append(rec("fs.read:/c", Outcome::Executed, Undo::None))
                .unwrap(),
            3
        );
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
        j.append(rec(
            "fs.write:/home/joey/a.txt",
            Outcome::Confirmed,
            undo.clone(),
        ))
        .unwrap();
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
        let u = Undo::RemoveDir {
            path: "/tmp/x".into(),
        };
        j.append(rec("fs.mkdir:/tmp/x", Outcome::Refused, u))
            .unwrap();
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
                Undo::RestoreFile {
                    path: "/tmp/a".into(),
                    backup: None,
                    existed: false,
                },
            ))
            .unwrap();
        assert!(j.last_revertible().unwrap().is_some());

        j.append(rec(
            &format!("journal.revert:{}", target),
            Outcome::Executed,
            Undo::None,
        ))
        .unwrap();

        let original = j.get(target).unwrap().unwrap();
        assert_eq!(original.undone_by, Some(2));
        assert!(
            !original.is_revertible(),
            "an undone action must not be undoable twice"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn snapshot_copies_only_existing_files() {
        let dir = tmpdir("snap");
        let j = Journal::open(&dir).unwrap();
        let src = dir.join("subject.txt");
        assert_eq!(j.snapshot(&src).unwrap(), None);
        fs::write(&src, b"before").unwrap();
        let backup = j
            .snapshot(&src)
            .unwrap()
            .expect("existing file yields a backup");
        assert_eq!(fs::read_to_string(&backup).unwrap(), "before");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn corrupt_lines_do_not_poison_the_log() {
        let dir = tmpdir("corrupt");
        let j = Journal::open(&dir).unwrap();
        j.append(rec("fs.read:/a", Outcome::Executed, Undo::None))
            .unwrap();
        let mut f = OpenOptions::new()
            .append(true)
            .open(dir.join("journal.jsonl"))
            .unwrap();
        f.write_all(b"{\"seq\": trunca\n").unwrap();
        drop(f);
        j.append(rec("fs.read:/b", Outcome::Executed, Undo::None))
            .unwrap();
        assert_eq!(j.read_all().unwrap().len(), 2);
        fs::remove_dir_all(&dir).ok();
    }

    // ------------------------------------------------------------ retention

    /// A record that snapshots a real file, so pruning has something to weigh.
    fn rec_with_backup(j: &Journal, dir: &Path, name: &str, bytes: usize) -> Record {
        let src = dir.join(name);
        fs::write(&src, vec![b'x'; bytes]).unwrap();
        let backup = j.snapshot(&src).unwrap();
        Record {
            undo: Undo::RestoreFile {
                path: src.to_string_lossy().to_string(),
                backup,
                existed: true,
            },
            ..rec(
                &format!("fs.write:{}", src.display()),
                Outcome::Executed,
                Undo::None,
            )
        }
    }

    #[test]
    fn rotation_ages_history_out_without_losing_it() {
        let dir = tmpdir("rotate");
        let j = Journal::open(&dir).unwrap();
        for i in 0..30 {
            j.append(rec(
                &format!("fs.read:/a{}", i),
                Outcome::Executed,
                Undo::None,
            ))
            .unwrap();
        }

        let keep = Retention {
            max_records: 10,
            max_archives: 4,
            ..Default::default()
        };
        let report = j.prune(keep).unwrap();
        assert!(report.rotated);

        // Everything is still readable, and still in order.
        let all = j.read_all().unwrap();
        assert_eq!(all.len(), 30, "rotation must not lose history");
        assert_eq!(all[0].seq, 1);
        assert_eq!(all[29].seq, 30);

        // And new records keep counting from where they left off.
        assert_eq!(
            j.append(rec("fs.read:/new", Outcome::Executed, Undo::None))
                .unwrap(),
            31
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn archives_beyond_the_limit_are_dropped_oldest_first() {
        let dir = tmpdir("archives");
        let j = Journal::open(&dir).unwrap();
        let keep = Retention {
            max_records: 5,
            max_archives: 2,
            ..Default::default()
        };

        // Five rotations, so three archives should be discarded.
        for round in 0..5 {
            for i in 0..6 {
                j.append(rec(
                    &format!("fs.read:/r{}-{}", round, i),
                    Outcome::Executed,
                    Undo::None,
                ))
                .unwrap();
            }
            j.prune(keep).unwrap();
        }

        let archives: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("journal."))
            .filter(|e| e.file_name().to_string_lossy() != "journal.jsonl")
            .collect();
        assert!(
            archives.len() <= 2,
            "kept {} archives, expected at most 2",
            archives.len()
        );

        // The most recent history survives; the oldest is what went.
        let all = j.read_all().unwrap();
        assert!(
            all.iter().any(|r| r.capability.contains("/r4-")),
            "recent history must remain"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_snapshot_needed_for_undo_is_never_removed() {
        // The invariant the whole design rests on. Pruning may lose old
        // history; it may never leave a journal entry pointing at a snapshot
        // that is gone.
        let dir = tmpdir("undo-safety");
        let work = dir.join("work");
        fs::create_dir_all(&work).unwrap();
        let j = Journal::open(&dir).unwrap();

        let live = rec_with_backup(&j, &work, "still-undoable.txt", 4096);
        let backup_path = match &live.undo {
            Undo::RestoreFile {
                backup: Some(b), ..
            } => PathBuf::from(b),
            _ => panic!("expected a snapshot"),
        };
        let seq = j.append(live).unwrap();

        let report = j
            .prune(Retention {
                max_records: 1,
                max_archives: 0,
                ..Default::default()
            })
            .unwrap();

        assert!(
            backup_path.exists(),
            "a snapshot backing a revertible action must survive"
        );
        assert_eq!(report.kept_for_undo, 1);
        assert!(j.get(seq).unwrap().unwrap().is_revertible());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn snapshots_nothing_can_restore_from_are_reclaimed() {
        let dir = tmpdir("orphans");
        let work = dir.join("work");
        fs::create_dir_all(&work).unwrap();
        let j = Journal::open(&dir).unwrap();

        // An action that has already been undone no longer needs its snapshot.
        let done = rec_with_backup(&j, &work, "already-undone.txt", 8192);
        let orphan = match &done.undo {
            Undo::RestoreFile {
                backup: Some(b), ..
            } => PathBuf::from(b),
            _ => panic!("expected a snapshot"),
        };
        let seq = j.append(done).unwrap();
        j.append(rec(
            &format!("journal.revert:{}", seq),
            Outcome::Executed,
            Undo::None,
        ))
        .unwrap();

        assert!(orphan.exists());
        let report = j.prune(Retention::default()).unwrap();

        assert!(
            !orphan.exists(),
            "an undone action's snapshot is dead weight"
        );
        assert_eq!(report.backups_removed, 1);
        assert!(report.bytes_reclaimed >= 8192);
        assert_eq!(report.kept_for_undo, 0);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn undo_still_works_after_a_prune() {
        let dir = tmpdir("prune-then-undo");
        let work = dir.join("work");
        fs::create_dir_all(&work).unwrap();
        let j = Journal::open(&dir).unwrap();

        let target = work.join("notes.md");
        fs::write(&target, b"original").unwrap();
        let backup = j.snapshot(&target).unwrap();
        fs::write(&target, b"changed").unwrap();
        j.append(Record {
            undo: Undo::RestoreFile {
                path: target.to_string_lossy().to_string(),
                backup: backup.clone(),
                existed: true,
            },
            ..rec("fs.write:/notes", Outcome::Executed, Undo::None)
        })
        .unwrap();

        // Fill the journal so a prune definitely runs, then undo.
        for i in 0..50 {
            j.append(rec(
                &format!("fs.read:/n{}", i),
                Outcome::Executed,
                Undo::None,
            ))
            .unwrap();
        }
        j.prune(Retention {
            max_records: 10,
            max_archives: 2,
            ..Default::default()
        })
        .unwrap();

        let record = j
            .last_revertible()
            .unwrap()
            .expect("the write should still be undoable");
        match &record.undo {
            Undo::RestoreFile {
                backup: Some(b), ..
            } => {
                fs::copy(b, &target).unwrap();
            }
            other => panic!("unexpected undo: {:?}", other),
        }
        assert_eq!(fs::read_to_string(&target).unwrap(), "original");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_backup_ceiling_is_enforced_even_against_undoable_actions() {
        // Running out of disk is the worse failure. When the snapshot store
        // exceeds its ceiling the oldest go, and those actions stop being
        // undoable -- deliberately, and reported.
        let dir = tmpdir("ceiling");
        let work = dir.join("work");
        fs::create_dir_all(&work).unwrap();
        let j = Journal::open(&dir).unwrap();

        for i in 0..5 {
            let r = rec_with_backup(&j, &work, &format!("big{}.bin", i), 20_000);
            j.append(r).unwrap();
        }
        let before = j.disk_usage();
        assert!(
            before > 90_000,
            "expected the snapshots to be sizeable, got {}",
            before
        );

        let report = j
            .prune(Retention {
                max_backup_bytes: 40_000,
                ..Default::default()
            })
            .unwrap();

        assert!(report.backups_removed > 0);
        assert!(j.disk_usage() < before);
        assert!(
            report.describe().contains("reclaimed"),
            "{}",
            report.describe()
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_preview_reports_exactly_what_the_real_prune_would_do() {
        // The bug this was written for: the preview skipped the journal
        // entirely, said "nothing to clean up", and then the real run
        // reclaimed a megabyte. A preview that under-reports is worse than
        // none -- it teaches you the operation is harmless.
        let dir = tmpdir("dryrun");
        let work = dir.join("work");
        fs::create_dir_all(&work).unwrap();
        let j = Journal::open(&dir).unwrap();

        for i in 0..4 {
            let r = rec_with_backup(&j, &work, &format!("f{}.bin", i), 10_000);
            let seq = j.append(r).unwrap();
            // Undo it, so its snapshot becomes reclaimable.
            j.append(rec(
                &format!("journal.revert:{}", seq),
                Outcome::Executed,
                Undo::None,
            ))
            .unwrap();
        }

        let before = j.disk_usage();
        let preview = j.prune_with(Retention::default(), true).unwrap();
        assert!(
            preview.backups_removed > 0,
            "the preview must see the reclaimable snapshots"
        );
        assert_eq!(j.disk_usage(), before, "a preview must remove nothing");

        let real = j.prune_with(Retention::default(), false).unwrap();
        assert_eq!(
            real.backups_removed, preview.backups_removed,
            "preview must match reality"
        );
        assert_eq!(real.bytes_reclaimed, preview.bytes_reclaimed);
        assert!(j.disk_usage() < before);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn pruning_a_healthy_journal_does_nothing() {
        let dir = tmpdir("noop");
        let j = Journal::open(&dir).unwrap();
        j.append(rec("fs.read:/a", Outcome::Executed, Undo::None))
            .unwrap();
        let report = j.prune(Retention::default()).unwrap();
        assert!(report.is_empty(), "{:?}", report);
        assert_eq!(report.describe(), "nothing to prune");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn formats_byte_counts_readably() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1536), "1.5 KB");
        assert_eq!(human_bytes(3 * 1024 * 1024 * 1024), "3.0 GB");
    }

    #[test]
    fn formats_timestamps_in_utc() {
        assert_eq!(format_ts(0), "1970-01-01 00:00:00");
        assert_eq!(format_ts(1_700_000_000), "2023-11-14 22:13:20");
        // A leap day, because the calendar arithmetic is the easy thing to get wrong.
        assert_eq!(format_ts(1_709_164_800), "2024-02-29 00:00:00");
    }
}
