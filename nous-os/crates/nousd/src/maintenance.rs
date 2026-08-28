//! Housekeeping the system does on itself.
//!
//! NOUS keeps a lot on your behalf: a journal of everything it did, a snapshot
//! of every file before it changed it, a trash store so deletion is reversible,
//! thumbnails, screenshots. Every one of those is the right call individually,
//! and together they are an unbounded copy of your disk.
//!
//! A system that tells you your disk is 92% full has no business being the
//! reason. So it prunes itself, on the same timer it watches you with, and it
//! says what it reclaimed rather than doing it silently.

use nous_core::journal::{human_bytes, now_secs, PruneReport, Retention};
use nous_core::json::{json_obj, Json};
use nous_core::{Config, Journal};
use std::path::{Path, PathBuf};

/// What one maintenance pass did.
#[derive(Debug, Default, Clone)]
pub struct Report {
    pub journal: PruneReport,
    pub trash_removed: usize,
    pub thumbs_removed: usize,
    pub screenshots_removed: usize,
    pub bytes_reclaimed: u64,
}

impl Report {
    pub fn is_empty(&self) -> bool {
        self.journal.is_empty()
            && self.trash_removed == 0
            && self.thumbs_removed == 0
            && self.screenshots_removed == 0
    }

    pub fn total_bytes(&self) -> u64 {
        self.bytes_reclaimed + self.journal.bytes_reclaimed
    }

    pub fn describe(&self) -> String {
        if self.is_empty() {
            return "nothing to clean up".to_string();
        }
        let mut parts = Vec::new();
        if !self.journal.is_empty() {
            parts.push(self.journal.describe());
        }
        if self.trash_removed > 0 {
            parts.push(format!(
                "emptied {} item(s) from the trash store",
                self.trash_removed
            ));
        }
        if self.thumbs_removed > 0 {
            parts.push(format!("dropped {} thumbnail(s)", self.thumbs_removed));
        }
        if self.screenshots_removed > 0 {
            parts.push(format!(
                "dropped {} screenshot(s)",
                self.screenshots_removed
            ));
        }
        format!(
            "{} ({} total)",
            parts.join("; "),
            human_bytes(self.total_bytes())
        )
    }

    pub fn to_json(&self) -> Json {
        json_obj([
            ("rotated", self.journal.rotated.into()),
            ("archives_removed", self.journal.archives_removed.into()),
            ("backups_removed", self.journal.backups_removed.into()),
            ("kept_for_undo", self.journal.kept_for_undo.into()),
            ("trash_removed", self.trash_removed.into()),
            ("thumbs_removed", self.thumbs_removed.into()),
            ("screenshots_removed", self.screenshots_removed.into()),
            ("bytes_reclaimed", self.total_bytes().into()),
            ("reclaimed", human_bytes(self.total_bytes()).into()),
            ("summary", self.describe().into()),
        ])
    }
}

pub fn retention_from(cfg: &Config) -> Retention {
    Retention {
        max_records: cfg.u64_or("retain.journal_records", 20_000) as usize,
        max_archives: cfg.u64_or("retain.journal_archives", 4) as usize,
        max_backup_bytes: cfg.u64_or("retain.backup_mb", 2048) * 1024 * 1024,
    }
}

const DAY: u64 = 86_400;

/// Run one pass.
///
/// `dry_run` reports what it would reclaim and removes nothing, which is what
/// `nousctl maintenance` shows you before you agree to it.
pub fn run(journal: &Journal, cfg: &Config, state: &Path, dry_run: bool) -> Report {
    let mut report = Report::default();

    match journal.prune_with(retention_from(cfg), dry_run) {
        Ok(p) => report.journal = p,
        Err(e) => nous_core::log_warn!("maintenance", "could not prune the journal: {}", e),
    }

    // The trash store. Thirty days is what every desktop does, and for the same
    // reason: long enough to notice a mistake, short enough to end.
    let trash_days = cfg.u64_or("retain.trash_days", 30);
    let (n, bytes) = expire(&state.join("trash"), trash_days * DAY, dry_run);
    report.trash_removed = n;
    report.bytes_reclaimed += bytes;

    // Thumbnails regenerate on demand, so they can go sooner.
    let thumb_days = cfg.u64_or("retain.thumbnail_days", 60);
    let (n, bytes) = expire(&state.join("media/thumbs"), thumb_days * DAY, dry_run);
    report.thumbs_removed = n;
    report.bytes_reclaimed += bytes;

    // Screenshots the system took itself. Ones you asked for and kept are in
    // your own folders and are none of its business.
    let shot_days = cfg.u64_or("retain.screenshot_days", 14);
    let (n, bytes) = expire(&state.join("screenshots"), shot_days * DAY, dry_run);
    report.screenshots_removed = n;
    report.bytes_reclaimed += bytes;

    report
}

