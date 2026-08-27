//! What the window holds, and how a key or a click reaches it.
//!
//! Three views in one window, switched rather than stacked. Each already knew
//! how to draw itself and what a gesture on it means; none of them had anywhere
//! to be drawn. This is that place.
//!
//! Where a view can read the truth off the disk it does — the file view lists a
//! real directory, the cutting room opens a real project. Where the truth lives
//! in a running daemon and there is no running daemon, the view says so rather
//! than showing something made up: a player with nothing playing is what a
//! player with nothing playing looks like.

use crate::filepane::FilePane;
use crate::link::Link;
use nous_core::json::Json;
use nous_ui::draw::{Canvas, Rect};
use nous_ui::ffi;
use nous_ui::player::Player;
use nous_ui::queue::Queue;
use nous_ui::theme::{Metrics, Theme};
use nous_ui::window::Key;
use std::path::PathBuf;

/// The bar across the top naming the views.
const TABS_H: f64 = 44.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Files,
    Player,
    Edit,
}

impl View {
    pub const ALL: [View; 3] = [View::Files, View::Player, View::Edit];

    pub fn title(self) -> &'static str {
        match self {
            View::Files => "Files",
            View::Player => "Player",
            View::Edit => "Edit",
        }
    }

    pub fn named(s: &str) -> Option<View> {
        match s.to_ascii_lowercase().as_str() {
            "files" | "file" => Some(View::Files),
            "player" | "play" | "music" | "video" => Some(View::Player),
            "edit" | "editor" | "cut" => Some(View::Edit),
            _ => None,
        }
    }

    fn next(self) -> View {
        match self {
            View::Files => View::Player,
            View::Player => View::Edit,
            View::Edit => View::Files,
        }
    }
}

pub struct App {
    pub view: View,
    pub pane: FilePane,
    pub queue: Queue,
    pub editor: Player,
    /// The line to the daemon. Held open across views, because what is playing
    /// and what is in a folder are asked of the same process.
    pub link: Link,
    /// The tab rectangles from the last frame, so a click can be tested against
    /// what was actually drawn rather than against a guess at where it was.
    tabs: Vec<(View, Rect)>,
}

impl App {
    pub fn new(view: View) -> App {
        let home = home();
        // Downloads if there is one, home if not: a first run that opens on a
        // folder that does not exist shows an empty grid, which reads as a
        // broken program rather than as an empty folder.
        let start = {
            let d = home.join("Downloads");
            if d.is_dir() {
                d
            } else {
                home.clone()
            }
        };
        App {
            view,
            pane: FilePane::new(start, home),
            queue: Queue::default(),
            editor: Player::new("untitled", Vec::new()),
            link: Link::new(),
            tabs: Vec::new(),
        }
    }

    /// Read what can be read without a daemon.
    pub fn load(&mut self) {
        if let Some((name, doc)) = newest_project() {
            self.editor = Player::from_project(&doc, |_| None);
            self.editor.project = name;
        }
        // What is playing, if anything is. Absent a daemon this answers
        // nothing, which is the truth and what the player already draws.
        self.refresh_playback();
    }

    /// Open the right-button menu where a pointer would have opened it.
    ///
    /// For looking at the menu without a pointer, which is the only way to look
    /// at it when the window is being driven by a screenshot rather than a
    /// hand. Not reachable from the keyboard or the mouse.
    pub fn demo_menu(&mut self, w: f64, h: f64) {
        if self.view != View::Files {
            return;
        }
        let body = at_origin(self.body(w, h));
        let grid = self.pane.grid_rect(body);
        self.pane
            .click(grid.x + 120.0, grid.y + 90.0, 3, body, &mut self.link);
    }

    pub fn refresh_playback(&mut self) {
        if let Some(report) = self.link.ask("media.state", Json::obj()) {
            let inner = report
                .get("steps")
                .and_then(|s| s.as_arr())
                .and_then(|a| a.first())
                .and_then(|s| s.get("result"))
                .cloned()
                .unwrap_or(report);
            self.queue.apply(&inner, |_| None);
        }
    }

