//! One line that finds anything, and does anything.
//!
//! Every desktop makes you decide first: are you *finding* something or
//! *doing* something? A search box for one, a menu or a terminal for the
//! other, and a folder tree for when you half-remember where you left it. That
//! split is not a fact about computers. It exists because a machine that
//! cannot understand a sentence has to be told which of its two mouths you are
//! speaking into.
//!
//! This does not ask. You type; whatever matches appears; whatever you typed
//! is also always available as something to ask for. "budget" finds the
//! spreadsheet. "sort these by year" matches nothing and is a request. Neither
//! needed a mode, and no keystroke was spent saying which one you meant.
//!
//! The results come from wherever they are — files from the index, actions
//! from the ledger, places, views — and are ranked together. A desktop keeps
//! those in different applications; there was never a reason beyond the fact
//! that different people wrote them.

use crate::history::Deed;
use crate::places::Place;
use nous_core::json::Json;
use std::path::PathBuf;

/// What kind of thing a result is, which decides its icon and what opening it
/// means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    Folder,
    File,
    /// Somewhere in the sidebar.
    Place,
    /// One of the window's views.
    View,
    /// Something that was done, which can be looked at or taken back.
    Deed,
}

impl Sort {
    /// The word shown beside a result, so a list of mixed things still reads.
    pub fn label(self) -> &'static str {
        match self {
            Sort::Folder => "folder",
            Sort::File => "file",
            Sort::Place => "place",
            Sort::View => "view",
            Sort::Deed => "done",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub sort: Sort,
    pub title: String,
    /// Where it is, or what it was — the quiet second line.
    pub detail: String,
    pub path: Option<PathBuf>,
    /// For a deed: which entry, so it can be taken back from here.
    pub seq: Option<u64>,
    pub score: f64,
}

/// How much each kind is worth relative to the others.
///
/// Not a preference: a correction. The file index scores against a corpus of
/// thousands and a view list scores against four, so their raw numbers are not
/// comparable. These bring them onto one scale, and the ordering encodes what
/// someone typing three letters usually wants — a place or a view they are
/// naming exactly, before a file that merely contains the word.
fn weight(sort: Sort) -> f64 {
    match sort {
        Sort::View => 1.35,
        Sort::Place => 1.3,
        Sort::Folder => 1.1,
        Sort::File => 1.0,
        // Lowest not because it matters least, but because the ledger is small
        // and its text repetitive, so its matches are cheap.
        Sort::Deed => 0.85,
    }
}

/// How well `text` answers `query`, or `None` if it does not.
///
/// Deliberately simple and local, for the things there are only a handful of.
/// Files come pre-scored by the daemon's index, which does the real work
/// against every file on the machine.
pub fn score_text(text: &str, query: &str) -> Option<f64> {
    let t = text.to_lowercase();
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return None;
    }
    if t == q {
        return Some(1.0);
    }
    if t.starts_with(&q) {
        // A prefix match is what someone typing a name is doing, and it beats
        // finding the word buried in something longer.
        return Some(0.85 - 0.1 * (t.len() as f64 / (t.len() + 24) as f64));
    }
    if t.contains(&q) {
        return Some(0.55);
    }
    // Every word of the query somewhere in the text, in any order: "budget
    // 2026" should find "2026-budget.xlsx".
    let words: Vec<&str> = q.split_whitespace().filter(|w| !w.is_empty()).collect();
    if words.len() > 1 && words.iter().all(|w| t.contains(w)) {
        return Some(0.45);
    }
    None
}