/// Remove entries in `dir` older than `max_age`. Returns how many and how much.
///
/// Directories are removed whole, because a trashed folder is one item to the
/// person who deleted it.
pub fn expire(dir: &Path, max_age: u64, dry_run: bool) -> (usize, u64) {
    let now = now_secs();
    let mut count = 0;
    let mut bytes = 0;

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return (0, 0),
    };
    for entry in entries.flatten() {
        let md = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let age = md
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| now.saturating_sub(d.as_secs()))
            .unwrap_or(0);
        if age <= max_age {
            continue;
        }
        let path = entry.path();
        let size = if md.is_dir() {
            dir_size(&path)
        } else {
            md.len()
        };
        if dry_run {
            count += 1;
            bytes += size;
            continue;
        }
        let removed = if md.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        if removed.is_ok() {
            count += 1;
            bytes += size;
        }
    }
    (count, bytes)
}

fn dir_size(dir: &Path) -> u64 {
    let mut total = 0;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            match e.metadata() {
                Ok(m) if m.is_dir() => total += dir_size(&e.path()),
                Ok(m) => total += m.len(),
                Err(_) => {}
            }
        }
    }
    total
}

/// Everything NOUS is currently storing on your behalf.
pub fn usage(journal: &Journal, state: &Path) -> Json {
    let trash = dir_size(&state.join("trash"));
    let thumbs = dir_size(&state.join("media/thumbs"));
    let shots = dir_size(&state.join("screenshots"));
    let index = dir_size(&state.join("index"));
    let journal_bytes = journal.disk_usage();
    let total = trash + thumbs + shots + index + journal_bytes;

    json_obj([
        ("journal_bytes", journal_bytes.into()),
        ("trash_bytes", trash.into()),
        ("thumbnail_bytes", thumbs.into()),
        ("screenshot_bytes", shots.into()),
        ("index_bytes", index.into()),
        ("total_bytes", total.into()),
        ("total", human_bytes(total).into()),
        ("path", state.to_string_lossy().to_string().into()),
    ])
}