    /// Escape closes the window unless a view is using it for something — a
    /// menu to dismiss, or a rename to abandon.
    pub fn handles_escape(&self) -> bool {
        self.view == View::Files && self.pane.wants_escape()
    }

    // --- input ------------------------------------------------------------

    /// Whether the views are typing rather than commanding, in which case the
    /// keys that switch view are letters and belong to whatever is being typed.
    fn is_typing(&self) -> bool {
        self.view == View::Files && self.pane.wants_escape()
    }

    pub fn key(&mut self, k: Key, w: f64, h: f64) {
        let body = self.body(w, h);
        if self.is_typing() {
            return self.pane.key(k, body, &mut self.link);
        }
        // View switching, wherever you are. Ctrl-held, so a bare "1" can still
        // be typed at a file name.
        if !k.ctrl && !k.alt {
            match k.sym {
                s if s == '1' as u64 => return self.view = View::Files,
                s if s == '2' as u64 => return self.view = View::Player,
                s if s == '3' as u64 => return self.view = View::Edit,
                _ => {}
            }
        }
        if k.is(ffi::XK_Tab) {
            self.view = self.view.next();
            return;
        }
        match self.view {
            View::Files => self.pane.key(k, body, &mut self.link),
            View::Player => self.player_key(k),
            View::Edit => self.edit_key(k),
        }
    }

    fn player_key(&mut self, k: Key) {
        use nous_ui::queue::Act;
        let act = match k.sym {
            s if s == ' ' as u64 => Some(Act::PlayPause),
            s if s == ffi::XK_Right => Some(Act::Nudge(5.0)),
            s if s == ffi::XK_Left => Some(Act::Nudge(-5.0)),
            s if s == ffi::XK_Up => {
                self.queue.move_selection(-1);
                None
            }
            s if s == ffi::XK_Down => {
                self.queue.move_selection(1);
                None
            }
            s if s == ffi::XK_Return || s == ffi::XK_KP_Enter => Some(Act::PlaySelected),
            s if s == 'n' as u64 => Some(Act::Next),
            s if s == 'p' as u64 => Some(Act::Previous),
            _ => None,
        };
        if let Some(a) = act {
            // The request the daemon would be sent. Nothing carries it there
            // yet, so the local half happens and the rest waits on the wiring.
            let _ = self.queue.act(a);
        }
    }

    fn edit_key(&mut self, k: Key) {
        use nous_ui::player::Command;
        let cmd = match k.sym {
            s if s == ' ' as u64 => Some(Command::PlayPause),
            s if s == ffi::XK_Right => Some(Command::Nudge(1.0)),
            s if s == ffi::XK_Left => Some(Command::Nudge(-1.0)),
            s if s == 'i' as u64 => Some(Command::MarkIn),
            s if s == 'o' as u64 => Some(Command::MarkOut),
            s if s == ffi::XK_Up => Some(Command::Prev),
            s if s == ffi::XK_Down => Some(Command::Next),
            _ => None,
        };
        if let Some(c) = cmd {
            let _ = self.editor.apply(c);
        }
    }

    pub fn text(&mut self, t: &str) {
        if self.view == View::Files {
            self.pane.text(t);
        }
    }

