//! The file index: search that understands what you meant, without needing a
//! model to answer every query.
//!
//! Ranking is BM25-style over three fields — filename, path, and the head of
//! the content — with a recency boost. It is deterministic and it runs in
//! milliseconds, which matters because search is typed into character by
//! character. A model is useful for *interpreting* a vague request; it is the
//! wrong tool for ranking ten thousand filenames on every keystroke.

use crate::exec::fsops;
use nous_core::journal::now_secs;
use nous_core::json::{json_obj, parse, Json};
use nous_core::Config;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How much of a file's text is kept for searching.
const CONTENT_HEAD_BYTES: usize = 4096;

#[derive(Debug, Clone)]
pub struct Doc {
    pub path: PathBuf,
    pub name: String,
    pub kind: String,
    pub size: u64,
    pub modified: u64,
    /// Lowercased searchable text: name, path segments, and a content head.
    pub text: String,
}

impl Doc {
    fn to_json(&self) -> Json {
        json_obj([
            ("path", self.path.to_string_lossy().to_string().into()),
            ("name", self.name.clone().into()),
            ("kind", self.kind.clone().into()),
            ("size", self.size.into()),
            ("modified", self.modified.into()),
            ("text", self.text.clone().into()),
        ])
    }

    fn from_json(v: &Json) -> Doc {
        Doc {
            path: PathBuf::from(v.str_or("path", "")),
            name: v.str_or("name", "").to_string(),
            kind: v.str_or("kind", "file").to_string(),
            size: v.get("size").and_then(|s| s.as_u64()).unwrap_or(0),
            modified: v.get("modified").and_then(|s| s.as_u64()).unwrap_or(0),
            text: v.str_or("text", "").to_string(),
        }
    }
}

#[derive(Debug, Default)]
pub struct Index {
    pub docs: Vec<Doc>,
    pub built: u64,
}

/// What the cache is holding.
struct Held {
    path: PathBuf,
    /// The file's modification time, so a rebuild is noticed.
    stamp: u64,
    index: Arc<Index>,
    /// When it was last wanted. The sweeper reads this and nothing else.
    touched: Instant,
}

/// How long an untouched index is kept before the memory goes back.
///
/// Long enough to cover someone typing, thinking, and typing again; short
/// enough that a daemon nobody is using is not sitting on a hundred megabytes.
const IDLE_RELEASE: Duration = Duration::from_secs(25);

/// How often to look. Rarely: this thread exists to give memory back, and one
/// that woke often would be its own small cost on the machines that need it.
const SWEEP_EVERY: Duration = Duration::from_secs(5);

fn cache() -> &'static Mutex<Option<Held>> {
    static CACHE: OnceLock<Mutex<Option<Held>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

