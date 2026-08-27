//! What you can do to a file, and what the keys and the menu mean.
//!
//! The verbs are the ones every file manager has, because they are the ones
//! people already know: open, rename, copy, cut, paste, move to trash. What is
//! different here is underneath — none of them touch the disk. Each becomes a
//! capability the daemon runs through the broker, which checks it against
//! policy, records how to undo it, and writes it down. That is the trade this
//! whole system makes: the same gestures as anywhere else, and a machine that
//! can be told to put it back.

use nous_core::json::{json_obj, Json};
use std::path::{Path, PathBuf};

/// What a copy or cut is holding.
#[derive(Debug, Clone, PartialEq)]
pub struct Clipboard {
    pub paths: Vec<PathBuf>,
    /// A cut moves; a copy duplicates. The distinction only matters at paste.
    pub cut: bool,
}

/// One thing that can be done, as the menu lists it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Open,
    OpenWith,
    Rename,
    Copy,
    Cut,
    Paste,
    Trash,
    NewFolder,
    Properties,
    Refresh,
}

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Action::Open => "Open",
            Action::OpenWith => "Open With…",
            Action::Rename => "Rename",
            Action::Copy => "Copy",
            Action::Cut => "Cut",
            Action::Paste => "Paste",
            Action::Trash => "Move to Trash",
            Action::NewFolder => "New Folder",
            Action::Properties => "Properties",
            Action::Refresh => "Refresh",
        }
    }

    /// The shortcut to print beside it, so the menu teaches the keyboard.
    pub fn shortcut(self) -> &'static str {
        match self {
            Action::Open => "Enter",
            Action::Rename => "F2",
            Action::Copy => "Ctrl+C",
            Action::Cut => "Ctrl+X",
            Action::Paste => "Ctrl+V",
            Action::Trash => "Delete",
            Action::NewFolder => "Ctrl+Shift+N",
            Action::Refresh => "F5",
            Action::OpenWith | Action::Properties => "",
        }
    }

    /// Whether a separator line goes above this entry, grouping the menu the
    /// way every other file manager groups it.
    pub fn starts_group(self) -> bool {
        matches!(self, Action::Copy | Action::Trash | Action::NewFolder)
    }
}

/// What the menu should offer, given what is under the pointer.
///
/// An action that cannot work is left out rather than greyed: this menu is
/// short enough that absence reads as clearly as a dimmed row, and a dimmed row
/// still asks to be aimed at.
pub fn menu_for(on_file: bool, clipboard_has_something: bool) -> Vec<Action> {
    let mut v = Vec::new();
    if on_file {
        v.push(Action::Open);
        v.push(Action::OpenWith);
        v.push(Action::Rename);
        v.push(Action::Copy);
        v.push(Action::Cut);
    }
    if clipboard_has_something {
        v.push(Action::Paste);
    }
    if on_file {
        v.push(Action::Trash);
    }
    v.push(Action::NewFolder);
    v.push(Action::Refresh);
    if on_file {
        v.push(Action::Properties);
    }
    v
}

/// A capability call, ready to hand to the daemon.
#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    pub cap: &'static str,
    pub args: Json,
    /// What the journal should say happened, in words.
    pub why: String,
}

/// Open something with whatever the desktop already uses for it.
pub fn open(path: &Path) -> Job {
    Job {
        cap: "desk.open",
        args: json_obj([("path", path.to_string_lossy().to_string().into())]),
        why: format!("open {}", name_of(path)),
    }
}