    pub fn click(&mut self, x: f64, y: f64, button: u32, w: f64, h: f64) {
        // A tab, if the click landed on one. Tested against the rectangles the
        // last frame actually drew.
        if let Some((v, _)) = self.tabs.iter().find(|(_, r)| r.contains(x, y)) {
            self.view = *v;
            return;
        }
        let body = self.body(w, h);
        if !body.contains(x, y) {
            return;
        }
        // Views lay themselves out from the origin of their own box.
        let (lx, ly) = (x - body.x, y - body.y);
        match self.view {
            // The file pane draws its own furniture from the body's origin, so
            // it is given the body and does its own arithmetic within it.
            View::Files => self
                .pane
                .click(lx, ly, button, at_origin(body), &mut self.link),
            View::Player => {
                let layout = nous_ui::queue::Layout::compute(&self.queue, body.w, body.h);
                if layout.scrub.contains(lx, ly) {
                    let f = layout.scrub_fraction(lx);
                    let to = f * self.queue.duration;
                    let _ = self.queue.act(nous_ui::queue::Act::SeekTo(to));
                } else if let Some(i) = layout.row_at(lx, ly) {
                    self.queue.selected = i;
                }
            }
            View::Edit => {
                let layout = nous_ui::player::Layout::compute(&self.editor, body.w, body.h);
                if layout.scrub.contains(lx, ly) {
                    let f = layout.scrub_fraction(lx);
                    let to = f * self.editor.duration();
                    let _ = self.editor.apply(nous_ui::player::Command::SeekTo(to));
                } else if let Some(i) = layout.clip_at(lx, ly) {
                    let _ = self.editor.apply(nous_ui::player::Command::Select(i));
                }
            }
        }
    }

    pub fn release(&mut self, _x: f64, _y: f64, _button: u32) {
        self.queue.scrubbing = false;
    }

    pub fn hover(&mut self, x: f64, y: f64, w: f64, h: f64) {
        if self.view == View::Files {
            let body = self.body(w, h);
            self.pane.hover(x - body.x, y - body.y);
        }
    }

    pub fn scroll(&mut self, dy: f64, w: f64, h: f64) {
        let body = self.body(w, h);
        match self.view {
            View::Files => self.pane.scroll(dy, at_origin(body)),
            View::Player => {
                let layout = nous_ui::queue::Layout::compute(&self.queue, body.w, body.h);
                let max = layout.max_scroll(&self.queue);
                self.queue.scroll = (self.queue.scroll + dy * 24.0).clamp(0.0, max);
            }
            View::Edit => {}
        }
    }

    // --- drawing ----------------------------------------------------------

    /// Where the current view gets to draw: everything under the tab bar.
    fn body(&self, w: f64, h: f64) -> Rect {
        Rect::new(0.0, TABS_H, w, (h - TABS_H).max(0.0))
    }

    pub fn render(&mut self, c: &Canvas, theme: &Theme, w: f64, h: f64) {
        c.fill_rect(Rect::new(0.0, 0.0, w, h), theme.backdrop_opaque);
        self.draw_tabs(c, theme, w);

        let body = self.body(w, h);
        if body.h <= 0.0 {
            return;
        }
        // Each view lays out from its own origin, so the canvas is moved under
        // it rather than every view being taught where the tab bar ends.
        c.clip_rect(body);
        c.translate(body.x, body.y);
        match self.view {
            View::Files => self.pane.render(c, theme, at_origin(body), &self.link),
            View::Player => {
                let layout = nous_ui::queue::Layout::compute(&self.queue, body.w, body.h);
                nous_ui::queue::render(c, &mut self.queue, theme, &layout);
            }
            View::Edit => {
                if self.editor.clips.is_empty() {
                    empty_note(
                        c,
                        theme,
                        body,
                        "No project open",
                        "A cut is made from a project the daemon keeps. \
                         Add something with `nousctl media.edit op=append`.",
                    );
                } else {
                    let layout = nous_ui::player::Layout::compute(&self.editor, body.w, body.h);
                    nous_ui::player::render(c, &mut self.editor, theme, &layout);
                }
            }
        }
        c.restore();
    }

    fn draw_tabs(&mut self, c: &Canvas, theme: &Theme, w: f64) {
        let bar = Rect::new(0.0, 0.0, w, TABS_H);
        c.fill_rect(bar, theme.backdrop_opaque);
        c.line(0.0, TABS_H - 0.5, w, TABS_H - 0.5, 1.0, theme.hairline);

        let f = theme.title_font();
        let mut x = Metrics::PAD;
        self.tabs.clear();
        for v in View::ALL {
            let (tw, th) = c.measure(v.title(), &f, None);
            let r = Rect::new(x, (TABS_H - th - 12.0) / 2.0, tw + 24.0, th + 12.0);
            let on = self.view == v;
            if on {
                c.fill_rounded(r, r.h / 2.0, theme.surface_active);
            }
            c.text(
                v.title(),
                r.x + 12.0,
                r.y + 6.0,
                &f,
                if on { theme.text } else { theme.text_dim },
                None,
            );
            self.tabs.push((v, r));
            x = r.right() + 6.0;
        }

        // The file pane says whether the daemon is there, in its own status
        // bar. Repeating it here would be two places to keep true.
        let _ = w;
    }
}

