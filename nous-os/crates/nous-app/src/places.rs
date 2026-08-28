//! The places bar, and where you have been.
//!
//! Two things every file manager has that a grid of tiles does not: a list of
//! the folders you actually use, and a memory of where you just were. Without
//! the first, reaching Pictures means walking up to home and back down. Without
//! the second, one wrong double-click costs you your place.

use std::path::{Path, PathBuf};

/// One entry in the sidebar.
#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    pub name: String,
    pub path: PathBuf,
}

/// The folders worth a shortcut, in the order every desktop lists them.
///
/// Only the ones that exist: an entry for a Music folder on a machine with no
/// Music folder is a row that does nothing, and a sidebar of those teaches you
/// to stop trusting the sidebar.
pub fn places(home: &Path) -> Vec<Place> {
    let mut out = vec![Place {
        name: "Home".to_string(),
        path: home.to_path_buf(),
    }];
    for name in [
        "Desktop",
        "Documents",
        "Downloads",
        "Music",
        "Pictures",
        "Videos",
    ] {
        let p = home.join(name);
        if p.is_dir() {
            out.push(Place {
                name: name.to_string(),
                path: p,
            });
        }
    }
    out
}

/// Where you have been, and how to get back.
///
/// A stack with a cursor rather than two stacks: going somewhere new after
/// stepping back has to discard the forward entries, and two stacks make it
/// possible to forget.
#[derive(Debug, Clone)]
pub struct History {
    seen: Vec<PathBuf>,
    at: usize,
}

impl History {
    pub fn new(start: PathBuf) -> History {
        History {
            seen: vec![start],
            at: 0,
        }
    }

    pub fn here(&self) -> &Path {
        &self.seen[self.at]
    }

    /// Go somewhere new. Anything ahead of here is forgotten, because it is no
    /// longer where "forward" leads.
    pub fn go(&mut self, to: PathBuf) {
        if self.here() == to {
            return;
        }
        self.seen.truncate(self.at + 1);
        self.seen.push(to);
        self.at = self.seen.len() - 1;
        // A history that grows without bound over a long session is a slow
        // leak; the oldest entries are the ones nobody goes back to.
        const KEEP: usize = 128;
        if self.seen.len() > KEEP {
            let drop = self.seen.len() - KEEP;
            self.seen.drain(0..drop);
            self.at -= drop;
        }
    }

    pub fn can_go_back(&self) -> bool {
        self.at > 0
    }

    pub fn can_go_forward(&self) -> bool {
        self.at + 1 < self.seen.len()
    }

    pub fn back(&mut self) -> Option<PathBuf> {
        if !self.can_go_back() {
            return None;
        }
        self.at -= 1;
        Some(self.seen[self.at].clone())
    }

    pub fn forward(&mut self) -> Option<PathBuf> {
        if !self.can_go_forward() {
            return None;
        }
        self.at += 1;
        Some(self.seen[self.at].clone())
    }
}

/// The path as clickable pieces, each with the folder it leads to.
///
/// Under the home directory the leading components are replaced by "Home":
/// `/home/joey/Music/live` reads as Home › Music › live, which is how a person
/// says where they are.
pub fn crumbs(path: &Path, home: &Path) -> Vec<Place> {
    let mut out: Vec<Place> = Vec::new();
    if let Ok(rest) = path.strip_prefix(home) {
        out.push(Place {
            name: "Home".to_string(),
            path: home.to_path_buf(),
        });
        let mut at = home.to_path_buf();
        for part in rest.components() {
            at = at.join(part);
            out.push(Place {
                name: part.as_os_str().to_string_lossy().to_string(),
                path: at.clone(),
            });
        }
        return out;
    }
    let mut at = PathBuf::from("/");
    out.push(Place {
        name: "/".to_string(),
        path: at.clone(),
    });
    for part in path.components().skip(1) {
        at = at.join(part);
        out.push(Place {
            name: part.as_os_str().to_string_lossy().to_string(),
            path: at.clone(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn going_somewhere_new_forgets_what_was_ahead() {
        // Otherwise "forward" leads to a folder you deliberately turned away
        // from, which is worse than no forward button at all.
        let mut h = History::new(PathBuf::from("/a"));
        h.go(PathBuf::from("/b"));
        h.go(PathBuf::from("/c"));
        assert_eq!(h.back(), Some(PathBuf::from("/b")));
        assert!(h.can_go_forward());
        h.go(PathBuf::from("/d"));
        assert!(!h.can_go_forward(), "still offering the way back to /c");
        assert_eq!(h.here(), Path::new("/d"));
    }

    #[test]
    fn the_ends_of_the_history_are_the_ends() {
        let mut h = History::new(PathBuf::from("/a"));
        assert!(!h.can_go_back() && !h.can_go_forward());
        assert_eq!(h.back(), None);
        assert_eq!(h.forward(), None);
        assert_eq!(h.here(), Path::new("/a"));
    }

    #[test]
    fn going_where_you_already_are_is_not_a_move() {
        // Refreshing, or clicking the folder you are in, should not fill the
        // history with the same entry until Back stops working.
        let mut h = History::new(PathBuf::from("/a"));
        h.go(PathBuf::from("/a"));
        h.go(PathBuf::from("/a"));
        assert!(!h.can_go_back());
    }

    #[test]
    fn a_very_long_session_does_not_grow_without_bound() {
        let mut h = History::new(PathBuf::from("/start"));
        for i in 0..500 {
            h.go(PathBuf::from(format!("/f{i}")));
        }
        assert!(h.seen.len() <= 128, "history holds {}", h.seen.len());
        // And the cursor still points at where we actually are.
        assert_eq!(h.here(), Path::new("/f499"));
        assert!(h.can_go_back());
        h.back();
        assert_eq!(h.here(), Path::new("/f498"));
    }

    #[test]
    fn a_path_under_home_reads_as_home_rather_than_as_slash_home_slash_you() {
        let home = Path::new("/home/joey");
        let c = crumbs(Path::new("/home/joey/Music/live"), home);
        let names: Vec<&str> = c.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["Home", "Music", "live"]);
        assert_eq!(c[1].path, Path::new("/home/joey/Music"));
        assert_eq!(c.last().unwrap().path, Path::new("/home/joey/Music/live"));
    }

    #[test]
    fn a_path_outside_home_is_shown_from_the_root() {
        let c = crumbs(Path::new("/var/log"), Path::new("/home/joey"));
        let names: Vec<&str> = c.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["/", "var", "log"]);
        assert_eq!(c[1].path, Path::new("/var"));
    }

    #[test]
    fn home_itself_is_one_crumb() {
        let home = Path::new("/home/joey");
        let c = crumbs(home, home);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].name, "Home");
    }

    #[test]
    fn only_folders_that_exist_get_a_shortcut() {
        let dir = std::env::temp_dir().join(format!("nous-places-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Music")).unwrap();
        std::fs::create_dir_all(dir.join("Documents")).unwrap();
        // A file, not a folder: it must not become a place you can open.
        std::fs::write(dir.join("Videos"), b"not a folder").unwrap();

        let p = places(&dir);
        let names: Vec<&str> = p.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, vec!["Home", "Documents", "Music"]);
        assert!(
            !names.contains(&"Videos"),
            "a file became a folder shortcut"
        );
        assert!(!names.contains(&"Pictures"), "a shortcut to nowhere");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
