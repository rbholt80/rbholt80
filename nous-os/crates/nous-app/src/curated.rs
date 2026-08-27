//! What the machine thinks about the folder you are looking at.
//!
//! Not a chat box. The curator already knows which files are duplicates, which
//! video is filed with the photographs, which installer has sat unopened for a
//! year — and until now it could only say so in a sentence, to someone who
//! thought to ask. This puts what it found onto the files themselves, in the
//! colour of the action it would take, so opening a folder is the question.
//!
//! Read-only. A mark is an opinion about a file, not something done to it, and
//! it is approved the way any plan is: deliberately, as a whole.

use nous_core::json::Json;
use nous_ui::files::{Entry, Mark};
use nous_ui::theme::Risk;

/// How bad the curator thought it was, as the risk of dealing with it.
///
/// Its severity runs one to four and means "how much this is worth your
/// attention". Risk means "how much damage undoing this would take". They are
/// different questions, and the mapping is by what the *remedy* costs: noticing
/// a video is in the wrong folder is a move, and moving something back is easy;
/// reclaiming a duplicate is a deletion, and that wants the colour deletions
/// have everywhere else in the system.
pub fn risk_of(kind: &str, severity: u64) -> Risk {
    match kind {
        // The remedy is a deletion, whatever the severity.
        "duplicate" | "stale_downloads" => Risk::Elevated,
        // The remedy is a move.
        "misfiled_media" | "screenshots" | "loose_by_kind" | "arrived_together" => Risk::Write,
        // Something new the daemon knows about and this does not. Judge it by
        // how loudly it was said, and lean high: a finding drawn in the mildest
        // colour available is a finding nobody looks at twice.
        _ => match severity {
            0..=1 => Risk::Read,
            2 => Risk::Write,
            3 => Risk::Elevated,
            _ => Risk::Critical,
        },
    }
}

/// Put the curator's findings onto the entries they are about.
///
/// A finding names several files — both halves of a duplicate, every clip that
/// arrived together — so one finding can mark many tiles. Where two findings
/// land on the same file the worse one wins: a file that is both a duplicate
/// and merely loose is a duplicate.
pub fn apply(entries: &mut [Entry], report: &Json) -> usize {
    let mut marked = 0;
    for f in report.arr_or_empty("findings") {
        let kind = f.str_or("kind", "");
        let risk = risk_of(kind, f.f64_or("severity", 2.0) as u64);
        let note = {
            let d = f.str_or("detail", "");
            if d.is_empty() {
                f.str_or("title", "worth a look").to_string()
            } else {
                d.to_string()
            }
        };
        for p in f.arr_or_empty("paths") {
            let Some(path) = p.as_str() else { continue };
            let Some(e) = entries.iter_mut().find(|e| e.path == path) else {
                // A finding about a file in a subfolder, or one that has since
                // moved. Not an error: the scan covers a tree and this is one
                // folder of it.
                continue;
            };
            let better = match &e.mark {
                Some(m) => risk > m.risk,
                None => {
                    marked += 1;
                    true
                }
            };
            if better {
                e.mark = Some(Mark {
                    risk,
                    note: note.clone(),
                });
            }
        }
    }
    marked
}