fn lock(m: &'static Mutex<Option<Held>>) -> std::sync::MutexGuard<'static, Option<Held>> {
    match m.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn stamp_of(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        // No file yet: a stamp that never matches, so an index written a
        // moment from now is still noticed.
        .unwrap_or(0)
}

/// One sleeping thread that hands the index's memory back when nobody has
/// wanted it for a while.
///
/// Started on the first search rather than at boot, so a daemon that is never
/// searched never starts it.
fn start_sweeper() {
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(|| {
        std::thread::Builder::new()
            .name("index-release".to_string())
            .spawn(|| loop {
                std::thread::sleep(SWEEP_EVERY);
                let mut held = lock(cache());
                let idle = held
                    .as_ref()
                    .map(|h| h.touched.elapsed() >= IDLE_RELEASE)
                    .unwrap_or(false);
                if idle {
                    *held = None;
                }
            })
            .ok();
    });
}

/// Split into lowercase terms, breaking on the separators that appear in
/// filenames — `holiday_photos-2024.tar.gz` should match "holiday" and "2024".
pub fn tokenize(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_ascii_lowercase())
        .collect()
}

impl Index {
    pub fn path() -> PathBuf {
        nous_core::ipc::state_dir().join("index/files.json")
    }

    pub fn load() -> Index {
        Index::load_from(&Index::path())
    }

    /// The index, kept in memory only for as long as it is being used.
    ///
    /// Searching used to read and parse the whole file every time it was
    /// asked. That was tolerable when a search was something you typed and
    /// pressed Return on; it is not when the interface searches on every
    /// keystroke, which for a home directory of any size means parsing
    /// megabytes per letter.
    ///
    /// Holding it forever instead would be worse. A machine with fifty
    /// thousand files has a hundred megabytes of index, and a daemon that
    /// grabs that the first time anyone types a letter and never gives it back
    /// has made itself unwelcome on exactly the machines this is meant to run
    /// on.
    ///
    /// So: held while it is wanted, released when it stops being. Typing is a
    /// burst — a dozen searches in as many seconds, then nothing for an hour.
    /// The burst is served from memory and the hour costs nothing. The price
    /// is one re-parse when someone comes back, paid once instead of per
    /// keystroke.
    pub fn cached() -> Arc<Index> {
        Index::cached_from(&Index::path())
    }

    pub fn cached_from(path: &Path) -> Arc<Index> {
        let cell = cache();
        let stamp = stamp_of(path);
        let mut held = lock(cell);
        if let Some(h) = held.as_mut() {
            if h.path == path && h.stamp == stamp && stamp != 0 {
                h.touched = Instant::now();
                return Arc::clone(&h.index);
            }
        }
        let idx = Arc::new(Index::load_from(path));
        *held = Some(Held {
            path: path.to_path_buf(),
            stamp,
            index: Arc::clone(&idx),
            touched: Instant::now(),
        });
        drop(held);
        start_sweeper();
        idx
    }

    /// How many bytes the cache is holding, for anything reporting on the
    /// daemon's footprint. Zero when it is holding nothing.
    pub fn cached_bytes() -> usize {
        lock(cache())
            .as_ref()
            .map(|h| h.index.footprint())
            .unwrap_or(0)
    }

    /// Roughly how much memory this index occupies.
    ///
    /// Counts the strings, which are almost all of it: a document is a few
    /// small numbers and up to four kilobytes of content head.
    pub fn footprint(&self) -> usize {
        self.docs
            .iter()
            .map(|d| {
                d.text.len()
                    + d.name.len()
                    + d.kind.len()
                    + d.path.as_os_str().len()
                    + std::mem::size_of::<Doc>()
            })
            .sum()
    }

    /// Let go of whatever is held. Used by the sweeper, and by anything that
    /// knows the machine wants the memory back now.
    pub fn release_cache() {
        *lock(cache()) = None;
    }

    pub fn load_from(path: &Path) -> Index {
        let json = match std::fs::read_to_string(path)
            .ok()
            .and_then(|s| parse(&s).ok())
        {
            Some(j) => j,
            None => return Index::default(),
        };
        Index {
            docs: json
                .arr_or_empty("docs")
                .iter()
                .map(Doc::from_json)
                .collect(),
            built: json.get("built").and_then(|b| b.as_u64()).unwrap_or(0),
        }
    }

    pub fn save(&self) -> Result<(), String> {
        self.save_to(&Index::path())
    }

    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("cannot create index dir: {}", e))?;
        }
        let json = json_obj([
            ("built", self.built.into()),
            (
                "docs",
                Json::Arr(self.docs.iter().map(|d| d.to_json()).collect()),
            ),
        ]);
        std::fs::write(path, json.to_string()).map_err(|e| format!("cannot write index: {}", e))
    }

    /// Walk the configured roots and build the index.
    pub fn build(roots: &[PathBuf], cfg: &Config) -> Index {
        let exclude = cfg.list("index.exclude");
        let max_bytes = cfg.u64_or("index.max_file_bytes", 1_048_576);
        let mut files = Vec::new();
        for r in roots {
            fsops::walk(r, 10, &exclude, &mut files, 60_000);
        }

        let mut docs = Vec::with_capacity(files.len());
        for f in files {
            let md = match std::fs::metadata(&f) {
                Ok(m) => m,
                Err(_) => continue,
            };
            let name = f
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            let kind = fsops::classify(&f, false).to_string();

            // Path segments are searchable, so "invoices 2023" finds a file
            // filed under Documents/Invoices/2023 even if its name says neither.
            let mut text = format!("{} {}", name, f.to_string_lossy().replace('/', " "));
            if is_textual(&kind) && md.len() <= max_bytes {
                if let Ok(bytes) = std::fs::read(&f) {
                    let head = &bytes[..bytes.len().min(CONTENT_HEAD_BYTES)];
                    if let Ok(s) = std::str::from_utf8(head) {
                        text.push(' ');
                        text.push_str(s);
                    }
                }
            }

            docs.push(Doc {
                path: f,
                name,
                kind,
                size: md.len(),
                modified: md
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
                text: text.to_ascii_lowercase(),
            });
        }
        Index {
            docs,
            built: now_secs(),
        }
    }

    /// Rank documents against a query.
    pub fn search(&self, query: &str, kind: Option<&str>, limit: usize) -> Vec<(f64, &Doc)> {
        let terms = tokenize(query);
        let candidates: Vec<&Doc> = self
            .docs
            .iter()
            .filter(|d| kind.map_or(true, |k| d.kind == k))
            .collect();
        if candidates.is_empty() {
            return Vec::new();
        }
        if terms.is_empty() {
            // No query: most recently modified first, which is what an empty
            // search box should show.
            let mut all: Vec<(f64, &Doc)> = candidates
                .into_iter()
                .map(|d| (d.modified as f64, d))
                .collect();
            all.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
            all.truncate(limit);
            return all;
        }

        // Document frequency, so a term appearing in every file counts for little.
        let mut df: HashMap<&str, usize> = HashMap::new();
        for t in &terms {
            let n = candidates
                .iter()
                .filter(|d| d.text.contains(t.as_str()))
                .count();
            df.insert(t.as_str(), n);
        }
        let total = candidates.len() as f64;
        let now = now_secs() as f64;

        let mut scored: Vec<(f64, &Doc)> = candidates
            .into_iter()
            .filter_map(|d| {
                let mut score = 0.0;
                let mut matched = 0;
                for t in &terms {
                    let occurrences = d.text.matches(t.as_str()).count();
                    if occurrences == 0 {
                        continue;
                    }
                    matched += 1;
                    let n = *df.get(t.as_str()).unwrap_or(&1) as f64;
                    let idf = ((total - n + 0.5) / (n + 0.5) + 1.0).ln();
                    // Saturating term frequency: the tenth occurrence should
                    // not count ten times as much as the first.
                    let tf = occurrences as f64 / (occurrences as f64 + 1.5);
                    score += idf * tf;

                    // A hit in the filename is worth far more than one in the body.
                    if d.name.to_ascii_lowercase().contains(t.as_str()) {
                        score += 2.0 * idf;
                    }
                }
                if matched == 0 {
                    return None;
                }
                // Every term present beats a partial match, decisively.
                if matched == terms.len() {
                    score *= 1.8;
                }
                // Gentle recency preference: a file touched today edges out an
                // equally relevant one from three years ago.
                let age_days = ((now - d.modified as f64) / 86_400.0).max(0.0);
                score *= 1.0 + (0.35 / (1.0 + age_days / 30.0));
                Some((score, d))
            })
            .collect();

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);
        scored
    }

    pub fn search_json(&self, query: &str, kind: Option<&str>, limit: usize) -> Json {
        let hits = self.search(query, kind, limit);
        let items: Vec<Json> = hits
            .iter()
            .map(|(score, d)| {
                json_obj([
                    ("path", d.path.to_string_lossy().to_string().into()),
                    ("name", d.name.clone().into()),
                    ("kind", d.kind.clone().into()),
                    ("size", d.size.into()),
                    ("modified", d.modified.into()),
                    ("score", ((score * 1000.0).round() / 1000.0).into()),
                ])
            })
            .collect();
        json_obj([
            ("query", query.into()),
            ("results", Json::Arr(items)),
            ("indexed", self.docs.len().into()),
            ("built", self.built.into()),
        ])
    }
}