/// Rename in place. The daemon refuses to write over an existing file, so this
/// cannot silently swallow a neighbour.
pub fn rename(path: &Path, to: &str) -> Result<Job, String> {
    let clean = to.trim();
    if clean.is_empty() {
        return Err("a file needs a name".to_string());
    }
    // A name is a name, not a path: letting one through would move the file
    // somewhere else while claiming to rename it.
    if clean.contains('/') {
        return Err("a name cannot contain '/'".to_string());
    }
    if clean == "." || clean == ".." {
        return Err("that is not a name".to_string());
    }
    if clean == name_of(path) {
        return Err("that is already its name".to_string());
    }
    let dest = path.parent().unwrap_or_else(|| Path::new("/")).join(clean);
    Ok(Job {
        cap: "fs.move",
        args: json_obj([
            ("from", path.to_string_lossy().to_string().into()),
            ("to", dest.to_string_lossy().to_string().into()),
        ]),
        why: format!("rename {} to {}", name_of(path), clean),
    })
}

/// Put something in the trash, from which the daemon can take it back out.
pub fn trash(path: &Path) -> Job {
    Job {
        cap: "fs.delete",
        args: json_obj([("path", path.to_string_lossy().to_string().into())]),
        why: format!("move {} to the trash", name_of(path)),
    }
}

pub fn new_folder(inside: &Path, name: &str) -> Result<Job, String> {
    let clean = name.trim();
    if clean.is_empty() {
        return Err("a folder needs a name".to_string());
    }
    if clean.contains('/') {
        return Err("a name cannot contain '/'".to_string());
    }
    let dest = inside.join(clean);
    Ok(Job {
        cap: "fs.mkdir",
        args: json_obj([("path", dest.to_string_lossy().to_string().into())]),
        why: format!("make a folder called {}", clean),
    })
}

/// Paste one clipboard entry into a folder.
///
/// A cut is a move. A copy has no `fs.copy` behind it, so it is refused rather
/// than quietly turned into a move — losing the original is not a rounding
/// error on "copy".
pub fn paste(item: &Path, into: &Path, cut: bool) -> Result<Job, String> {
    if !cut {
        return Err("copying is not wired up yet — cut and paste moves instead".to_string());
    }
    if item.parent() == Some(into) {
        return Err("it is already there".to_string());
    }
    // Moving a folder inside itself destroys it. The daemon would refuse, but
    // the answer belongs where the gesture is, not three layers down.
    if into.starts_with(item) {
        return Err("a folder cannot be moved inside itself".to_string());
    }
    let dest = into.join(name_of(item));
    Ok(Job {
        cap: "fs.move",
        args: json_obj([
            ("from", item.to_string_lossy().to_string().into()),
            ("to", dest.to_string_lossy().to_string().into()),
        ]),
        why: format!("move {} into {}", name_of(item), name_of(into)),
    })
}

pub fn name_of(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| p.to_string_lossy().to_string())
}

/// Which entry typing `typed` should jump to.
///
/// Prefix first, because that is what typing a name means; then anywhere in the
/// name, so "report" still finds "2026-report.pdf". Case is ignored, since
/// nobody holds shift to find a file.
pub fn type_ahead(names: &[String], typed: &str) -> Option<usize> {
    if typed.is_empty() {
        return None;
    }
    let want = typed.to_lowercase();
    let lower: Vec<String> = names.iter().map(|n| n.to_lowercase()).collect();
    lower
        .iter()
        .position(|n| n.starts_with(&want))
        .or_else(|| lower.iter().position(|n| n.contains(&want)))
}