/// Gather everything that matches, best first.
///
/// `files` is the daemon's reply to `fs.search`, or `Json::Null` when there is
/// no daemon — in which case this still returns places, views and whatever the
/// ledger holds, because none of those needed one.
pub fn gather(
    query: &str,
    files: &Json,
    places: &[Place],
    deeds: &[Deed],
    views: &[(&'static str, &'static str)],
    limit: usize,
) -> Vec<Hit> {
    let q = query.trim();
    if q.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<Hit> = Vec::new();

    for (name, detail) in views {
        if let Some(s) = score_text(name, q) {
            hits.push(Hit {
                sort: Sort::View,
                title: (*name).to_string(),
                detail: (*detail).to_string(),
                path: None,
                seq: None,
                score: s * weight(Sort::View),
            });
        }
    }

    for p in places {
        if let Some(s) = score_text(&p.name, q) {
            hits.push(Hit {
                sort: Sort::Place,
                title: p.name.clone(),
                detail: p.path.to_string_lossy().to_string(),
                path: Some(p.path.clone()),
                seq: None,
                score: s * weight(Sort::Place),
            });
        }
    }

    for d in deeds {
        let text = format!("{} {}", d.headline(), d.detail);
        if let Some(s) = score_text(&text, q) {
            hits.push(Hit {
                sort: Sort::Deed,
                title: d.headline().to_string(),
                detail: if d.can_undo() {
                    format!("{} · can be taken back", d.capability)
                } else {
                    d.capability.clone()
                },
                path: None,
                seq: Some(d.seq),
                score: s * weight(Sort::Deed),
            });
        }
    }

    // Files arrive already ranked against every file on the machine. Their
    // scores are normalised against the best of them so a corpus that happens
    // to score high cannot swamp everything else.
    let results = files.arr_or_empty("results");
    let best = results
        .iter()
        .map(|r| r.f64_or("score", 0.0))
        .fold(0.0_f64, f64::max)
        .max(1e-6);
    for r in results.iter().take(limit) {
        let path = PathBuf::from(r.str_or("path", ""));
        let kind = r.str_or("kind", "file");
        let sort = if kind == "dir" || kind == "folder" {
            Sort::Folder
        } else {
            Sort::File
        };
        let name = r.str_or("name", "").to_string();
        // The daemon's ranking, plus a nudge for an exact name match: it scores
        // a whole document, and someone typing a filename means the filename.
        let exact = score_text(&name, q).unwrap_or(0.0);
        let s = (r.f64_or("score", 0.0) / best).clamp(0.0, 1.0) * 0.8 + exact * 0.2;
        hits.push(Hit {
            sort,
            title: name,
            detail: shorten(&path),
            path: Some(path),
            seq: None,
            score: s * weight(sort),
        });
    }

    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            // A stable tie-break, so a list does not reshuffle between two
            // keystrokes that changed nothing about the ranking.
            .then_with(|| a.title.cmp(&b.title))
    });
    hits.dedup_by(|a, b| a.sort == b.sort && a.title == b.title && a.path == b.path);
    hits.truncate(limit);
    hits
}

/// A path as a person would say it: under the home directory, from home.
fn shorten(p: &std::path::Path) -> String {
    let s = p.to_string_lossy().to_string();
    if let Ok(home) = std::env::var("HOME") {
        if let Some(rest) = s.strip_prefix(&home) {
            return format!("~{rest}");
        }
    }
    s
}