pub fn state_root() -> PathBuf {
    nous_core::ipc::state_dir()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{Duration, SystemTime};

    fn scratch(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("nous-maint-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    /// Backdate a path so age-based rules can be tested without waiting.
    fn age(path: &Path, days: u64) {
        let when = SystemTime::now() - Duration::from_secs(days * DAY);
        let ft = filetime_secs(when);
        set_mtime(path, ft);
    }

    fn filetime_secs(t: SystemTime) -> u64 {
        t.duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    fn set_mtime(path: &Path, secs: u64) {
        // No dependencies, so utimensat directly.
        #[repr(C)]
        struct Timespec {
            tv_sec: i64,
            tv_nsec: i64,
        }
        extern "C" {
            fn utimensat(dirfd: i32, path: *const u8, times: *const Timespec, flags: i32) -> i32;
        }
        let mut c: Vec<u8> = path.to_string_lossy().as_bytes().to_vec();
        c.push(0);
        let times = [
            Timespec {
                tv_sec: secs as i64,
                tv_nsec: 0,
            },
            Timespec {
                tv_sec: secs as i64,
                tv_nsec: 0,
            },
        ];
        // SAFETY: `c` is NUL-terminated and `times` is a two-element array as
        // utimensat expects. AT_FDCWD is -100 on Linux.
        unsafe {
            utimensat(-100, c.as_ptr(), times.as_ptr(), 0);
        }
    }

    #[test]
    fn old_trash_is_emptied_and_recent_trash_is_not() {
        let dir = scratch("trash");
        let trash = dir.join("trash");
        fs::create_dir_all(&trash).unwrap();

        let old = trash.join("deleted-ages-ago.bin");
        let recent = trash.join("deleted-yesterday.bin");
        fs::write(&old, vec![b'x'; 5000]).unwrap();
        fs::write(&recent, vec![b'x'; 5000]).unwrap();
        age(&old, 45);
        age(&recent, 1);

        let (n, bytes) = expire(&trash, 30 * DAY, false);
        assert_eq!(n, 1);
        assert_eq!(bytes, 5000);
        assert!(!old.exists(), "a month-old deletion should be gone");
        assert!(
            recent.exists(),
            "yesterday's deletion must still be recoverable"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_trashed_folder_counts_as_one_item() {
        let dir = scratch("trashdir");
        let trash = dir.join("trash");
        fs::create_dir_all(trash.join("a-folder/nested")).unwrap();
        fs::write(trash.join("a-folder/one.txt"), vec![b'x'; 100]).unwrap();
        fs::write(trash.join("a-folder/nested/two.txt"), vec![b'x'; 200]).unwrap();
        age(&trash.join("a-folder"), 60);

        let (n, bytes) = expire(&trash, 30 * DAY, false);
        assert_eq!(n, 1, "one folder is one item to the person who deleted it");
        assert_eq!(bytes, 300, "but its whole size is reclaimed");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_dry_run_reports_without_removing() {
        let dir = scratch("dry");
        let trash = dir.join("trash");
        fs::create_dir_all(&trash).unwrap();
        let old = trash.join("old.bin");
        fs::write(&old, vec![b'x'; 4096]).unwrap();
        age(&old, 90);

        let (n, bytes) = expire(&trash, 30 * DAY, true);
        assert_eq!(n, 1);
        assert_eq!(bytes, 4096);
        assert!(old.exists(), "a dry run must remove nothing");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_directory_is_not_an_error() {
        assert_eq!(
            expire(Path::new("/nonexistent/nous/trash"), DAY, false),
            (0, 0)
        );
    }

    #[test]
    fn a_full_pass_reports_what_it_reclaimed() {
        let dir = scratch("full");
        let journal = Journal::open(&dir.join("journal")).unwrap();
        for sub in ["trash", "media/thumbs", "screenshots"] {
            let d = dir.join(sub);
            fs::create_dir_all(&d).unwrap();
            let f = d.join("stale.bin");
            fs::write(&f, vec![b'x'; 2048]).unwrap();
            age(&f, 365);
        }

        let report = run(&journal, &Config::with_defaults(), &dir, false);
        assert_eq!(report.trash_removed, 1);
        assert_eq!(report.thumbs_removed, 1);
        assert_eq!(report.screenshots_removed, 1);
        assert_eq!(report.bytes_reclaimed, 3 * 2048);
        let text = report.describe();
        assert!(text.contains("trash store"), "{text}");
        assert!(text.contains("6.0 KB"), "{text}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_preview_matches_what_a_real_pass_reclaims() {
        let dir = scratch("preview-match");
        let journal = Journal::open(&dir.join("journal")).unwrap();
        for sub in ["trash", "media/thumbs", "screenshots"] {
            let d = dir.join(sub);
            fs::create_dir_all(&d).unwrap();
            let f = d.join("stale.bin");
            fs::write(&f, vec![b'x'; 4096]).unwrap();
            age(&f, 365);
        }
        let cfg = Config::with_defaults();

        let preview = run(&journal, &cfg, &dir, true);
        assert!(!preview.is_empty(), "the preview must see the stale files");
        assert!(
            dir.join("trash/stale.bin").exists(),
            "a preview removes nothing"
        );

        let real = run(&journal, &cfg, &dir, false);
        assert_eq!(
            real.total_bytes(),
            preview.total_bytes(),
            "preview must match reality"
        );
        assert!(!dir.join("trash/stale.bin").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_clean_system_reports_nothing_to_do() {
        let dir = scratch("clean");
        let journal = Journal::open(&dir.join("journal")).unwrap();
        let report = run(&journal, &Config::with_defaults(), &dir, false);
        assert!(report.is_empty());
        assert_eq!(report.describe(), "nothing to clean up");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn usage_accounts_for_everything_it_keeps() {
        let dir = scratch("usage");
        let journal = Journal::open(&dir.join("journal")).unwrap();
        fs::create_dir_all(dir.join("trash")).unwrap();
        fs::write(dir.join("trash/thing.bin"), vec![b'x'; 10_000]).unwrap();

        let u = usage(&journal, &dir);
        assert_eq!(u.get("trash_bytes").unwrap().as_u64(), Some(10_000));
        assert!(u.get("total_bytes").unwrap().as_u64().unwrap() >= 10_000);
        assert!(
            u.str_or("total", "").ends_with("KB"),
            "{}",
            u.str_or("total", "")
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn retention_reads_its_limits_from_configuration() {
        let mut cfg = Config::with_defaults();
        cfg.set("retain.journal_records", "500");
        cfg.set("retain.backup_mb", "64");
        let r = retention_from(&cfg);
        assert_eq!(r.max_records, 500);
        assert_eq!(r.max_backup_bytes, 64 * 1024 * 1024);
    }
}
