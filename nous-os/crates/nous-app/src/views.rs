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

use crate::ask::{self, Ask};
use crate::filepane::FilePane;
use crate::history::{self, Deed};
use crate::link::Link;
use nous_core::json::{json_obj, Json};
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
    /// What was done, and how to take it back.
    History,
}

impl View {
    pub const ALL: [View; 4] = [View::Files, View::Player, View::Edit, View::History];

    pub fn title(self) -> &'static str {
        match self {
            View::Files => "Files",
            View::Player => "Player",
            View::Edit => "Edit",
            View::History => "History",
        }
    }

    pub fn named(s: &str) -> Option<View> {
        match s.to_ascii_lowercase().as_str() {
            "files" | "file" => Some(View::Files),
            "player" | "play" | "music" | "video" => Some(View::Player),
            "edit" | "editor" | "cut" => Some(View::Edit),
            "history" | "journal" | "undo" | "log" => Some(View::History),
            _ => None,
        }
    }

    fn next(self) -> View {
        match self {
            View::Files => View::Player,
            View::Player => View::Edit,
            View::Edit => View::History,
            View::History => View::Files,
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
    /// The ledger, as last read. Re-read whenever anything is done, because a
    /// history that is stale is worse than one that is absent: it says the
    /// thing you just did did not happen.
    pub deeds: Vec<Deed>,
    pub history_selected: usize,
    pub history_scroll: f64,
    /// What the link's change counter read when the ledger was last read. Any
    /// difference means something has happened since.
    seen_changes: u64,
    /// What to say about the last undo, wherever the user happens to be.
    pub said: Option<String>,
    /// The one line at the top that knows what you are looking at.
    pub ask: Ask,
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
            deeds: Vec::new(),
            history_selected: 0,
            history_scroll: 0.0,
            seen_changes: u64::MAX,
            said: None,
            ask: Ask::new(),
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
        self.pane.click(
            grid.x + 120.0,
            grid.y + 90.0,
            3,
            false,
            false,
            body,
            &mut self.link,
        );
    }

    /// Draw the curator's marks from a report given on the command line, or
    /// from a stand-in one, so the marked-up folder can be looked at without a
    /// daemon. Never reachable from the keyboard or the mouse.
    pub fn demo_marks(&mut self, from_file: Option<&str>) {
        let report = match from_file.and_then(|p| std::fs::read_to_string(p).ok()) {
            Some(text) => match nous_core::json::parse(&text) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("nous: {e}");
                    return;
                }
            },
            None => return,
        };
        let marked = crate::curated::apply(&mut self.pane.files.entries, &report);
        self.pane.files.proposal = crate::curated::summary(&report, marked);
    }

    /// Re-read the ledger from the daemon.
    pub fn refresh_history(&mut self) {
        let Some(reply) = self.link.journal(60) else {
            self.seen_changes = self.link.changes;
            self.deeds.clear();
            return;
        };
        self.seen_changes = self.link.changes;
        self.deeds = history::read(&reply);
        self.history_selected = self
            .history_selected
            .min(self.deeds.len().saturating_sub(1));
    }

    /// Take one entry back, by its sequence number.
    ///
    /// The undo goes through the broker like anything else, so undoing is
    /// itself written down — which is what makes it possible to see that
    /// something was undone, and by what.
    pub fn undo_seq(&mut self, seq: u64) {
        let params = json_obj([("seq", seq.into())]);
        match self.link.call_journal_revert(params) {
            Ok(_) => {
                self.said = Some(format!("took back #{seq}"));
                // The folder on screen may be what changed.
                self.pane.reload();
            }
            Err(e) => self.said = Some(e),
        }
    }

    /// Plain undo: the most recent thing that can still be taken back.
    pub fn undo_last(&mut self) {
        if self.deeds.is_empty() {
            self.refresh_history();
        }
        match history::newest_undoable(&self.deeds).map(|d| d.seq) {
            Some(seq) => self.undo_seq(seq),
            None if !self.link.connected() => {
                self.said = Some("no daemon — nothing is being recorded".to_string())
            }
            None => self.said = Some("nothing left to take back".to_string()),
        }
    }

    /// Show a ledger read from a file, so the history view can be looked at
    /// without a daemon that has done things. Never reachable from the
    /// keyboard or the mouse.
    pub fn demo_history(&mut self, from_file: Option<&str>) {
        let Some(text) = from_file.and_then(|p| std::fs::read_to_string(p).ok()) else {
            return;
        };
        match nous_core::json::parse(&text) {
            Ok(v) => {
                self.deeds = history::read(&v);
                self.seen_changes = self.link.changes;
            }
            Err(e) => eprintln!("nous: {e}"),
        }
    }

    /// Show a plan read from a file, so the ask bar can be looked at without a
    /// daemon to produce one. Never reachable from the keyboard or the mouse.
    pub fn demo_ask(&mut self, from_file: Option<&str>) {
        let Some(text) = from_file.and_then(|p| std::fs::read_to_string(p).ok()) else {
            return;
        };
        match nous_core::json::parse(&text) {
            Ok(v) => {
                let asked = v.str_or("asked", "tidy this folder").to_string();
                self.ask.edit.set(&asked);
                self.ask.state = crate::ask::read_plan(&v, &asked);
            }
            Err(e) => eprintln!("nous: {e}"),
        }
    }

    /// Put words in the bar and look them up, as though they had been typed.
    /// Never reachable from the keyboard or the mouse.
    pub fn demo_type(&mut self, text: &str) {
        self.ask.focused = true;
        self.ask.edit.set(text);
        self.look();
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
        self.ask.focused
            || self.ask.has_proposal()
            || (self.view == View::Files && self.pane.wants_escape())
    }

    // --- input ------------------------------------------------------------

    /// Whether the views are typing rather than commanding, in which case the
    /// keys that switch view are letters and belong to whatever is being typed.
    fn is_typing(&self) -> bool {
        self.ask.focused || (self.view == View::Files && self.pane.wants_escape())
    }

    pub fn key(&mut self, k: Key, w: f64, h: f64) {
        let body = self.body(w, h);
        if self.ask.focused {
            return self.ask_key(k);
        }
        // A proposal is showing but the bar does not have the keyboard —
        // Enter and Escape still belong to it, because those are the two
        // answers it is waiting for.
        if self.ask.has_proposal() {
            if k.is(ffi::XK_Return) || k.is(ffi::XK_KP_Enter) {
                self.ask.confirm(&mut self.link);
                self.pane.reload();
                return;
            }
            if k.is(ffi::XK_Escape) {
                self.ask.dismiss();
                return;
            }
        }
        if self.is_typing() {
            return self.pane.key(k, body, &mut self.link);
        }
        // Ctrl-K, or a slash, to start asking: the two gestures people who use
        // anything else already have in their fingers.
        if (k.ctrl && k.sym == 'k' as u64) || (!k.ctrl && !k.alt && k.sym == '/' as u64) {
            self.ask.focused = true;
            return;
        }
        // View switching, wherever you are. Ctrl-held, so a bare "1" can still
        // be typed at a file name.
        if !k.ctrl && !k.alt {
            match k.sym {
                s if s == '1' as u64 => return self.view = View::Files,
                s if s == '2' as u64 => return self.view = View::Player,
                s if s == '3' as u64 => return self.view = View::Edit,
                s if s == '4' as u64 => return self.view = View::History,
                _ => {}
            }
        }
        if k.is(ffi::XK_Tab) {
            self.view = self.view.next();
            return;
        }
        // Undo, from wherever you are. The one shortcut everybody already
        // knows, and the one this system most needs to honour: it is the whole
        // reason it is safe to let it act.
        if k.ctrl && k.sym == 'z' as u64 {
            self.undo_last();
            return;
        }
        match self.view {
            View::Files => self.pane.key(k, body, &mut self.link),
            View::Player => self.player_key(k),
            View::Edit => self.edit_key(k),
            View::History => self.history_key(k, body),
        }
    }

    fn ask_key(&mut self, k: Key) {
        use nous_ui::input::Step as EditStep;
        if k.is(ffi::XK_Escape) {
            return self.ask.dismiss();
        }
        // Up and down walk the results and step off the end onto the request,
        // so there is no gesture for "I meant the other reading" — you just
        // keep going in the direction you were already going.
        if k.is(ffi::XK_Down) {
            return self.ask.move_choice(1);
        }
        if k.is(ffi::XK_Up) {
            return self.ask.move_choice(-1);
        }
        if k.is(ffi::XK_Return) || k.is(ffi::XK_KP_Enter) {
            if self.ask.has_proposal() {
                self.ask.confirm(&mut self.link);
                self.pane.reload();
                return;
            }
            // On a result, Enter opens it. On the request row, Enter asks.
            if let Some(hit) = self.ask.chosen_hit().cloned() {
                self.open_hit(&hit);
                return;
            }
            let ctx = self.context_path();
            self.ask.submit(&mut self.link, &ctx);
            return;
        }
        let step = if k.ctrl {
            EditStep::Word
        } else {
            EditStep::Char
        };
        let edited = match k.sym {
            s if s == ffi::XK_BackSpace => {
                self.ask.edit.backspace(step);
                true
            }
            s if s == ffi::XK_Delete => {
                self.ask.edit.delete(step);
                true
            }
            s if s == ffi::XK_Left => {
                self.ask.edit.move_caret(-1, step, k.shift);
                false
            }
            s if s == ffi::XK_Right => {
                self.ask.edit.move_caret(1, step, k.shift);
                false
            }
            s if s == 'a' as u64 && k.ctrl => {
                self.ask.edit.select_all();
                false
            }
            _ => false,
        };
        if edited {
            self.look();
        }
    }

    /// Look for whatever is in the bar now.
    fn look(&mut self) {
        let places = crate::places::places(&self.pane.home);
        // The ledger is what makes past actions findable, so it is worth
        // having even when the History view has not been opened.
        if self.deeds.is_empty() && self.seen_changes != self.link.changes {
            self.refresh_history();
        }
        let deeds = std::mem::take(&mut self.deeds);
        self.ask.look(&mut self.link, &places, &deeds);
        self.deeds = deeds;
    }

    /// Go to whatever was chosen from the list.
    fn open_hit(&mut self, hit: &crate::find::Hit) {
        use crate::find::Sort;
        match hit.sort {
            Sort::View => {
                if let Some(v) = View::named(&hit.title) {
                    self.view = v;
                }
            }
            Sort::Place | Sort::Folder => {
                if let Some(p) = &hit.path {
                    self.view = View::Files;
                    self.pane.go(p.clone());
                }
            }
            Sort::File => {
                if let Some(p) = &hit.path {
                    // Show it where it lives, selected, rather than opening it
                    // straight away: finding something and looking at it are
                    // different, and only one of them can be undone.
                    self.view = View::Files;
                    if let Some(parent) = p.parent() {
                        self.pane.go(parent.to_path_buf());
                    }
                    let name = crate::manage::name_of(p);
                    self.pane.reload_selecting(&name);
                }
            }
            Sort::Deed => {
                self.view = View::History;
                if let Some(seq) = hit.seq {
                    if let Some(i) = self.deeds.iter().position(|d| d.seq == seq) {
                        self.history_selected = i;
                    }
                }
            }
        }
        self.ask.dismiss();
    }

    fn history_key(&mut self, k: Key, body: Rect) {
        if self.deeds.is_empty() {
            return;
        }
        let last = self.deeds.len() - 1;
        match k.sym {
            s if s == ffi::XK_Up => self.history_selected = self.history_selected.saturating_sub(1),
            s if s == ffi::XK_Down => self.history_selected = (self.history_selected + 1).min(last),
            s if s == ffi::XK_Home => self.history_selected = 0,
            s if s == ffi::XK_End => self.history_selected = last,
            // Return takes back the entry the keyboard is on, which is not
            // necessarily the newest — that is the point of a list.
            s if s == ffi::XK_Return || s == ffi::XK_KP_Enter => {
                if let Some(d) = self.deeds.get(self.history_selected) {
                    if d.can_undo() {
                        let seq = d.seq;
                        self.undo_seq(seq);
                    } else {
                        self.said = Some("that one cannot be taken back".to_string());
                    }
                }
            }
            0xffc2 => self.seen_changes = u64::MAX, // F5
            _ => return,
        }
        // Keep the selection on screen.
        let l = history::Layout::compute(&self.deeds, body.w, body.h, self.history_scroll);
        if let Some(r) = l.rows.get(self.history_selected) {
            if r.y < l.body.y {
                self.history_scroll -= l.body.y - r.y;
            } else if r.bottom() > l.body.bottom() {
                self.history_scroll += r.bottom() - l.body.bottom();
            }
            self.history_scroll = self.history_scroll.clamp(0.0, l.max_scroll());
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
        if self.ask.focused {
            self.ask.edit.insert(t);
            self.look();
            return;
        }
        if self.view == View::Files {
            self.pane.text(t);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn click(&mut self, x: f64, y: f64, button: u32, ctrl: bool, shift: bool, w: f64, h: f64) {
        // A tab, if the click landed on one. Tested against the rectangles the
        // last frame actually drew.
        if let Some((v, _)) = self.tabs.iter().find(|(_, r)| r.contains(x, y)) {
            self.view = *v;
            return;
        }
        let bar = ask::Layout::compute(&self.ask, w, TABS_H);
        if bar.bar.contains(x, y) {
            self.ask.focused = true;
            return;
        }
        if let Some(i) = bar.steps.iter().position(|r| r.contains(x, y)) {
            if let Some(hit) = self.ask.hits().get(i).cloned() {
                return self.open_hit(&hit);
            }
            // The row past the end is the request.
            if i == self.ask.hits().len() && !self.ask.hits().is_empty() {
                let ctx = self.context_path();
                self.ask.submit(&mut self.link, &ctx);
                return;
            }
        }
        let body = self.body(w, h);
        if !body.contains(x, y) {
            return;
        }
        // Clicking into a view puts the keyboard back where the click landed,
        // but leaves a proposal showing: clicking a file to check it against
        // the plan must not throw the plan away.
        self.ask.focused = false;
        // Views lay themselves out from the origin of their own box.
        let (lx, ly) = (x - body.x, y - body.y);
        match self.view {
            // The file pane draws its own furniture from the body's origin, so
            // it is given the body and does its own arithmetic within it.
            View::Files => {
                self.pane
                    .click(lx, ly, button, ctrl, shift, at_origin(body), &mut self.link)
            }
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
            View::History => {
                let l = history::Layout::compute(&self.deeds, body.w, body.h, self.history_scroll);
                if let Some(i) = l.undo_at(lx, ly) {
                    if let Some(seq) = self.deeds.get(i).map(|d| d.seq) {
                        self.history_selected = i;
                        self.undo_seq(seq);
                    }
                } else if let Some(i) = l.row_at(lx, ly) {
                    self.history_selected = i;
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
            View::History => {
                let l = history::Layout::compute(&self.deeds, body.w, body.h, self.history_scroll);
                self.history_scroll = (self.history_scroll + dy * 26.0).clamp(0.0, l.max_scroll());
            }
            View::Edit => {}
        }
    }

    // --- drawing ----------------------------------------------------------

    /// Where the current view gets to draw: everything under the tab row and
    /// the ask bar. The bar takes more room when it has a plan to show, and
    /// the view under it shrinks rather than being covered — a proposal that
    /// hides the folder it is about is a proposal you cannot check.
    fn body(&self, w: f64, h: f64) -> Rect {
        let top = TABS_H + ask::Layout::compute(&self.ask, w, TABS_H).height();
        Rect::new(0.0, top, w, (h - top).max(0.0))
    }

    /// What the ask bar should say it is about.
    fn context(&self) -> String {
        match self.view {
            View::Files => crate::manage::name_of(&self.pane.here()),
            View::Player => "what is playing".to_string(),
            View::Edit => self.editor.project.clone(),
            View::History => "what has been done".to_string(),
        }
    }

    /// The folder a request is about, as a path the daemon can use.
    fn context_path(&self) -> String {
        self.pane.here().to_string_lossy().to_string()
    }

    /// Work that was put off until something could be done about it.
    ///
    /// Navigation happens in the middle of handling a key and must stay a pure
    /// move; asking the daemon what it makes of a folder is a round trip. This
    /// is where the two meet, once per frame.
    pub fn settle(&mut self) {
        if self.view == View::Files && self.pane.wants_curating {
            self.pane.wants_curating = false;
            self.pane.curate(&mut self.link);
        }
        // The ledger is read when it is being looked at and known to be
        // behind. Re-reading it on every frame would be a round trip per
        // repaint to answer a question that only changes when something is
        // done.
        if self.view == View::History && self.seen_changes != self.link.changes {
            self.refresh_history();
        }
    }

    pub fn render(&mut self, c: &Canvas, theme: &Theme, w: f64, h: f64) {
        c.fill_rect(Rect::new(0.0, 0.0, w, h), theme.backdrop_opaque);
        self.draw_tabs(c, theme, w);
        let bar = ask::Layout::compute(&self.ask, w, TABS_H);
        let ctx = self.context();
        ask::render(c, &self.ask, theme, &bar, &ctx);

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
            View::History => {
                let l = history::Layout::compute(&self.deeds, body.w, body.h, self.history_scroll);
                history::render(
                    c,
                    &self.deeds,
                    self.history_selected,
                    theme,
                    &l,
                    now_secs(),
                    self.link.connected(),
                );
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
        self.draw_tab_row(c, theme, w);
    }

    fn draw_tab_row(&mut self, c: &Canvas, theme: &Theme, w: f64) {
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

        // On the right: what the last undo did, wherever it was pressed from.
        // Undo is the one action reachable from every view, so its answer has
        // to be readable from every view too.
        if let Some(said) = &self.said {
            let small = theme.small_font();
            let (sw, sh) = c.measure(said, &small, Some(w * 0.4));
            c.text(
                said,
                w - sw - Metrics::PAD,
                (TABS_H - sh) / 2.0,
                &small,
                theme.text_dim,
                Some(w * 0.4),
            );
        }
    }
}

/// Seconds since the epoch, for saying how long ago something happened.
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
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
        a.key(key('4' as u64), 1000.0, 700.0);
        assert_eq!(a.view, View::History);

        // Tabbing visits every view and comes round, so a view added without
        // a number key still has a way in.
        a.key(key(ffi::XK_Tab), 1000.0, 700.0);
        assert_eq!(
            a.view,
            View::Files,
            "tab should come round rather than stop"
        );
        let mut seen = vec![a.view];
        for _ in 1..View::ALL.len() {
            a.key(key(ffi::XK_Tab), 1000.0, 700.0);
            seen.push(a.view);
        }
        for v in View::ALL {
            assert!(seen.contains(&v), "{} cannot be tabbed to", v.title());
        }
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
        // Pinned to the grid: this is about the translation from window
        // coordinates to the view's own, not about how the folder is
        // arranged, and equal tiles in a known order make the target
        // unambiguous.
        a.pane.mode = crate::filepane::Mode::Grid;
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

        // Taken from the body rather than written as TABS_H: the ask bar sits
        // between the tabs and the view, and hard-coding the offset here would
        // make this test agree with a stale idea of the layout.
        let top = a.body(w, h).y;
        a.click(vx, vy + top, 1, false, false, w, h);
        assert_eq!(
            a.pane.files.selected, i,
            "the click landed on the wrong tile"
        );

        let mut b = app_on(&dir);
        b.pane.mode = crate::filepane::Mode::Grid;
        b.click(vx, vy, 1, false, false, w, h);
        assert_ne!(
            b.pane.files.selected, i,
            "the offset makes no difference, so this proves nothing"
        );
        assert!(
            top > TABS_H,
            "the ask bar takes no room, so this tests less than it should"
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
        a.click(
            player_tab.x + 4.0,
            player_tab.y + 4.0,
            1,
            false,
            false,
            1180.0,
            720.0,
        );
        assert_eq!(a.view, View::Player);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clicking_before_anything_is_drawn_does_not_reach_for_a_tab_that_is_not_there() {
        let dir = scratch("early-click");
        let mut a = app_on(&dir);
        assert!(a.tabs.is_empty());
        a.click(30.0, 20.0, 1, false, false, 1180.0, 720.0); // where a tab will be
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
    fn undo_reaches_the_daemon_from_every_view() {
        // Undo is the reason it is safe to let this system act. It cannot be
        // somewhere you have to navigate to first.
        let dir = scratch("undo-anywhere");
        for v in View::ALL {
            let mut a = app_on(&dir);
            a.view = v;
            a.key(
                Key {
                    sym: 'z' as u64,
                    ctrl: true,
                    shift: false,
                    alt: false,
                },
                1000.0,
                700.0,
            );
            assert!(a.said.is_some(), "ctrl-z did nothing in {}", v.title());
            assert_eq!(a.view, v, "undo moved us somewhere");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn with_no_daemon_undo_says_why_rather_than_nothing() {
        let dir = scratch("undo-nodaemon");
        let mut a = app_on(&dir);
        a.undo_last();
        let said = a.said.as_deref().unwrap_or("");
        assert!(said.contains("daemon"), "said {said:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_ledger_is_re_read_whenever_anything_has_been_done() {
        // A history that is stale is worse than one that is missing: it says
        // the thing you just did did not happen.
        let dir = scratch("ledger-stale");
        let mut a = app_on(&dir);
        a.view = View::History;
        a.settle();
        let after_first = a.seen_changes;
        assert_eq!(
            after_first, a.link.changes,
            "the ledger was not read at all"
        );

        // Something happened.
        a.link.changes += 1;
        a.settle();
        assert_eq!(a.seen_changes, a.link.changes, "the ledger was left behind");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_ledger_is_not_re_read_on_every_frame() {
        // It is a round trip. Asking once a repaint would be a round trip per
        // repaint to answer a question that only changes when something is
        // done.
        let dir = scratch("ledger-quiet");
        let mut a = app_on(&dir);
        a.view = View::History;
        a.settle();
        let before = a.link.changes;
        for _ in 0..5 {
            a.settle();
        }
        assert_eq!(a.link.changes, before, "settling asked the daemon again");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_slash_or_ctrl_k_starts_asking_from_any_view() {
        let dir = scratch("ask-start");
        for v in View::ALL {
            let mut a = app_on(&dir);
            a.view = v;
            a.key(key('/' as u64), 1180.0, 720.0);
            assert!(a.ask.focused, "slash did not start asking in {}", v.title());

            let mut b = app_on(&dir);
            b.view = v;
            b.key(
                Key {
                    sym: 'k' as u64,
                    ctrl: true,
                    shift: false,
                    alt: false,
                },
                1180.0,
                720.0,
            );
            assert!(
                b.ask.focused,
                "ctrl-k did not start asking in {}",
                v.title()
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn typing_into_the_bar_does_not_reach_the_view_underneath() {
        // "2" is a view switch and "/" starts asking. Both are also characters
        // in "sort the 2025 files".
        let dir = scratch("ask-typing");
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        let mut a = app_on(&dir);
        a.key(key('/' as u64), 1180.0, 720.0);
        a.text("sort the 2");
        a.key(key('2' as u64), 1180.0, 720.0);
        assert_eq!(
            a.view,
            View::Files,
            "a digit typed at the bar switched view"
        );
        assert_eq!(a.ask.edit.text(), "sort the 2");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_proposal_answers_enter_and_escape_wherever_the_keyboard_is() {
        // The bar may have lost focus to a click in the folder while a plan is
        // still showing. Those two keys are the answers it is waiting for.
        let dir = scratch("ask-answer");
        let mut a = app_on(&dir);
        a.ask.state = crate::ask::read_plan(
            &nous_core::json::parse(
                r#"{"steps":[{"capability":"fs.move","summary":"move it","risk":"write"}],
                    "plan":{"intent_id":"i1"}}"#,
            )
            .unwrap(),
            "move it",
        );
        a.ask.focused = false;
        assert!(
            a.handles_escape(),
            "escape would have closed the window on a live plan"
        );
        a.key(key(ffi::XK_Escape), 1180.0, 720.0);
        assert!(!a.ask.has_proposal(), "escape left the plan showing");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clicking_a_file_to_check_it_does_not_throw_the_plan_away() {
        // Checking the folder against the proposal is exactly what someone
        // should do before saying yes.
        let dir = scratch("ask-keep");
        for i in 0..6 {
            std::fs::write(dir.join(format!("f{i}.txt")), b"x").unwrap();
        }
        let mut a = app_on(&dir);
        a.ask.focused = true;
        a.ask.state = crate::ask::read_plan(
            &nous_core::json::parse(
                r#"{"steps":[{"capability":"fs.move","summary":"move it","risk":"write"}],
                    "plan":{"intent_id":"i1"}}"#,
            )
            .unwrap(),
            "move it",
        );
        let body = a.body(1180.0, 720.0);
        a.click(
            body.x + 60.0,
            body.y + 120.0,
            1,
            false,
            false,
            1180.0,
            720.0,
        );
        assert!(a.ask.has_proposal(), "clicking a file discarded the plan");
        assert!(!a.ask.focused, "the keyboard stayed in the bar");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_proposal_shrinks_the_view_rather_than_covering_it() {
        // A plan drawn over the folder it is about is a plan you cannot check.
        let dir = scratch("ask-room");
        let mut a = app_on(&dir);
        let plain = a.body(1180.0, 720.0);
        a.ask.state = crate::ask::read_plan(
            &nous_core::json::parse(
                r#"{"steps":[{"capability":"fs.move","summary":"a","risk":"write"},
                             {"capability":"fs.delete","summary":"b","risk":"elevated"}],
                    "plan":{"intent_id":"i1"}}"#,
            )
            .unwrap(),
            "x",
        );
        let with = a.body(1180.0, 720.0);
        assert!(with.y > plain.y, "the plan is drawn over the view");
        assert!(with.h < plain.h);
        assert!(with.bottom() <= 720.0 + 0.001);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn typing_finds_things_without_being_told_to_search() {
        // No mode, no button, no second box. The words are the search and the
        // same words are the request.
        let dir = scratch("find-live");
        let mut a = app_on(&dir);
        a.key(key('/' as u64), 1180.0, 720.0);
        a.text("Player");
        assert!(!a.ask.hits().is_empty(), "typing found nothing at all");
        assert_eq!(a.ask.hits()[0].title, "Player");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn opening_a_found_view_goes_there() {
        let dir = scratch("find-open-view");
        let mut a = app_on(&dir);
        a.key(key('/' as u64), 1180.0, 720.0);
        a.text("History");
        a.key(key(ffi::XK_Return), 1180.0, 720.0);
        assert_eq!(a.view, View::History, "opening a found view went nowhere");
        assert!(!a.ask.focused, "the bar stayed open over what it opened");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn opening_a_found_file_shows_it_where_it_lives_rather_than_launching_it() {
        // Finding something and opening it are different, and only one of
        // them can be undone.
        let dir = scratch("find-open-file");
        std::fs::create_dir(dir.join("deep")).unwrap();
        std::fs::write(dir.join("deep/needle.txt"), b"x").unwrap();
        let mut a = app_on(&dir);
        let hit = crate::find::Hit {
            sort: crate::find::Sort::File,
            title: "needle.txt".into(),
            detail: String::new(),
            path: Some(dir.join("deep/needle.txt")),
            seq: None,
            score: 1.0,
        };
        a.open_hit(&hit);
        assert_eq!(a.view, View::Files);
        assert_eq!(
            a.pane.here(),
            dir.join("deep"),
            "did not go to where it lives"
        );
        assert_eq!(
            a.pane.files.selected_entry().map(|e| e.name.as_str()),
            Some("needle.txt"),
            "found it and then did not point at it"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_request_reading_is_always_reachable_from_the_results() {
        // Something matching must never take away the ability to ask. Down
        // from the last result reaches it; it is not a separate gesture.
        let dir = scratch("find-request");
        let mut a = app_on(&dir);
        a.key(key('/' as u64), 1180.0, 720.0);
        a.text("Files");
        assert!(!a.ask.hits().is_empty());
        assert_eq!(
            a.ask.chosen,
            Some(0),
            "a plain name should start on the match"
        );
        for _ in 0..a.ask.hits().len() {
            a.key(key(ffi::XK_Down), 1180.0, 720.0);
        }
        assert_eq!(a.ask.chosen, None, "could not reach the request");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_sentence_starts_on_the_request_even_when_something_matches() {
        // "move budget.xlsx to Documents" matches a file, but it is plainly
        // an instruction. Guessing wrong costs one arrow key, which is why a
        // guess is allowed here at all.
        let dir = scratch("find-sentence");
        let mut a = app_on(&dir);
        a.key(key('/' as u64), 1180.0, 720.0);
        a.text("move Files into Player and sort them");
        assert_eq!(
            a.ask.chosen, None,
            "a plain instruction was read as a lookup"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clearing_the_bar_puts_the_results_away() {
        let dir = scratch("find-clear");
        let mut a = app_on(&dir);
        a.key(key('/' as u64), 1180.0, 720.0);
        a.text("Edit");
        assert!(!a.ask.hits().is_empty());
        for _ in 0..4 {
            a.key(key(ffi::XK_BackSpace), 1180.0, 720.0);
        }
        assert!(a.ask.hits().is_empty(), "an empty bar still showed a list");
        assert_eq!(a.body(1180.0, 720.0).y, TABS_H + crate::ask::BAR_H);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_view_names_people_would_type_all_work() {
        assert_eq!(View::named("files"), Some(View::Files));
        assert_eq!(View::named("PLAYER"), Some(View::Player));
        assert_eq!(View::named("music"), Some(View::Player));
        assert_eq!(View::named("edit"), Some(View::Edit));
        assert_eq!(View::named("history"), Some(View::History));
        assert_eq!(View::named("undo"), Some(View::History));
        assert_eq!(View::named("nonsense"), None);
    }
}