/// Whether what was typed reads as a request rather than a name.
///
/// Used only to decide what to put first — the other reading is always still
/// there, one arrow key away. Guessing wrong therefore costs a keystroke, not
/// an outcome, which is the only reason a guess is allowed here at all.
pub fn looks_like_a_request(query: &str) -> bool {
    let q = query.trim().to_lowercase();
    if q.split_whitespace().count() >= 4 {
        return true;
    }
    // A verb at the front is someone telling the machine to do something.
    const VERBS: [&str; 18] = [
        "sort", "move", "delete", "remove", "rename", "make", "create", "tidy", "clean", "find",
        "play", "open", "show", "put", "copy", "free", "organise", "organize",
    ];
    q.split_whitespace()
        .next()
        .is_some_and(|w| VERBS.contains(&w))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nous_core::json::parse;

    fn place(name: &str, path: &str) -> Place {
        Place {
            name: name.to_string(),
            path: PathBuf::from(path),
        }
    }

    const VIEWS: [(&str, &str); 4] = [
        ("Files", "your folders"),
        ("Player", "music and video"),
        ("Edit", "the cutting room"),
        ("History", "what has been done"),
    ];

    fn files(json: &str) -> Json {
        parse(json).unwrap()
    }

    #[test]
    fn one_list_holds_things_a_desktop_would_keep_in_different_applications() {
        let f = files(
            r#"{"results":[{"path":"/home/j/Music/setlist.txt","name":"setlist.txt","kind":"file","score":2.0}]}"#,
        );
        let hits = gather(
            "music",
            &f,
            &[place("Music", "/home/j/Music")],
            &[],
            &VIEWS,
            10,
        );
        let sorts: Vec<Sort> = hits.iter().map(|h| h.sort).collect();
        assert!(
            sorts.contains(&Sort::Place),
            "the folder shortcut is missing"
        );
        assert!(sorts.contains(&Sort::File), "the file is missing");
        // The place is named exactly; the file merely lives there.
        assert_eq!(hits[0].sort, Sort::Place, "{hits:#?}");
    }

    #[test]
    fn a_name_typed_exactly_comes_before_something_that_merely_contains_it() {
        let f = files(
            r#"{"results":[
                {"path":"/a/notes-about-budget-planning.md","name":"notes-about-budget-planning.md","kind":"file","score":9.0},
                {"path":"/a/budget.xlsx","name":"budget.xlsx","kind":"file","score":7.0}]}"#,
        );
        let hits = gather("budget.xlsx", &f, &[], &[], &VIEWS, 10);
        assert_eq!(
            hits[0].title, "budget.xlsx",
            "the index's own ranking buried an exact name: {hits:#?}"
        );
    }

    #[test]
    fn every_word_anywhere_finds_it_whatever_order_they_were_typed_in() {
        assert!(score_text("2026-budget.xlsx", "budget 2026").is_some());
        assert!(score_text("2026-budget.xlsx", "budget zebra").is_none());
        // A single word that is not there is still not there.
        assert!(score_text("holiday.jpg", "invoice").is_none());
    }

    #[test]
    fn a_prefix_beats_a_word_buried_in_the_middle() {
        let a = score_text("Documents", "doc").unwrap();
        let b = score_text("my-old-documents-backup", "doc").unwrap();
        assert!(a > b, "{a} vs {b}");
    }

    #[test]
    fn a_file_index_that_scores_high_cannot_swamp_everything_else() {
        // The index scores against every file on the machine; the view list
        // scores against four. Raw numbers from the two are not comparable.
        let f = files(
            r#"{"results":[{"path":"/a/history-of-rome.txt","name":"history-of-rome.txt","kind":"file","score":9999.0}]}"#,
        );
        let hits = gather("history", &f, &[], &[], &VIEWS, 10);
        assert_eq!(
            hits[0].sort,
            Sort::View,
            "a single file outranked the view it was named after: {hits:#?}"
        );
    }

    #[test]
    fn with_no_daemon_the_things_that_never_needed_one_are_still_found() {
        let hits = gather(
            "player",
            &Json::Null,
            &[place("Pictures", "/home/j/Pictures")],
            &[],
            &VIEWS,
            10,
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].sort, Sort::View);
    }

    #[test]
    fn something_that_was_done_can_be_found_again() {
        // Which is the point of writing it down. A desktop keeps this in a log
        // file nobody opens.
        let deeds = crate::history::read(
            &parse(
                r#"{"records":[{"seq":7,"ts":1,"intent":"tidy my downloads",
                     "detail":"moved 84 images into Pictures/2026","capability":"fs.move",
                     "risk":"write","outcome":"ok","undo":{"kind":"move_path"}}]}"#,
            )
            .unwrap(),
        );
        let hits = gather("tidy", &Json::Null, &[], &deeds, &VIEWS, 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].sort, Sort::Deed);
        assert_eq!(hits[0].seq, Some(7));
        assert!(
            hits[0].detail.contains("taken back"),
            "did not say it could be undone: {}",
            hits[0].detail
        );
    }

    #[test]
    fn nothing_typed_finds_nothing_rather_than_everything() {
        for q in ["", "   "] {
            assert!(gather(q, &Json::Null, &[place("Home", "/h")], &[], &VIEWS, 10).is_empty());
        }
    }

    #[test]
    fn the_same_thing_twice_appears_once() {
        let f = files(
            r#"{"results":[
                {"path":"/a/b.txt","name":"b.txt","kind":"file","score":2.0},
                {"path":"/a/b.txt","name":"b.txt","kind":"file","score":2.0}]}"#,
        );
        let hits = gather("b.txt", &f, &[], &[], &VIEWS, 10);
        assert_eq!(hits.len(), 1, "{hits:#?}");
    }

    #[test]
    fn the_order_does_not_reshuffle_between_keystrokes_that_changed_nothing() {
        // Two results scoring identically must not swap places, or the list
        // moves under the finger about to click it.
        let f = files(
            r#"{"results":[
                {"path":"/a/zeta.txt","name":"zeta.txt","kind":"file","score":5.0},
                {"path":"/a/alpha.txt","name":"alpha.txt","kind":"file","score":5.0}]}"#,
        );
        let a = gather("txt", &f, &[], &[], &VIEWS, 10);
        let b = gather("txt", &f, &[], &[], &VIEWS, 10);
        assert_eq!(
            a.iter().map(|h| h.title.clone()).collect::<Vec<_>>(),
            b.iter().map(|h| h.title.clone()).collect::<Vec<_>>()
        );
        assert_eq!(a[0].title, "alpha.txt", "the tie-break is not stable");
    }

    #[test]
    fn a_sentence_reads_as_a_request_and_a_name_does_not() {
        assert!(looks_like_a_request("sort these by year"));
        assert!(looks_like_a_request("get rid of anything I already have"));
        assert!(looks_like_a_request("tidy"));
        assert!(!looks_like_a_request("budget.xlsx"));
        assert!(!looks_like_a_request("Downloads"));
        assert!(!looks_like_a_request("holiday photos"));
    }

    #[test]
    fn a_path_is_shown_the_way_people_say_it() {
        std::env::set_var("HOME", "/home/joey");
        assert_eq!(
            shorten(std::path::Path::new("/home/joey/Music/a.flac")),
            "~/Music/a.flac"
        );
        assert_eq!(
            shorten(std::path::Path::new("/var/log/syslog")),
            "/var/log/syslog"
        );
    }

    #[test]
    fn the_list_is_bounded_however_much_matches() {
        let many: Vec<String> = (0..200)
            .map(|i| {
                format!(r#"{{"path":"/a/f{i}.txt","name":"f{i}.txt","kind":"file","score":1.0}}"#)
            })
            .collect();
        let f = files(&format!(r#"{{"results":[{}]}}"#, many.join(",")));
        let hits = gather("txt", &f, &[], &[], &VIEWS, 12);
        assert_eq!(hits.len(), 12);
    }
}