fn is_textual(kind: &str) -> bool {
    matches!(kind, "text" | "code" | "config" | "sheet")
}

#[cfg(test)]
mod tests {

    #[test]
    fn the_index_is_not_re_read_from_disk_on_every_search() {
        // The interface searches on every keystroke. Parsing the whole file
        // per letter makes typing slower the more files you own.
        let dir = std::env::temp_dir().join(format!("nous-idx-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("index.json");

        let mut idx = Index::default();
        idx.docs.push(Doc {
            path: PathBuf::from("/a/first.txt"),
            name: "first.txt".into(),
            kind: "file".into(),
            size: 1,
            modified: 1,
            text: "first.txt".into(),
        });
        idx.save_to(&path).unwrap();

        let a = Index::cached_from(&path);
        let b = Index::cached_from(&path);
        assert!(Arc::ptr_eq(&a, &b), "the index was read from disk twice");

        // A rebuild is picked up rather than served stale from the cache.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let mut two = Index::default();
        two.docs.push(Doc {
            path: PathBuf::from("/a/second.txt"),
            name: "second.txt".into(),
            kind: "file".into(),
            size: 1,
            modified: 1,
            text: "second.txt".into(),
        });
        two.save_to(&path).unwrap();
        let c = Index::cached_from(&path);
        assert_eq!(c.docs.len(), 1);
        assert_eq!(
            c.docs[0].name, "second.txt",
            "served a rebuilt index from the cache"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_memory_goes_back_when_nobody_is_searching() {
        // A daemon that grabs a hundred megabytes the first time anyone types
        // a letter and never gives it back has made itself unwelcome on
        // exactly the machines this is meant to run on.
        let dir = std::env::temp_dir().join(format!("nous-idx-release-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("index.json");
        let mut idx = Index::default();
        for i in 0..50 {
            idx.docs.push(Doc {
                path: PathBuf::from(format!("/a/f{i}.txt")),
                name: format!("f{i}.txt"),
                kind: "file".into(),
                size: 1,
                modified: 1,
                text: "x".repeat(4096),
            });
        }
        idx.save_to(&path).unwrap();

        let held = Index::cached_from(&path);
        assert!(
            Index::cached_bytes() > 100_000,
            "the cache reports holding nothing"
        );
        assert_eq!(held.docs.len(), 50);

        Index::release_cache();
        assert_eq!(Index::cached_bytes(), 0, "the memory was not given back");
        // And the next search still works, having paid one re-parse for it.
        assert_eq!(Index::cached_from(&path).docs.len(), 50);
        Index::release_cache();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_the_cache_holds_can_be_measured() {
        // A footprint nobody can read is one nobody will notice growing.
        let mut idx = Index::default();
        assert_eq!(idx.footprint(), 0);
        idx.docs.push(Doc {
            path: PathBuf::from("/a/b.txt"),
            name: "b.txt".into(),
            kind: "file".into(),
            size: 1,
            modified: 1,
            text: "y".repeat(1000),
        });
        assert!(idx.footprint() > 1000, "{}", idx.footprint());
    }

    #[test]
    fn an_index_that_is_not_there_does_not_get_cached_as_empty() {
        // Otherwise the first search before the first build poisons every
        // search afterwards.
        let path = std::env::temp_dir().join("nous-idx-missing.json");
        let _ = std::fs::remove_file(&path);
        let a = Index::cached_from(&path);
        assert!(a.docs.is_empty());
        let b = Index::cached_from(&path);
        assert!(!Arc::ptr_eq(&a, &b), "cached the absence of an index");
    }
    use super::*;
    use std::fs;

    fn scratch(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("nous-index-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn build(dir: &Path) -> Index {
        Index::build(
            std::slice::from_ref(&dir.to_path_buf()),
            &Config::with_defaults(),
        )
    }

    #[test]
    fn tokenizes_filenames_the_way_people_write_them() {
        assert_eq!(
            tokenize("holiday_photos-2024.tar.gz"),
            ["holiday", "photos", "2024", "tar", "gz"]
        );
        assert!(tokenize("   ").is_empty());
    }

    #[test]
    fn finds_files_by_name() {
        let dir = scratch("byname");
        fs::write(dir.join("tax-return-2023.pdf"), b"x").unwrap();
        fs::write(dir.join("holiday.jpg"), b"x").unwrap();
        let idx = build(&dir);

        let hits = idx.search("tax return", None, 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].1.name, "tax-return-2023.pdf");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn finds_files_by_content() {
        let dir = scratch("bycontent");
        fs::write(dir.join("notes.md"), b"the mitochondria is the powerhouse").unwrap();
        fs::write(dir.join("other.md"), b"unrelated text").unwrap();
        let idx = build(&dir);

        let hits = idx.search("mitochondria", None, 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].1.name, "notes.md");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_name_match_outranks_a_body_match() {
        let dir = scratch("nameweight");
        fs::write(dir.join("budget.md"), b"nothing relevant here at all").unwrap();
        fs::write(dir.join("notes.md"), b"budget budget budget budget").unwrap();
        let idx = build(&dir);

        let hits = idx.search("budget", None, 10);
        assert_eq!(
            hits[0].1.name, "budget.md",
            "the file *called* budget should win"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn path_segments_are_searchable() {
        let dir = scratch("bypath");
        fs::create_dir_all(dir.join("Invoices/2023")).unwrap();
        fs::write(dir.join("Invoices/2023/scan001.pdf"), b"x").unwrap();
        let idx = build(&dir);

        let hits = idx.search("invoices 2023", None, 10);
        assert_eq!(
            hits.len(),
            1,
            "a file should be findable by where it is filed"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn matching_every_term_beats_matching_some() {
        let dir = scratch("allterms");
        fs::write(dir.join("holiday-photos-italy.jpg"), b"x").unwrap();
        fs::write(dir.join("holiday.txt"), b"x").unwrap();
        let idx = build(&dir);

        let hits = idx.search("holiday italy", None, 10);
        assert_eq!(hits[0].1.name, "holiday-photos-italy.jpg");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn results_can_be_narrowed_by_kind() {
        let dir = scratch("bykind");
        fs::write(dir.join("summer.mp3"), b"x").unwrap();
        fs::write(dir.join("summer.txt"), b"x").unwrap();
        let idx = build(&dir);

        let audio = idx.search("summer", Some("audio"), 10);
        assert_eq!(audio.len(), 1);
        assert_eq!(audio[0].1.kind, "audio");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_query_lists_the_most_recent_first() {
        let dir = scratch("empty");
        fs::write(dir.join("old.txt"), b"x").unwrap();
        fs::write(dir.join("new.txt"), b"x").unwrap();
        // Force a clear ordering rather than relying on filesystem timing.
        let mut idx = build(&dir);
        for d in idx.docs.iter_mut() {
            d.modified = if d.name == "new.txt" { 2_000 } else { 1_000 };
        }
        let hits = idx.search("", None, 10);
        assert_eq!(hits[0].1.name, "new.txt");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_match_returns_nothing_rather_than_everything() {
        let dir = scratch("nomatch");
        fs::write(dir.join("a.txt"), b"hello").unwrap();
        let idx = build(&dir);
        assert!(idx.search("zzzznotpresent", None, 10).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn binary_files_are_indexed_by_name_but_not_content() {
        let dir = scratch("binary");
        fs::write(dir.join("clip.mp4"), [0xffu8, 0xd8, 0x00, 0x01]).unwrap();
        let idx = build(&dir);
        assert_eq!(idx.docs.len(), 1);
        assert!(!idx.docs[0].text.contains('\u{0}'));
        assert!(idx.search("clip", None, 5).len() == 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn survives_a_save_and_load_round_trip() {
        let dir = scratch("persist");
        fs::write(dir.join("report.md"), b"quarterly figures").unwrap();
        let idx = build(&dir);
        let out = dir.join("index.json");
        idx.save_to(&out).unwrap();

        let back = Index::load_from(&out);
        assert_eq!(back.docs.len(), 1);
        assert_eq!(back.search("quarterly", None, 5).len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_index_loads_as_empty_rather_than_failing() {
        let idx = Index::load_from(Path::new("/nonexistent/index.json"));
        assert!(idx.docs.is_empty());
        assert!(idx.search("anything", None, 5).is_empty());
    }
}