/// The body rectangle as the view sees it once the canvas has been moved under
/// it: same size, origin at zero. Views lay out from their own corner, and
/// handing one the window-space rectangle is how every offset bug starts.
fn at_origin(body: Rect) -> Rect {
    Rect::new(0.0, 0.0, body.w, body.h)
}

/// A view with nothing in it, saying so and saying what would fill it.
fn empty_note(c: &Canvas, theme: &Theme, body: Rect, head: &str, detail: &str) {
    let hf = theme.title_font();
    let bf = theme.body_font();
    let max = (body.w * 0.6).max(200.0);
    let (hw, hh) = c.measure(head, &hf, None);
    let (dw, _) = c.measure_wrapped(detail, &bf, max);
    let total = hh + 10.0 + 40.0;
    let y = (body.h - total) / 2.0;
    c.text(head, (body.w - hw) / 2.0, y, &hf, theme.text_dim, None);
    c.text_wrapped(
        detail,
        (body.w - dw.min(max)) / 2.0,
        y + hh + 10.0,
        &bf,
        theme.text_faint,
        max,
    );
}

// --- reading the disk ------------------------------------------------------

fn home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"))
}

/// The most recently touched edit project, if there is one.
pub fn newest_project() -> Option<(String, Json)> {
    let dir = nous_core::ipc::state_dir().join("media/projects");
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if p.extension().map(|x| x != "json").unwrap_or(true) {
            continue;
        }
        let when = e.metadata().ok()?.modified().ok()?;
        if best.as_ref().map(|(t, _)| when > *t).unwrap_or(true) {
            best = Some((when, p));
        }
    }
    let (_, path) = best?;
    let doc = nous_core::json::parse(&std::fs::read_to_string(&path).ok()?).ok()?;
    let name = doc.str_or("name", "untitled").to_string();
    Some((name, doc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nous_ui::draw::Image;
    use std::path::Path;

    fn key(sym: u64) -> Key {
        Key {
            sym,
            ctrl: false,
            shift: false,
            alt: false,
        }
    }

    fn scratch(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("nous-app-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn app_on(dir: &Path) -> App {
        let mut a = App::new(View::Files);
        a.pane = FilePane::new(dir.to_path_buf(), dir.to_path_buf());
        a
    }

    #[test]
    fn views_switch_by_number_and_by_tab() {
        let dir = scratch("switch");
        let mut a = app_on(&dir);
        a.key(key('2' as u64), 1000.0, 700.0);
        assert_eq!(a.view, View::Player);
        a.key(key('3' as u64), 1000.0, 700.0);
        assert_eq!(a.view, View::Edit);
        a.key(key(ffi::XK_Tab), 1000.0, 700.0);
        assert_eq!(
            a.view,
            View::Files,
            "tab should come round rather than stop"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_digit_typed_into_a_rename_is_a_digit_not_a_view_change() {
        // "1" switches view everywhere except where it is a character someone
        // is typing, and renaming a file to "2026" must not land in the player.
        let dir = scratch("digits");
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        let mut a = app_on(&dir);
        a.pane.act(crate::manage::Action::Rename, &mut a.link);
        assert!(a.is_typing(), "rename did not start");
        a.key(key('2' as u64), 1000.0, 700.0);
        assert_eq!(a.view, View::Files, "typing a digit changed view");
    }

    #[test]
    fn escape_closes_a_menu_before_it_closes_the_window() {
        let dir = scratch("escape");
        let mut a = app_on(&dir);
        assert!(!a.handles_escape(), "an idle window should close on escape");
        a.pane.menu = Some((10.0, 10.0, crate::manage::menu_for(false, false)));
        assert!(
            a.handles_escape(),
            "escape would have closed the window under a menu"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_click_in_the_body_is_measured_from_the_views_own_origin() {
        // The view lays itself out from zero; the tab bar sits above it. Miss
        // this and every click in the file grid lands one row too high.
        let dir = scratch("origin");
        for i in 0..12 {
            std::fs::write(dir.join(format!("f{i}.txt")), b"x").unwrap();
        }
        let mut a = app_on(&dir);
        let (w, h) = (1180.0, 720.0);
        let body = at_origin(a.body(w, h));
        let grid = a.pane.grid_rect(body);
        let layout = nous_ui::files::Layout::compute_bare(&a.pane.files, grid.w, grid.h);
        // Near the top edge of the first tile on the second row. A tile is far
        // taller than the tab bar, so a point in the middle of one would land
        // on the same tile either way and prove nothing.
        let i = layout.columns;
        let tile = layout.tile_rect(i, a.pane.files.scroll);
        let (vx, vy) = (grid.x + tile.x + tile.w / 2.0, grid.y + tile.y + 4.0);

        a.click(vx, vy + TABS_H, 1, w, h);
        assert_eq!(
            a.pane.files.selected, i,
            "the click landed on the wrong tile"
        );

        let mut b = app_on(&dir);
        b.click(vx, vy, 1, w, h);
        assert_ne!(
            b.pane.files.selected, i,
            "the offset makes no difference, so this proves nothing"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_click_on_a_tab_switches_to_it() {
        let dir = scratch("tabs");
        let mut a = app_on(&dir);
        // Tabs are hit-tested against what the last frame drew, so a frame has
        // to have been drawn.
        let img = Image::new(1180, 720).unwrap();
        a.render(&img.canvas(), &Theme::dark(), 1180.0, 720.0);
        let (_, player_tab) = a
            .tabs
            .iter()
            .find(|(v, _)| *v == View::Player)
            .cloned()
            .expect("a Player tab was drawn");
        a.click(player_tab.x + 4.0, player_tab.y + 4.0, 1, 1180.0, 720.0);
        assert_eq!(a.view, View::Player);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clicking_before_anything_is_drawn_does_not_reach_for_a_tab_that_is_not_there() {
        let dir = scratch("early-click");
        let mut a = app_on(&dir);
        assert!(a.tabs.is_empty());
        a.click(30.0, 20.0, 1, 1180.0, 720.0); // where a tab will be
        assert_eq!(
            a.view,
            View::Files,
            "acted on a tab that had never been drawn"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_view_draws_something_rather_than_an_empty_box() {
        let dir = scratch("draw-all");
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        for theme in [Theme::dark(), Theme::light()] {
            for v in View::ALL {
                let mut a = app_on(&dir);
                a.view = v;
                let img = Image::new(1180, 720).unwrap();
                a.render(&img.canvas(), &theme, 1180.0, 720.0);
                let body = a.body(1180.0, 720.0);
                assert!(img.variety(body) > 4, "{} is blank", v.title());
                // And the tab bar is always there to get back out with.
                assert!(
                    img.variety(Rect::new(0.0, 0.0, 1180.0, TABS_H)) > 4,
                    "no tab bar"
                );
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_window_too_short_for_a_tab_bar_does_not_draw_a_negative_body() {
        let dir = scratch("tiny");
        let mut a = app_on(&dir);
        let body = a.body(400.0, 20.0);
        assert!(body.h >= 0.0, "body height {}", body.h);
        let img = Image::new(400, 20).unwrap();
        a.render(&img.canvas(), &Theme::dark(), 400.0, 20.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_view_names_people_would_type_all_work() {
        assert_eq!(View::named("files"), Some(View::Files));
        assert_eq!(View::named("PLAYER"), Some(View::Player));
        assert_eq!(View::named("music"), Some(View::Player));
        assert_eq!(View::named("edit"), Some(View::Edit));
        assert_eq!(View::named("nonsense"), None);
    }
}
