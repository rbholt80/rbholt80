//! What the panel was summoned *over*.
//!
//! The file manager passes a selection, the hotkey passes the window that was
//! in front. Without it, "tidy these" and "what is this about?" have no
//! referent and the shell has to guess.
//!
//! The focused window has to be captured before the panel appears, because a
//! moment later the focused window is the panel. The caller does that and
//! passes it in.

use nous_core::json::{json_obj, Json};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Context {
    /// Title of the window that was in front when the panel was summoned.
    pub focus: Option<String>,
    /// Files the file manager had selected.
    pub paths: Vec<String>,
    /// The folder being looked at.
    pub cwd: Option<String>,
}

impl Context {
    /// A few words naming what is attached, for the chip on the prompt line.
    ///
    /// Returns `None` when there is nothing worth showing. A focused window
    /// title alone is not: the panel is always summoned over *something*, so
    /// showing it every time would be noise rather than information.
    pub fn label(&self) -> Option<String> {
        let folder = self.cwd.as_deref().map(basename);
        match (self.paths.len(), folder) {
            (0, None) => None,
            (0, Some(dir)) => Some(dir.to_string()),
            (1, _) => Some(basename(&self.paths[0]).to_string()),
            (n, None) => Some(format!("{n} items")),
            (n, Some(dir)) => Some(format!("{n} items · {dir}")),
        }
    }

    pub fn to_json(&self) -> Json {
        json_obj([
            ("focus", self.focus.clone().map_or(Json::Null, Json::from)),
            (
                "paths",
                Json::Arr(self.paths.iter().cloned().map(Json::from).collect()),
            ),
            ("cwd", self.cwd.clone().map_or(Json::Null, Json::from)),
        ])
    }
}

/// The last component of a path, with any trailing slash ignored.
fn basename(p: &str) -> &str {
    let trimmed = p.trim_end_matches('/');
    if trimmed.is_empty() {
        return "/";
    }
    match trimmed.rsplit_once('/') {
        Some((_, name)) if !name.is_empty() => name,
        _ => trimmed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(paths: &[&str], cwd: Option<&str>) -> Context {
        Context {
            focus: None,
            paths: paths.iter().map(|s| s.to_string()).collect(),
            cwd: cwd.map(str::to_string),
        }
    }

    #[test]
    fn nothing_attached_shows_no_chip() {
        assert_eq!(Context::default().label(), None);
        // A window title alone is not worth a chip: the panel is always over
        // something, so showing it every time says nothing.
        let mut only_focus = Context::default();
        only_focus.focus = Some("Firefox".into());
        assert_eq!(only_focus.label(), None);
        // It is still sent, though: the resolver uses it even when the panel
        // does not show it.
        assert_eq!(
            only_focus.to_json().get("focus").and_then(|v| v.as_str()),
            Some("Firefox")
        );
    }

    #[test]
    fn one_file_is_named_and_several_are_counted() {
        assert_eq!(
            ctx(&["/home/joey/Downloads/report.pdf"], None)
                .label()
                .as_deref(),
            Some("report.pdf")
        );
        assert_eq!(
            ctx(
                &["/a/x.png", "/a/y.png", "/a/z.png"],
                Some("/home/joey/Downloads")
            )
            .label()
            .as_deref(),
            Some("3 items · Downloads")
        );
        assert_eq!(
            ctx(&["/a/x.png", "/a/y.png"], None).label().as_deref(),
            Some("2 items")
        );
    }

    #[test]
    fn a_folder_on_its_own_is_named_by_its_last_component() {
        assert_eq!(
            ctx(&[], Some("/home/joey/Pictures")).label().as_deref(),
            Some("Pictures")
        );
        // Trailing slashes are how a file manager often hands a folder over.
        assert_eq!(
            ctx(&[], Some("/home/joey/Pictures/")).label().as_deref(),
            Some("Pictures")
        );
        assert_eq!(ctx(&[], Some("/")).label().as_deref(), Some("/"));
    }

    #[test]
    fn a_bare_name_with_no_slashes_survives() {
        assert_eq!(basename("notes.txt"), "notes.txt");
        assert_eq!(basename(""), "/");
    }

    #[test]
    fn the_json_carries_every_field_the_daemon_reads() {
        let c = Context {
            focus: Some("Nemo".into()),
            paths: vec!["/a/x".into(), "/a/y".into()],
            cwd: Some("/a".into()),
        };
        let j = c.to_json();
        assert_eq!(j.get("focus").and_then(|v| v.as_str()), Some("Nemo"));
        assert_eq!(j.get("cwd").and_then(|v| v.as_str()), Some("/a"));
        assert_eq!(j.arr_or_empty("paths").len(), 2);
    }

    #[test]
    fn an_absent_field_is_null_rather_than_an_empty_string() {
        // The daemon filters empty strings out, but sending null says "not
        // known" where "" says "known to be nothing".
        let j = Context::default().to_json();
        assert!(j.get("focus").is_some_and(|v| v.is_null()));
        assert!(j.get("cwd").is_some_and(|v| v.is_null()));
        assert_eq!(j.arr_or_empty("paths").len(), 0);
    }
}