/// The one line along the bottom: what is here and what could be done.
///
/// `None` when there is nothing to say, so a clean folder shows no bar rather
/// than a bar saying nothing.
pub fn summary(report: &Json, marked_here: usize) -> Option<String> {
    if marked_here == 0 {
        return None;
    }
    let reclaimable = report.str_or("reclaimable", "");
    let noun = if marked_here == 1 { "file" } else { "files" };
    // Only mention space when there is some: "and 0 B to reclaim" is worse
    // than not mentioning it.
    if reclaimable.is_empty() || report.f64_or("reclaimable_bytes", 0.0) < 1.0 {
        Some(format!("{marked_here} {noun} worth a look here"))
    } else {
        Some(format!(
            "{marked_here} {noun} worth a look here · {reclaimable} could be reclaimed"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nous_core::json::parse;

    fn entry(path: &str) -> Entry {
        Entry {
            name: path.rsplit('/').next().unwrap_or(path).to_string(),
            path: path.to_string(),
            is_dir: false,
            size: 0,
            thumb: None,
            mark: None,
        }
    }

    #[test]
    fn a_finding_marks_every_file_it_names() {
        // A duplicate is a claim about two files, and marking one of them
        // leaves the pair looking like one stray and one innocent.
        let mut e = vec![
            entry("/d/holiday.jpg"),
            entry("/d/holiday-copy.jpg"),
            entry("/d/notes.txt"),
        ];
        let r = parse(
            r#"{"findings":[{"kind":"duplicate","severity":4,"title":"2 copies",
                 "detail":"same picture twice",
                 "paths":["/d/holiday.jpg","/d/holiday-copy.jpg"]}],
                "reclaimable":"2.4 MB","reclaimable_bytes":2400000}"#,
        )
        .unwrap();
        assert_eq!(apply(&mut e, &r), 2);
        assert_eq!(e[0].mark.as_ref().unwrap().risk, Risk::Elevated);
        assert_eq!(e[1].mark.as_ref().unwrap().note, "same picture twice");
        assert!(e[2].mark.is_none(), "marked a file no finding named");
    }

    #[test]
    fn the_worse_of_two_findings_wins_the_tile() {
        // A file that is both a duplicate and merely loose is a duplicate, and
        // drawing it in the milder colour hides the thing worth knowing.
        let mut e = vec![entry("/d/a.jpg")];
        let r = parse(
            r#"{"findings":[
                 {"kind":"loose_by_kind","severity":1,"detail":"loose","paths":["/d/a.jpg"]},
                 {"kind":"duplicate","severity":4,"detail":"a copy of b.jpg","paths":["/d/a.jpg"]}]}"#,
        )
        .unwrap();
        apply(&mut e, &r);
        assert_eq!(e[0].mark.as_ref().unwrap().risk, Risk::Elevated);
        assert_eq!(e[0].mark.as_ref().unwrap().note, "a copy of b.jpg");
    }

    #[test]
    fn the_worse_one_wins_whichever_order_they_arrive_in() {
        let mut e = vec![entry("/d/a.jpg")];
        let r = parse(
            r#"{"findings":[
                 {"kind":"duplicate","severity":4,"detail":"a copy","paths":["/d/a.jpg"]},
                 {"kind":"loose_by_kind","severity":1,"detail":"loose","paths":["/d/a.jpg"]}]}"#,
        )
        .unwrap();
        apply(&mut e, &r);
        assert_eq!(
            e[0].mark.as_ref().unwrap().note,
            "a copy",
            "the milder finding overwrote it"
        );
    }

    #[test]
    fn a_finding_about_a_file_that_is_not_here_is_ignored() {
        // The scan covers a tree; this is one folder of it.
        let mut e = vec![entry("/d/a.jpg")];
        let r =
            parse(r#"{"findings":[{"kind":"duplicate","severity":4,"paths":["/d/sub/b.jpg"]}]}"#)
                .unwrap();
        assert_eq!(apply(&mut e, &r), 0);
        assert!(e[0].mark.is_none());
    }

    #[test]
    fn a_finding_with_no_detail_still_says_something() {
        let mut e = vec![entry("/d/a.jpg")];
        let r = parse(
            r#"{"findings":[{"kind":"screenshots","severity":2,"title":"14 screenshots","paths":["/d/a.jpg"]}]}"#,
        )
        .unwrap();
        apply(&mut e, &r);
        assert_eq!(e[0].mark.as_ref().unwrap().note, "14 screenshots");
    }

    #[test]
    fn a_remedy_that_deletes_is_coloured_like_a_deletion() {
        // The severity says how much it matters; the colour says what undoing
        // it would cost. Moving a file back is easy, restoring one is not.
        assert_eq!(risk_of("duplicate", 1), Risk::Elevated);
        assert_eq!(risk_of("stale_downloads", 1), Risk::Elevated);
        assert_eq!(risk_of("misfiled_media", 4), Risk::Write);
        assert_eq!(risk_of("loose_by_kind", 4), Risk::Write);
    }

    #[test]
    fn a_kind_this_does_not_know_is_judged_by_how_loudly_it_was_said() {
        // The daemon may learn new findings. An unknown one must not be
        // silently drawn in the mildest colour available.
        assert_eq!(risk_of("something_new", 4), Risk::Critical);
        assert_eq!(risk_of("something_new", 2), Risk::Write);
        assert_eq!(risk_of("something_new", 0), Risk::Read);
    }

    #[test]
    fn a_clean_folder_says_nothing_rather_than_saying_nothing_is_wrong() {
        let r = parse(r#"{"findings":[],"reclaimable_bytes":0}"#).unwrap();
        assert_eq!(summary(&r, 0), None);
    }

    #[test]
    fn the_summary_mentions_space_only_when_there_is_some() {
        let r = parse(r#"{"reclaimable":"0 B","reclaimable_bytes":0}"#).unwrap();
        let s = summary(&r, 3).unwrap();
        assert!(!s.contains("reclaim"), "offered to reclaim nothing: {s}");
        assert!(s.contains("3 files"));

        let r = parse(r#"{"reclaimable":"2.4 GB","reclaimable_bytes":2400000000}"#).unwrap();
        let s = summary(&r, 1).unwrap();
        assert!(s.contains("2.4 GB"), "{s}");
        assert!(s.contains("1 file") && !s.contains("1 files"), "{s}");
    }

    #[test]
    fn a_report_that_is_nonsense_marks_nothing_and_does_not_panic() {
        let mut e = vec![entry("/d/a.jpg")];
        for bad in [
            r#"{}"#,
            r#"{"findings":"no"}"#,
            r#"{"findings":[{"paths":[3]}]}"#,
        ] {
            let r = parse(bad).unwrap();
            assert_eq!(apply(&mut e, &r), 0, "{bad}");
        }
    }
}