/// A size the way a person says it.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut n = bytes as f64;
    let mut u = 0;
    while n >= 1024.0 && u < UNITS.len() - 1 {
        n /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{} B", bytes)
    } else if n < 10.0 {
        format!("{:.1} {}", n, UNITS[u])
    } else {
        format!("{:.0} {}", n, UNITS[u])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rename_stays_in_the_folder_it_started_in() {
        let j = rename(Path::new("/home/j/Downloads/a.txt"), "b.txt").unwrap();
        assert_eq!(j.cap, "fs.move");
        assert_eq!(j.args.str_or("to", ""), "/home/j/Downloads/b.txt");
    }

    #[test]
    fn a_name_with_a_slash_in_it_is_refused_rather_than_moving_the_file() {
        // "rename this to ../../b" would be a move dressed as a rename, and the
        // journal entry would say the wrong thing about it.
        let e = rename(Path::new("/home/j/a.txt"), "../../b.txt").unwrap_err();
        assert!(e.contains('/'), "{e}");
        assert!(rename(Path::new("/home/j/a.txt"), "").is_err());
        assert!(
            rename(Path::new("/home/j/a.txt"), "  ").is_err(),
            "whitespace is not a name"
        );
        assert!(rename(Path::new("/home/j/a.txt"), "..").is_err());
    }

    #[test]
    fn renaming_something_to_what_it_is_called_is_not_a_change() {
        let e = rename(Path::new("/home/j/a.txt"), "a.txt").unwrap_err();
        assert!(e.contains("already"), "{e}");
        // Trailing space is the same name, and would otherwise make a second
        // file whose name looks identical.
        assert!(rename(Path::new("/home/j/a.txt"), "a.txt ").is_err());
    }

    #[test]
    fn a_folder_cannot_be_pasted_into_itself() {
        // The move would take the folder with it and leave nothing behind.
        let e = paste(
            Path::new("/home/j/Music"),
            Path::new("/home/j/Music/live"),
            true,
        )
        .unwrap_err();
        assert!(e.contains("itself"), "{e}");
    }

    #[test]
    fn pasting_where_it_already_is_says_so_instead_of_failing_obscurely() {
        let e = paste(Path::new("/home/j/a.txt"), Path::new("/home/j"), true).unwrap_err();
        assert!(e.contains("already"), "{e}");
    }

    #[test]
    fn a_cut_and_paste_moves_and_keeps_the_name() {
        let j = paste(Path::new("/home/j/a.txt"), Path::new("/home/j/Docs"), true).unwrap();
        assert_eq!(j.args.str_or("from", ""), "/home/j/a.txt");
        assert_eq!(j.args.str_or("to", ""), "/home/j/Docs/a.txt");
    }

    #[test]
    fn a_copy_is_refused_rather_than_quietly_becoming_a_move() {
        // There is no fs.copy behind this. Doing a move instead would delete
        // the original, which is not a small difference from copying it.
        let e = paste(Path::new("/home/j/a.txt"), Path::new("/home/j/Docs"), false).unwrap_err();
        assert!(e.contains("copying"), "{e}");
    }

    #[test]
    fn the_menu_offers_only_what_can_be_done() {
        let on_nothing = menu_for(false, false);
        assert!(
            !on_nothing.contains(&Action::Rename),
            "offered to rename nothing"
        );
        assert!(
            !on_nothing.contains(&Action::Paste),
            "offered an empty clipboard"
        );
        assert!(
            on_nothing.contains(&Action::NewFolder),
            "no way to make a folder"
        );

        let on_file = menu_for(true, true);
        assert!(on_file.contains(&Action::Open));
        assert!(on_file.contains(&Action::Trash));
        assert!(on_file.contains(&Action::Paste));
    }

    #[test]
    fn typing_a_name_finds_it_by_its_beginning_first() {
        let names: Vec<String> = ["2026-report.pdf", "report.txt", "Receipts"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        // "report.txt" starts with it; "2026-report.pdf" merely contains it.
        assert_eq!(type_ahead(&names, "rep"), Some(1));
        assert_eq!(type_ahead(&names, "REC"), Some(2), "case should not matter");
        // Nothing starts with "2026-r" except the first, which also contains it.
        assert_eq!(type_ahead(&names, "2026"), Some(0));
        assert_eq!(type_ahead(&names, "zzz"), None);
        assert_eq!(
            type_ahead(&names, ""),
            None,
            "typing nothing jumped somewhere"
        );
    }

    #[test]
    fn sizes_read_the_way_people_say_them() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(20 * 1024 * 1024), "20 MB");
        assert_eq!(human_size(1024_u64.pow(4)), "1.0 TB");
    }

    #[test]
    fn every_action_the_menu_can_show_has_a_label() {
        for a in menu_for(true, true) {
            assert!(!a.label().is_empty(), "{a:?} has no label");
        }
    }
}
