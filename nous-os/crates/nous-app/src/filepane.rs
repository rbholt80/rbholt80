//! A file manager: the grid, and everything around it that makes a grid usable.
//!
//! The tiles were already here. What was missing is what a person actually does
//! with a folder — get to it, go back, open something with the program that
//! opens it, rename it, throw it away — and the furniture that makes those
//! reachable without being taught: places down the side, a path you can click
//! along the top, a menu on the right button, and the keys everyone's hands
//! already know.
//!
//! Nothing here writes to the disk. Every change is a capability the daemon
//! runs, so it lands in the journal and can be taken back.

use crate::curated;
use crate::link::Link;
use crate::manage::{self, Action, Clipboard};
use crate::places::{crumbs, places, History};
use nous_core::json::Json;
use nous_ui::draw::{Canvas, Rect};
use nous_ui::ffi;
use nous_ui::field::Field;
use nous_ui::files::{Entry, Files};
use nous_ui::input::{Edit, Step as EditStep};
use nous_ui::theme::{Metrics, Theme};
use nous_ui::window::Key;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

const SIDEBAR_W: f64 = 186.0;
/// Below this the sidebar goes away rather than crushing the grid.
const SIDEBAR_MIN_GRID: f64 = 430.0;
const PLACE_H: f64 = 30.0;
const CRUMBS_H: f64 = 40.0;
const STATUS_H: f64 = 28.0;
const MENU_ROW_H: f64 = 27.0;
/// The narrowest a menu gets. Wider when its own text needs it — a menu sized
/// by a constant has its longest label run into its longest shortcut, which is
/// exactly the pair a reader needs to tell apart.
const MENU_MIN_W: f64 = 190.0;
/// Room between the end of a label and the start of its shortcut.
const MENU_GAP: f64 = 26.0;
/// How long typed letters keep accumulating into one search before it restarts.
const TYPE_AHEAD_GAP: Duration = Duration::from_millis(900);

/// How the folder is arranged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Weighted by what is likely to matter, grouped by kind. The default,
    /// because it is the one that answers "what is in here" rather than
    /// "what is in here, alphabetically".
    Field,
    /// Equal tiles in name order. Kept because sometimes the question really
    /// is "where is the file called X", and then equal and alphabetical is
    /// exactly right.
    Grid,
}

/// What is being typed into, if anything.
pub enum Editing {
    None,
    /// Renaming the entry at this index.
    Rename {
        index: usize,
        edit: Edit,
    },
    /// Naming a new folder.
    NewFolder {
        edit: Edit,
    },
}

pub struct FilePane {
    pub files: Files,
    pub mode: Mode,
    pub history: History,
    pub home: PathBuf,
    pub clipboard: Option<Clipboard>,
    pub editing: Editing,
    /// The right-button menu, if one is open, and where it was opened.
    pub menu: Option<(f64, f64, Vec<Action>)>,
    pub menu_hover: Option<usize>,
    /// The last thing that went wrong, or the last thing that worked.
    pub status: Option<String>,
    /// Set when the folder changed and nobody has asked the curator about the
    /// new one yet. Checked once a frame, so navigation stays a pure move and
    /// the round trip happens in one place.
    pub wants_curating: bool,
    typed: String,
    typed_at: Option<Instant>,
    /// Rectangles from the last frame, so clicks are tested against what was
    /// drawn rather than against a second guess at where it went.
    drawn_places: Vec<(PathBuf, Rect)>,
    drawn_crumbs: Vec<(PathBuf, Rect)>,
    drawn_menu: Vec<(Action, Rect)>,
    back_btn: Rect,
    fwd_btn: Rect,
    up_btn: Rect,
}

impl FilePane {
    pub fn new(start: PathBuf, home: PathBuf) -> FilePane {
        let mut p = FilePane {
            files: Files::new("", Vec::new()),
            mode: Mode::Field,
            history: History::new(start.clone()),
            home,
            clipboard: None,
            editing: Editing::None,
            menu: None,
            menu_hover: None,
            status: None,
            wants_curating: true,
            typed: String::new(),
            typed_at: None,
            drawn_places: Vec::new(),
            drawn_crumbs: Vec::new(),
            drawn_menu: Vec::new(),
            back_btn: Rect::new(0.0, 0.0, 0.0, 0.0),
            fwd_btn: Rect::new(0.0, 0.0, 0.0, 0.0),
            up_btn: Rect::new(0.0, 0.0, 0.0, 0.0),
        };
        p.reload();
        p
    }

    pub fn here(&self) -> PathBuf {
        self.history.here().to_path_buf()
    }

    /// Re-read the folder and ask what the curator makes of it.
    ///
    /// The scan is the slow half and needs the daemon, so it is separate: the
    /// files appear at once and the opinions arrive with them or not at all.
    pub fn refresh(&mut self, link: &mut Link) {
        self.reload();
        self.curate(link);
    }

    /// Ask the curator about this folder and draw what it says onto the tiles.
    ///
    /// Silent when there is no daemon. An interface that complained every time
    /// you opened a folder without one would be unusable without one, and
    /// looking at your files does not need a daemon.
    pub fn curate(&mut self, link: &mut Link) {
        if !link.connected() {
            return;
        }
        let here = self.here();
        let args = nous_core::json::json_obj([(
            "roots",
            Json::Arr(vec![here.to_string_lossy().to_string().into()]),
        )]);
        let Some(report) = link.ask("curate.scan", args) else {
            return;
        };
        // The broker wraps each step's own answer; the findings are inside.
        let inner = report
            .get("steps")
            .and_then(|s| s.as_arr())
            .and_then(|a| a.first())
            .and_then(|s| s.get("result"))
            .cloned()
            .unwrap_or(report);
        let marked = curated::apply(&mut self.files.entries, &inner);
        self.files.proposal = curated::summary(&inner, marked);
    }

    /// Re-read the current folder, keeping the selection on the same file where
    /// it still exists. A rename that moved the selection to a random neighbour
    /// would be its own small betrayal.
    pub fn reload(&mut self) {
        let was = self.files.selected_entry().map(|e| e.name.clone());
        let dir = self.here();
        let entries = read_folder(&dir);
        let mut f = Files::new(&dir.to_string_lossy(), entries);
        if let Some(name) = was {
            if let Some(i) = f.entries.iter().position(|e| e.name == name) {
                f.selected = i;
            }
        }
        f.scroll = self.files.scroll.min(f.entries.len() as f64 * 200.0);
        self.files = f;
    }

    /// Keep the selection on a named file after the folder changed under it.
    pub fn reload_selecting(&mut self, name: &str) {
        self.reload();
        if let Some(i) = self.files.entries.iter().position(|e| e.name == name) {
            self.files.selected = i;
        }
    }

    pub fn go(&mut self, to: PathBuf) {
        if !to.is_dir() {
            return;
        }
        self.history.go(to);
        self.files.scroll = 0.0;
        self.editing = Editing::None;
        self.menu = None;
        self.reload();
        self.files.selected = 0;
        // Whoever moved us is expected to ask the curator; `go` is called from
        // places that have no link to hand.
        self.wants_curating = true;
    }

    fn back(&mut self) {
        if let Some(p) = self.history.back() {
            let _ = p;
            self.files.scroll = 0.0;
            self.reload();
        }
    }

    fn forward(&mut self) {
        if let Some(p) = self.history.forward() {
            let _ = p;
            self.files.scroll = 0.0;
            self.reload();
        }
    }

    fn up(&mut self) {
        // The folder we are leaving, so the eye lands on where it came from
        // rather than on whatever happens to sort first.
        let leaving = manage::name_of(&self.here());
        if let Some(parent) = self.here().parent().map(Path::to_path_buf) {
            self.go(parent);
            if let Some(i) = self.files.entries.iter().position(|e| e.name == leaving) {
                self.files.selected = i;
            }
        }
    }

    // --- doing things -----------------------------------------------------

    /// Open what is selected: into a folder, or out to whatever program the
    /// desktop already uses for that kind of file.
    pub fn open_selected(&mut self, link: &mut Link) {
        let Some(e) = self.files.selected_entry().cloned() else {
            return;
        };
        if e.is_dir {
            self.go(PathBuf::from(&e.path));
            return;
        }
        let job = manage::open(Path::new(&e.path));
        match link.invoke(job.cap, job.args, &job.why) {
            // Opening does not change the folder, so nothing is reloaded.
            Ok(_) => self.status = Some(format!("opened {}", e.name)),
            Err(err) => self.status = Some(err),
        }
    }

    pub fn act(&mut self, a: Action, link: &mut Link) {
        self.menu = None;
        let selected = self.files.selected_entry().cloned();
        match a {
            Action::Open | Action::OpenWith => self.open_selected(link),
            Action::Rename => {
                if let Some(e) = selected {
                    self.editing = Editing::Rename {
                        index: self.files.selected,
                        edit: Edit::from(&e.name),
                    };
                    // The extension is rarely what is being changed, so the
                    // stem is what a fresh rename has selected.
                    if let Editing::Rename { edit, .. } = &mut self.editing {
                        edit.select_all();
                    }
                }
            }
            Action::Copy | Action::Cut => {
                let chosen = self.files.chosen();
                if chosen.is_empty() {
                    return;
                }
                let paths: Vec<PathBuf> = chosen
                    .iter()
                    .filter_map(|i| self.files.entries.get(*i))
                    .map(|e| PathBuf::from(&e.path))
                    .collect();
                let verb = if a == Action::Cut { "cut" } else { "copied" };
                self.status = Some(match paths.len() {
                    1 => format!("{verb} {}", manage::name_of(&paths[0])),
                    n => format!("{verb} {n} items"),
                });
                self.clipboard = Some(Clipboard {
                    paths,
                    cut: a == Action::Cut,
                });
            }
            Action::Paste => {
                let Some(c) = self.clipboard.clone() else {
                    return;
                };
                let into = self.here();
                let mut moved = 0;
                for p in &c.paths {
                    match manage::paste(p, &into, c.cut) {
                        Ok(job) => match link.invoke(job.cap, job.args, &job.why) {
                            Ok(_) => moved += 1,
                            Err(e) => {
                                self.status = Some(e);
                                break;
                            }
                        },
                        Err(e) => {
                            self.status = Some(e);
                            break;
                        }
                    }
                }
                if moved > 0 {
                    let verb = if c.cut { "moved" } else { "copied" };
                    // A cut is spent once pasted. A copy is not: pasting the
                    // same thing into three folders is a thing people do.
                    if c.cut {
                        self.clipboard = None;
                    }
                    self.status = Some(format!("{verb} {moved} here"));
                    self.reload();
                }
            }
            Action::Trash => {
                let paths: Vec<PathBuf> = self
                    .files
                    .chosen()
                    .iter()
                    .filter_map(|i| self.files.entries.get(*i))
                    .map(|e| PathBuf::from(&e.path))
                    .collect();
                if paths.is_empty() {
                    return;
                }
                let mut done = 0;
                for p in &paths {
                    let job = manage::trash(p);
                    match link.invoke(job.cap, job.args, &job.why) {
                        Ok(_) => done += 1,
                        Err(e) => {
                            // Stop at the first refusal rather than carrying
                            // on: whatever stopped this one probably stops the
                            // rest, and a half-finished deletion nobody asked
                            // about is worse than none.
                            self.status = Some(if done == 0 {
                                e
                            } else {
                                format!("{e} — {done} already moved to the trash")
                            });
                            self.reload();
                            return;
                        }
                    }
                }
                self.status = Some(match done {
                    1 => format!("moved {} to the trash", manage::name_of(&paths[0])),
                    n => format!("moved {n} items to the trash"),
                });
                self.files.choose_none();
                self.reload();
            }
            Action::NewFolder => {
                self.editing = Editing::NewFolder {
                    edit: Edit::from("New Folder"),
                };
                if let Editing::NewFolder { edit } = &mut self.editing {
                    edit.select_all();
                }
            }
            Action::Refresh => {
                // Both halves: the folder may have changed on disk, and what
                // the curator makes of it may have changed with it. Re-reading
                // the files and keeping stale opinions about them drawn on top
                // is the worse of the two possible half-refreshes.
                self.status = None;
                self.files.proposal = None;
                self.refresh(link);
            }
            Action::Properties => {
                if let Some(e) = selected {
                    self.status = Some(format!(
                        "{} — {} — {}",
                        e.name,
                        if e.is_dir {
                            "folder".to_string()
                        } else {
                            manage::human_size(e.size)
                        },
                        e.path
                    ));
                }
            }
        }
    }

    fn commit_edit(&mut self, link: &mut Link) {
        let editing = std::mem::replace(&mut self.editing, Editing::None);
        match editing {
            Editing::Rename { index, edit } => {
                let Some(e) = self.files.entries.get(index).cloned() else {
                    return;
                };
                match manage::rename(Path::new(&e.path), edit.text()) {
                    Ok(job) => {
                        let name = edit.text().trim().to_string();
                        match link.invoke(job.cap, job.args, &job.why) {
                            Ok(_) => {
                                self.status = Some(job.why);
                                self.reload_selecting(&name);
                            }
                            Err(err) => self.status = Some(err),
                        }
                    }
                    // "that is already its name" is not worth saying out loud:
                    // pressing Enter on an unchanged name means "never mind".
                    Err(msg) if msg.contains("already its name") => {}
                    Err(msg) => self.status = Some(msg),
                }
            }
            Editing::NewFolder { edit } => {
                let here = self.here();
                match manage::new_folder(&here, edit.text()) {
                    Ok(job) => {
                        let name = edit.text().trim().to_string();
                        match link.invoke(job.cap, job.args, &job.why) {
                            Ok(_) => {
                                self.status = Some(job.why);
                                self.reload_selecting(&name);
                            }
                            Err(err) => self.status = Some(err),
                        }
                    }
                    Err(msg) => self.status = Some(msg),
                }
            }
            Editing::None => {}
        }
    }

    // --- input ------------------------------------------------------------

    /// True when the pane wants Escape for itself — closing a menu or
    /// abandoning a rename, rather than closing the window.
    pub fn wants_escape(&self) -> bool {
        self.menu.is_some() || !matches!(self.editing, Editing::None)
    }

    pub fn key(&mut self, k: Key, body: Rect, link: &mut Link) {
        // A rename in progress swallows everything: the letters belong to it,
        // and so does Escape.
        if !matches!(self.editing, Editing::None) {
            self.edit_key(k, link);
            return;
        }
        if self.menu.is_some() {
            if k.is(ffi::XK_Escape) {
                self.menu = None;
            }
            return;
        }

        if k.ctrl {
            match k.sym {
                s if s == 'c' as u64 => self.act(Action::Copy, link),
                s if s == 'x' as u64 => self.act(Action::Cut, link),
                s if s == 'v' as u64 => self.act(Action::Paste, link),
                s if s == 'n' as u64 && k.shift => self.act(Action::NewFolder, link),
                s if s == 'h' as u64 => self.go(self.home.clone()),
                s if s == 'a' as u64 => self.files.choose_all(),
                _ => {}
            }
            return;
        }
        if k.alt {
            match k.sym {
                s if s == ffi::XK_Left => self.back(),
                s if s == ffi::XK_Right => self.forward(),
                s if s == ffi::XK_Up => self.up(),
                _ => {}
            }
            return;
        }

        let grid = self.grid_rect(body);
        let layout = nous_ui::files::Layout::compute_bare(&self.files, grid.w, grid.h);
        let cols = layout.columns.max(1);
        // Shift with an arrow grows the choice rather than moving it, which
        // is the gesture every list in every system uses.
        if k.shift {
            let before = self.files.selected;
            match k.sym {
                s if s == ffi::XK_Left => self.files.move_selection(-1, 0, cols),
                s if s == ffi::XK_Right => self.files.move_selection(1, 0, cols),
                s if s == ffi::XK_Up => self.files.move_selection(0, -1, cols),
                s if s == ffi::XK_Down => self.files.move_selection(0, 1, cols),
                _ => return,
            }
            let to = self.files.selected;
            self.files.selected = before;
            self.files.extend_to(to);
            return;
        }
        match k.sym {
            s if s == ffi::XK_Left => self.files.move_selection(-1, 0, cols),
            s if s == ffi::XK_Right => self.files.move_selection(1, 0, cols),
            s if s == ffi::XK_Up => self.files.move_selection(0, -1, cols),
            s if s == ffi::XK_Down => self.files.move_selection(0, 1, cols),
            s if s == ffi::XK_Home => self.files.selected = 0,
            s if s == ffi::XK_End => {
                self.files.selected = self.files.entries.len().saturating_sub(1)
            }
            s if s == ffi::XK_Return || s == ffi::XK_KP_Enter => self.open_selected(link),
            s if s == ffi::XK_BackSpace => self.up(),
            s if s == ffi::XK_Delete => self.act(Action::Trash, link),
            // Backslash: near Return, unused everywhere else, and the two
            // arrangements are close enough in spirit that flipping between
            // them should cost one key rather than a menu.
            s if s == '\\' as u64 => self.toggle_mode(),
            // F2 and F5, which are the same everywhere and worth being the same
            // here.
            0xffbf => self.act(Action::Rename, link),
            0xffc2 => self.act(Action::Refresh, link),
            _ => return,
        }
        let layout = nous_ui::files::Layout::compute_bare(&self.files, grid.w, grid.h);
        self.files.reveal(&layout);
    }

    fn edit_key(&mut self, k: Key, link: &mut Link) {
        if k.is(ffi::XK_Escape) {
            self.editing = Editing::None;
            return;
        }
        if k.is(ffi::XK_Return) || k.is(ffi::XK_KP_Enter) {
            self.commit_edit(link);
            return;
        }
        let step = if k.ctrl {
            EditStep::Word
        } else {
            EditStep::Char
        };
        let edit = match &mut self.editing {
            Editing::Rename { edit, .. } | Editing::NewFolder { edit } => edit,
            Editing::None => return,
        };
        match k.sym {
            s if s == ffi::XK_BackSpace => edit.backspace(step),
            s if s == ffi::XK_Delete => edit.delete(step),
            s if s == ffi::XK_Left => edit.move_caret(-1, step, k.shift),
            s if s == ffi::XK_Right => edit.move_caret(1, step, k.shift),
            s if s == 'a' as u64 && k.ctrl => edit.select_all(),
            _ => {}
        }
    }

    /// Letters typed with nothing else going on: jump to a file by name.
    pub fn text(&mut self, t: &str) {
        if let Editing::Rename { edit, .. } | Editing::NewFolder { edit } = &mut self.editing {
            edit.insert(t);
            return;
        }
        if self.menu.is_some() || t.chars().all(char::is_whitespace) {
            return;
        }
        // A pause means a new search rather than a longer one, or hunting for
        // "budget" an hour later finds nothing because "report" is still there.
        let fresh = self
            .typed_at
            .map(|t| t.elapsed() > TYPE_AHEAD_GAP)
            .unwrap_or(true);
        if fresh {
            self.typed.clear();
        }
        self.typed.push_str(t);
        self.typed_at = Some(Instant::now());
        let names: Vec<String> = self.files.entries.iter().map(|e| e.name.clone()).collect();
        if let Some(i) = manage::type_ahead(&names, &self.typed) {
            self.files.selected = i;
        }
    }

    /// `ctrl` and `shift` are the click's own modifiers, which decide
    /// whether it starts a choice, adds to one, or extends one.
    #[allow(clippy::too_many_arguments)]
    pub fn click(
        &mut self,
        x: f64,
        y: f64,
        button: u32,
        ctrl: bool,
        shift: bool,
        body: Rect,
        link: &mut Link,
    ) {
        // An open menu takes the next click, wherever it lands: choosing from it
        // or dismissing it.
        if self.menu.is_some() {
            if let Some((a, _)) = self.drawn_menu.iter().find(|(_, r)| r.contains(x, y)) {
                let a = *a;
                self.act(a, link);
            } else {
                self.menu = None;
            }
            return;
        }
        if !matches!(self.editing, Editing::None) {
            self.commit_edit(link);
            return;
        }

        if button == 3 {
            let on_file = self
                .grid_rect(body)
                .contains(x, y)
                .then(|| self.hit_tile(x, y, body))
                .flatten()
                .map(|i| self.files.selected = i)
                .is_some();
            let menu = manage::menu_for(on_file, self.clipboard.is_some());
            self.menu = Some((x, y, menu));
            self.menu_hover = None;
            return;
        }

        if self.back_btn.contains(x, y) {
            return self.back();
        }
        if self.fwd_btn.contains(x, y) {
            return self.forward();
        }
        if self.up_btn.contains(x, y) {
            return self.up();
        }
        if let Some((p, _)) = self.drawn_places.iter().find(|(_, r)| r.contains(x, y)) {
            let p = p.clone();
            return self.go(p);
        }
        if let Some((p, _)) = self.drawn_crumbs.iter().find(|(_, r)| r.contains(x, y)) {
            let p = p.clone();
            return self.go(p);
        }
        if let Some(i) = self.hit_tile(x, y, body) {
            // Ctrl adds one, shift takes everything in between, and a plain
            // click starts again — which is what every file manager does and
            // therefore what people's hands already expect.
            if ctrl {
                self.files.toggle(i);
                return;
            }
            if shift {
                self.files.extend_to(i);
                return;
            }
            // Double-click opens, which is what a double-click does. A single
            // click that opened whatever was already chosen made every attempt
            // to re-select something into an accidental launch.
            let again = self.files.selected == i && recent_click(&mut self.typed_at, i);
            self.files.choose_only(i);
            if again {
                self.open_selected(link);
            }
        }
    }

    fn hit_tile(&self, x: f64, y: f64, body: Rect) -> Option<usize> {
        let grid = self.grid_rect(body);
        if !grid.contains(x, y) {
            return None;
        }
        match self.mode {
            Mode::Field => self.field(grid).hit(x - grid.x, y - grid.y),
            Mode::Grid => {
                let layout = nous_ui::files::Layout::compute_bare(&self.files, grid.w, grid.h);
                self.files.hit(&layout, x - grid.x, y - grid.y)
            }
        }
    }

    /// The arrangement for the space available.
    ///
    /// Recomputed rather than cached: it is a treemap over a few hundred
    /// numbers and no disk is touched, since every entry carries its own
    /// modification time.
    fn field(&self, grid: Rect) -> Field {
        Field::arrange(
            &self.files.entries,
            Rect::new(0.0, 0.0, grid.w, grid.h),
            now_secs(),
        )
    }

    pub fn toggle_mode(&mut self) {
        self.mode = match self.mode {
            Mode::Field => Mode::Grid,
            Mode::Grid => Mode::Field,
        };
    }

    pub fn hover(&mut self, x: f64, y: f64) {
        if self.menu.is_some() {
            self.menu_hover = self.drawn_menu.iter().position(|(_, r)| r.contains(x, y));
        }
    }

    pub fn scroll(&mut self, dy: f64, body: Rect) {
        let grid = self.grid_rect(body);
        let layout = nous_ui::files::Layout::compute_bare(&self.files, grid.w, grid.h);
        self.files.scroll_by(dy * 26.0, &layout);
    }

    // --- layout and drawing -----------------------------------------------

    pub fn sidebar_rect(&self, body: Rect) -> Rect {
        if body.w - SIDEBAR_W < SIDEBAR_MIN_GRID {
            return Rect::new(body.x, body.y, 0.0, 0.0);
        }
        Rect::new(body.x, body.y, SIDEBAR_W, body.h - STATUS_H)
    }

    pub fn grid_rect(&self, body: Rect) -> Rect {
        let side = self.sidebar_rect(body).w;
        Rect::new(
            body.x + side,
            body.y + CRUMBS_H,
            (body.w - side).max(0.0),
            (body.h - CRUMBS_H - STATUS_H).max(0.0),
        )
    }

    pub fn render(&mut self, c: &Canvas, theme: &Theme, body: Rect, link: &Link) {
        c.fill_rect(body, theme.backdrop_opaque);
        let side = self.sidebar_rect(body);
        if side.w > 0.0 {
            self.draw_sidebar(c, theme, side);
        }
        self.draw_crumbs(
            c,
            theme,
            Rect::new(body.x + side.w, body.y, body.w - side.w, CRUMBS_H),
        );

        let grid = self.grid_rect(body);
        c.clip_rect(grid);
        c.translate(grid.x, grid.y);
        match self.mode {
            Mode::Field => {
                let area = Rect::new(0.0, 0.0, grid.w, grid.h);
                let f = self.field(grid);
                let chosen = self.files.chosen();
                nous_ui::field::render(c, &f, &self.files.entries, &chosen, theme, area);
            }
            Mode::Grid => {
                let layout = nous_ui::files::Layout::compute_bare(&self.files, grid.w, grid.h);
                nous_ui::files::render(c, &mut self.files, theme, &layout);
                self.draw_edit_overlay(c, theme, &layout);
            }
        }
        c.restore();

        self.draw_status(
            c,
            theme,
            Rect::new(body.x, body.bottom() - STATUS_H, body.w, STATUS_H),
            link,
        );
        self.draw_menu(c, theme, body);
    }

    fn draw_sidebar(&mut self, c: &Canvas, theme: &Theme, r: Rect) {
        // Softened, not re-alpha'd: `surface` is a four-per-cent tint, and
        // setting its alpha to a half paints a grey slab down the side of the
        // window instead of lifting it slightly.
        c.fill_rect(r, theme.surface.softer(0.6));
        c.line(
            r.right() - 0.5,
            r.y,
            r.right() - 0.5,
            r.bottom(),
            1.0,
            theme.hairline,
        );
        let f = theme.body_font();
        let small = theme.small_font();
        c.text(
            "PLACES",
            r.x + Metrics::PAD,
            r.y + 12.0,
            &small,
            theme.text_faint,
            None,
        );
        let here = self.here();
        let mut y = r.y + 34.0;
        self.drawn_places.clear();
        for p in places(&self.home) {
            let row = Rect::new(r.x + 6.0, y, r.w - 12.0, PLACE_H - 2.0);
            let on = here == p.path;
            if on {
                c.fill_rounded(row, Metrics::RADIUS_SMALL / 2.0, theme.surface_active);
                c.fill_rounded(
                    Rect::new(row.x, row.y + 5.0, 3.0, row.h - 10.0),
                    1.5,
                    theme.voice,
                );
            }
            c.text(
                &p.name,
                row.x + 14.0,
                row.y + 5.0,
                &f,
                if on { theme.text } else { theme.text_dim },
                Some(row.w - 20.0),
            );
            self.drawn_places.push((p.path.clone(), row));
            y += PLACE_H;
        }
    }

    fn draw_crumbs(&mut self, c: &Canvas, theme: &Theme, r: Rect) {
        let f = theme.body_font();
        let small = theme.small_font();
        let cy = r.y + r.h / 2.0;

        // Back, forward, up. Drawn dim when there is nowhere to go, because a
        // button that looks live and does nothing is worse than one that says
        // it cannot.
        let mut x = r.x + Metrics::PAD;
        let btn = |c: &Canvas, x: f64, on: bool, dir: f64| -> Rect {
            let b = Rect::new(x, cy - 11.0, 22.0, 22.0);
            let col = if on { theme.text } else { theme.text_faint };
            // A chevron from two lines.
            let (mx, my) = (b.x + b.w / 2.0, b.y + b.h / 2.0);
            c.line(mx + 3.0 * dir, my - 5.0, mx - 2.0 * dir, my, 1.6, col);
            c.line(mx - 2.0 * dir, my, mx + 3.0 * dir, my + 5.0, 1.6, col);
            b
        };
        self.back_btn = btn(c, x, self.history.can_go_back(), 1.0);
        x += 26.0;
        self.fwd_btn = btn(c, x, self.history.can_go_forward(), -1.0);
        x += 30.0;
        // Up: the same chevron turned, drawn from lines for the same reason.
        {
            let b = Rect::new(x, cy - 11.0, 22.0, 22.0);
            let on = self.here().parent().is_some();
            let col = if on { theme.text } else { theme.text_faint };
            let (mx, my) = (b.x + b.w / 2.0, b.y + b.h / 2.0);
            c.line(mx - 5.0, my + 2.0, mx, my - 3.0, 1.6, col);
            c.line(mx, my - 3.0, mx + 5.0, my + 2.0, 1.6, col);
            self.up_btn = b;
            x = b.right() + 12.0;
        }

        self.drawn_crumbs.clear();
        let parts = crumbs(&self.here(), &self.home);
        let last = parts.len().saturating_sub(1);
        for (i, p) in parts.iter().enumerate() {
            let (tw, th) = c.measure(&p.name, &f, None);
            if x + tw > r.right() - 8.0 {
                // Out of room: say so rather than drawing off the edge.
                c.text("…", x, cy - th / 2.0, &f, theme.text_faint, None);
                break;
            }
            c.text(
                &p.name,
                x,
                cy - th / 2.0,
                &f,
                if i == last {
                    theme.text
                } else {
                    theme.text_dim
                },
                None,
            );
            self.drawn_crumbs
                .push((p.path.clone(), Rect::new(x - 3.0, r.y, tw + 6.0, r.h)));
            x += tw;
            if i != last {
                let (sw, _) = c.measure(" › ", &small, None);
                c.text(" › ", x, cy - th / 2.0, &small, theme.text_faint, None);
                x += sw;
            }
        }
        c.line(
            r.x,
            r.bottom() - 0.5,
            r.right(),
            r.bottom() - 0.5,
            1.0,
            theme.hairline,
        );
    }

    /// The rename box, drawn over the tile it belongs to.
    fn draw_edit_overlay(&self, c: &Canvas, theme: &Theme, layout: &nous_ui::files::Layout) {
        let (text, caret, at) = match &self.editing {
            Editing::Rename { index, edit } => {
                let Some(t) = layout.tile_for(*index, self.files.scroll) else {
                    return;
                };
                (
                    edit.text().to_string(),
                    edit.caret(),
                    Rect::new(t.x + 4.0, t.bottom() - 32.0, t.w - 8.0, 26.0),
                )
            }
            Editing::NewFolder { edit } => (
                edit.text().to_string(),
                edit.caret(),
                Rect::new(layout.body.x, layout.body.y, 220.0, 28.0),
            ),
            Editing::None => return,
        };
        c.fill_rounded(at, Metrics::RADIUS_SMALL / 2.0, theme.backdrop_opaque);
        c.stroke_rounded(at, Metrics::RADIUS_SMALL / 2.0, 1.5, theme.voice);
        let f = theme.body_font();
        c.clip_rect(at);
        c.text(&text, at.x + 6.0, at.y + 5.0, &f, theme.text, None);
        let (cw, _) = c.measure(&text[..caret.min(text.len())], &f, None);
        c.line(
            at.x + 6.0 + cw,
            at.y + 5.0,
            at.x + 6.0 + cw,
            at.bottom() - 5.0,
            1.5,
            theme.voice,
        );
        c.restore();
    }

    fn draw_status(&self, c: &Canvas, theme: &Theme, r: Rect, link: &Link) {
        c.fill_rect(r, theme.backdrop_opaque);
        c.line(r.x, r.y + 0.5, r.right(), r.y + 0.5, 1.0, theme.hairline);
        let small = theme.small_font();
        let cy = r.y + r.h / 2.0;

        // Left, most pressing first: what just went wrong, then what the
        // curator would like to do about this folder, then what is selected.
        // The proposal outranks the selection because it is about the whole
        // folder and the selection is already visible as a ring.
        // More than one chosen outranks everything: it is the fact that
        // changes what the next keystroke will do, and a Delete pressed
        // without knowing six things are selected is the mistake this line
        // exists to prevent.
        let chosen = self.files.chosen();
        let left = match (chosen.len(), &self.status, &self.files.proposal) {
            (n, _, _) if n > 1 => {
                let bytes: u64 = chosen
                    .iter()
                    .filter_map(|i| self.files.entries.get(*i))
                    .map(|e| e.size)
                    .sum();
                format!("{} selected · {}", n, manage::human_size(bytes))
            }
            (_, Some(s), _) => s.clone(),
            (_, None, Some(p)) => p.clone(),
            (_, None, None) => {
                let n = self.files.entries.len();
                let total: u64 = self.files.entries.iter().map(|e| e.size).sum();
                match self.files.selected_entry() {
                    Some(e) if !e.is_dir => format!("{} — {}", e.name, manage::human_size(e.size)),
                    Some(e) => format!("{} — folder", e.name),
                    None => format!("{} items · {}", n, manage::human_size(total)),
                }
            }
        };
        let (_, lh) = c.measure(&left, &small, None);
        c.text(
            &left,
            r.x + Metrics::PAD,
            cy - lh / 2.0,
            &small,
            match (chosen.len(), &self.status, &self.files.proposal) {
                (n, _, _) if n > 1 => theme.text,
                (_, Some(_), _) => theme.warn,
                (_, None, Some(_)) => theme.voice,
                (_, None, None) => theme.text_dim,
            },
            Some(r.w * 0.65),
        );

        // Right: whether anything can actually be done.
        let (note, colour) = if link.connected() {
            ("daemon connected", theme.ok)
        } else {
            (
                link.trouble.as_deref().unwrap_or("no daemon"),
                theme.text_faint,
            )
        };
        let (nw, nh) = c.measure(note, &small, None);
        c.text(
            note,
            r.right() - nw - Metrics::PAD,
            cy - nh / 2.0,
            &small,
            colour,
            None,
        );
    }

    fn draw_menu(&mut self, c: &Canvas, theme: &Theme, body: Rect) {
        self.drawn_menu.clear();
        let Some((mx, my, actions)) = self.menu.clone() else {
            return;
        };
        let f = theme.body_font();
        let small = theme.small_font();
        let gaps = actions.iter().filter(|a| a.starts_group()).count() as f64;
        let h = actions.len() as f64 * MENU_ROW_H + gaps * 7.0 + 10.0;
        // Wide enough for its widest row, measured rather than assumed.
        let width = actions
            .iter()
            .map(|a| {
                let (lw, _) = c.measure(a.label(), &f, None);
                let sw = if a.shortcut().is_empty() {
                    0.0
                } else {
                    c.measure(a.shortcut(), &small, None).0 + MENU_GAP
                };
                lw + sw + 28.0
            })
            .fold(MENU_MIN_W, f64::max)
            .min(body.w - 8.0);
        // Flip rather than run off: a menu opened near the bottom right of the
        // window must still be entirely on screen.
        let x = if mx + width > body.right() {
            (mx - width).max(body.x)
        } else {
            mx
        };
        let y = if my + h > body.bottom() {
            (my - h).max(body.y)
        } else {
            my
        };
        let box_ = Rect::new(x, y, width, h);
        c.fill_rounded(box_, Metrics::RADIUS_SMALL, theme.floating());
        c.stroke_rounded(box_, Metrics::RADIUS_SMALL, 1.0, theme.hairline);

        let mut ry = y + 5.0;
        for (i, a) in actions.iter().enumerate() {
            if a.starts_group() && i > 0 {
                c.line(
                    box_.x + 8.0,
                    ry + 3.0,
                    box_.right() - 8.0,
                    ry + 3.0,
                    1.0,
                    theme.hairline,
                );
                ry += 7.0;
            }
            let row = Rect::new(box_.x + 4.0, ry, box_.w - 8.0, MENU_ROW_H);
            if self.menu_hover == Some(self.drawn_menu.len()) {
                c.fill_rounded(row, Metrics::RADIUS_SMALL / 2.0, theme.surface_active);
            }
            c.text(a.label(), row.x + 10.0, row.y + 5.0, &f, theme.text, None);
            let sc = a.shortcut();
            if !sc.is_empty() {
                let (sw, _) = c.measure(sc, &small, None);
                c.text(
                    sc,
                    row.right() - sw - 10.0,
                    row.y + 7.0,
                    &small,
                    theme.text_faint,
                    None,
                );
            }
            self.drawn_menu.push((*a, row));
            ry += MENU_ROW_H;
        }
    }
}

/// Whether this click follows close enough on the last to be a double-click.
///
/// Reuses the type-ahead clock, which is the last time anything was typed or
/// clicked — good enough to tell a deliberate second click from one a minute
/// later, and one fewer piece of state to keep honest.
fn recent_click(last: &mut Option<Instant>, _index: usize) -> bool {
    let quick = last
        .map(|t| t.elapsed() < Duration::from_millis(450))
        .unwrap_or(false);
    *last = Some(Instant::now());
    quick
}

/// List a directory: folders first, then files, both case-insensitively
/// alphabetical. Dotfiles are configuration, not things you filed.
pub fn read_folder(dir: &Path) -> Vec<Entry> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<Entry> = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let meta = e.metadata().ok();
        let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
        out.push(Entry {
            name,
            path: e.path().to_string_lossy().to_string(),
            is_dir,
            size: meta.as_ref().map(|m| m.len()).unwrap_or(0),
            // Taken here, where the metadata is already in hand, rather than
            // by the view that wants it — which lays out every frame.
            modified: meta
                .as_ref()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0),
            thumb: None,
            mark: None,
        });
    }
    out.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use nous_ui::draw::Image;

    fn scratch(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("nous-pane-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn pane(dir: &Path) -> FilePane {
        FilePane::new(dir.to_path_buf(), dir.to_path_buf())
    }

    fn key(sym: u64) -> Key {
        Key {
            sym,
            ctrl: false,
            shift: false,
            alt: false,
        }
    }

    const BODY: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 1180.0,
        h: 676.0,
    };

    #[test]
    fn a_folder_is_read_folders_first_then_alphabetically() {
        let dir = scratch("listing");
        for name in ["zebra.txt", "apple.txt", ".hidden", "Middle.txt"] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        std::fs::create_dir(dir.join("zzz-folder")).unwrap();
        std::fs::create_dir(dir.join("aaa-folder")).unwrap();

        let e = read_folder(&dir);
        let names: Vec<&str> = e.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "aaa-folder",
                "zzz-folder",
                "apple.txt",
                "Middle.txt",
                "zebra.txt"
            ],
            "a folder buried among files is a folder you cannot find"
        );
        assert!(
            !names.contains(&".hidden"),
            "dotfiles are configuration, not things you filed"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_that_cannot_be_read_is_empty_rather_than_a_crash() {
        assert!(read_folder(Path::new("/nonexistent/never/was")).is_empty());
    }

    #[test]
    fn going_into_a_folder_and_back_out_lands_where_you_left() {
        // Coming up should put the eye on the folder just left, not on
        // whatever happens to sort first.
        let dir = scratch("updown");
        for n in ["aaa", "target", "zzz"] {
            std::fs::create_dir(dir.join(n)).unwrap();
        }
        let mut p = pane(&dir);
        p.files.selected = p
            .files
            .entries
            .iter()
            .position(|e| e.name == "target")
            .unwrap();
        p.open_selected(&mut Link::new());
        assert_eq!(p.here(), dir.join("target"));

        p.up();
        assert_eq!(p.here(), dir);
        assert_eq!(
            p.files.selected_entry().map(|e| e.name.as_str()),
            Some("target"),
            "came back out to the wrong file"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_reload_keeps_the_selection_on_the_same_file() {
        // A file appearing earlier in the sort must not drag the selection to
        // a different file under the hand about to act on it.
        let dir = scratch("reload");
        for n in ["b.txt", "c.txt"] {
            std::fs::write(dir.join(n), b"x").unwrap();
        }
        let mut p = pane(&dir);
        p.files.selected = 1; // c.txt
        assert_eq!(p.files.selected_entry().unwrap().name, "c.txt");
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        p.reload();
        assert_eq!(
            p.files.selected_entry().unwrap().name,
            "c.txt",
            "the selection slid onto another file"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn typing_letters_jumps_to_a_file_by_name() {
        let dir = scratch("typeahead");
        for n in ["alpha.txt", "beta.txt", "gamma.txt"] {
            std::fs::write(dir.join(n), b"x").unwrap();
        }
        let mut p = pane(&dir);
        p.text("g");
        assert_eq!(p.files.selected_entry().unwrap().name, "gamma.txt");
        p.text("b");
        // "gb" matches nothing, so the selection stays rather than jumping
        // somewhere arbitrary.
        assert_eq!(p.files.selected_entry().unwrap().name, "gamma.txt");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn typed_letters_go_into_a_rename_instead_of_hunting_for_files() {
        let dir = scratch("rename-type");
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        let mut p = pane(&dir);
        p.act(Action::Rename, &mut Link::new());
        p.text("hello");
        match &p.editing {
            Editing::Rename { edit, .. } => assert_eq!(edit.text(), "hello"),
            _ => panic!("the rename was abandoned by typing into it"),
        }
    }

    #[test]
    fn escape_abandons_a_rename_and_changes_nothing() {
        let dir = scratch("rename-escape");
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        let mut p = pane(&dir);
        let mut link = Link::new();
        p.act(Action::Rename, &mut link);
        p.text("something-else");
        p.key(key(ffi::XK_Escape), BODY, &mut link);
        assert!(matches!(p.editing, Editing::None));
        assert!(dir.join("a.txt").exists(), "the file was renamed anyway");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_menu_opens_where_it_was_asked_for_and_stays_on_screen() {
        let dir = scratch("menu");
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        let mut p = pane(&dir);
        let mut link = Link::new();
        // Right-click in the far bottom-right corner: the menu must flip
        // rather than draw off the edge where nothing can reach it.
        p.click(
            BODY.right() - 6.0,
            BODY.bottom() - 6.0,
            3,
            false,
            false,
            BODY,
            &mut link,
        );
        assert!(p.menu.is_some(), "right-click opened no menu");
        let img = Image::new(1180, 720).unwrap();
        p.render(&img.canvas(), &Theme::dark(), BODY, &link);
        for (a, r) in &p.drawn_menu {
            assert!(
                r.right() <= BODY.right() + 0.5,
                "{:?} runs off the right",
                a
            );
            assert!(
                r.bottom() <= BODY.bottom() + 0.5,
                "{:?} runs off the bottom",
                a
            );
            assert!(
                r.x >= BODY.x - 0.5 && r.y >= BODY.y - 0.5,
                "{:?} runs off the top-left",
                a
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_menu_hides_what_is_under_it() {
        // Drawn in the surface tint the menu was a few per cent of white laid
        // over the grid, and the file names read straight through it.
        //
        // The property is uniformity: the menu's ground must be one colour
        // wherever it is sampled, whatever happens to be behind that spot.
        // Comparing against a computed colour instead would fail on a
        // one-unit rounding difference from eight-bit compositing, which is
        // not the bug.
        for theme in [Theme::dark(), Theme::light()] {
            let dir = scratch("menu-opaque");
            for i in 0..14 {
                std::fs::write(dir.join(format!("a-longish-file-name-{i}.txt")), b"x").unwrap();
            }
            let mut p = pane(&dir);
            let mut link = Link::new();
            let grid = p.grid_rect(BODY);
            let t =
                nous_ui::files::Layout::compute_bare(&p.files, grid.w, grid.h).tile_rect(0, 0.0);
            let (cx, cy) = (grid.x + t.x + 20.0, grid.y + t.y + 20.0);

            let render = |p: &mut FilePane, link: &Link| {
                let img = Image::new(1180, 720).unwrap();
                p.render(&img.canvas(), &theme, BODY, link);
                img
            };

            // Where the menu will be, before there is one.
            p.click(cx, cy, 3, false, false, BODY, &mut link);
            let img = render(&mut p, &link);
            let spots: Vec<(i32, i32)> = p
                .drawn_menu
                .iter()
                .map(|(_, r)| ((r.right() - 3.0) as i32, (r.y + r.h / 2.0) as i32))
                .collect();
            assert!(spots.len() > 3, "no menu was drawn");

            let under: Vec<_> = spots.iter().map(|(x, y)| img.pixel(*x, *y)).collect();
            assert!(
                under.iter().all(|c| *c == under[0]),
                "the menu is see-through: its ground varies with what is behind it — {under:?}"
            );

            // And those same points are not all alike without a menu over
            // them, or uniformity would be no achievement.
            p.menu = None;
            let bare = render(&mut p, &link);
            let showing: Vec<_> = spots.iter().map(|(x, y)| bare.pixel(*x, *y)).collect();
            assert!(
                showing.iter().any(|c| *c != showing[0]),
                "nothing varies behind the menu, so this proves nothing"
            );
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn no_menu_label_runs_into_its_shortcut() {
        // "New Folder" and "Ctrl+Shift+N" are the widest pair, and at a fixed
        // width they overlapped — which is precisely the pair a reader has to
        // tell apart to learn the shortcut.
        let dir = scratch("menu-width");
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        let mut p = pane(&dir);
        let mut link = Link::new();
        p.click(300.0, 200.0, 3, false, false, BODY, &mut link);
        let img = Image::new(1180, 720).unwrap();
        let c = img.canvas();
        p.render(&c, &Theme::dark(), BODY, &link);

        let f = Theme::dark().body_font();
        let small = Theme::dark().small_font();
        for (a, r) in &p.drawn_menu {
            if a.shortcut().is_empty() {
                continue;
            }
            let label_end = r.x + 10.0 + c.measure(a.label(), &f, None).0;
            let shortcut_start = r.right() - 10.0 - c.measure(a.shortcut(), &small, None).0;
            assert!(
                shortcut_start > label_end,
                "{} overlaps {} by {:.0}px",
                a.label(),
                a.shortcut(),
                label_end - shortcut_start
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_click_away_from_an_open_menu_dismisses_it_without_acting() {
        let dir = scratch("menu-dismiss");
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        let mut p = pane(&dir);
        let mut link = Link::new();
        p.click(300.0, 300.0, 3, false, false, BODY, &mut link);
        let img = Image::new(1180, 720).unwrap();
        p.render(&img.canvas(), &Theme::dark(), BODY, &link);
        assert!(p.menu.is_some());
        p.click(20.0, BODY.bottom() - 60.0, 1, false, false, BODY, &mut link);
        assert!(p.menu.is_none(), "the menu stayed open");
        assert!(
            dir.join("a.txt").exists(),
            "dismissing a menu did something"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clicking_lands_on_the_right_file_in_either_arrangement() {
        // The two arrangements put files in completely different places. A
        // hit test written for one of them silently selects the wrong file in
        // the other.
        let dir = scratch("modes-hit");
        for i in 0..14 {
            std::fs::write(dir.join(format!("f{i:02}.jpg")), vec![b'x'; 1000 * (i + 1)]).unwrap();
        }
        for mode in [Mode::Field, Mode::Grid] {
            let mut p = pane(&dir);
            p.mode = mode;
            let mut link = Link::new();
            let grid = p.grid_rect(BODY);

            // Pick a target from whichever arrangement is in force.
            let (want, x, y) = match mode {
                Mode::Field => {
                    let f = p.field(grid);
                    let cell = f
                        .cells()
                        .find(|c| c.rect.w > 40.0 && c.rect.h > 40.0)
                        .expect("some cell is big enough to aim at");
                    (
                        cell.index,
                        grid.x + cell.rect.x + cell.rect.w / 2.0,
                        grid.y + cell.rect.y + cell.rect.h / 2.0,
                    )
                }
                Mode::Grid => {
                    let l = nous_ui::files::Layout::compute_bare(&p.files, grid.w, grid.h);
                    let t = l.tile_rect(3, 0.0);
                    (3, grid.x + t.x + t.w / 2.0, grid.y + t.y + t.h / 2.0)
                }
            };
            p.click(x, y, 1, false, false, BODY, &mut link);
            assert_eq!(p.files.selected, want, "clicked the wrong file in {mode:?}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn both_arrangements_draw_and_they_do_not_draw_the_same() {
        let dir = scratch("modes-draw");
        for i in 0..20 {
            std::fs::write(
                dir.join(format!("f{i:02}.{}", ["jpg", "pdf", "flac"][i % 3])),
                vec![b'x'; 500 * (i + 1)],
            )
            .unwrap();
        }
        let link = Link::new();
        let shot = |mode: Mode| {
            let mut p = pane(&dir);
            p.mode = mode;
            let img = Image::new(1180, 720).unwrap();
            p.render(&img.canvas(), &Theme::dark(), BODY, &link);
            img
        };
        let field = shot(Mode::Field);
        let grid = shot(Mode::Grid);
        let p = pane(&dir);
        let area = p.grid_rect(BODY);
        assert!(field.variety(area) > 5, "the field is blank");
        assert!(grid.variety(area) > 5, "the grid is blank");

        let mut differs = 0;
        for y in area.y as i32..area.bottom() as i32 {
            for x in area.x as i32..area.right() as i32 {
                if field.pixel(x, y) != grid.pixel(x, y) {
                    differs += 1;
                }
            }
        }
        assert!(
            differs > 5000,
            "the two arrangements draw the same thing: {differs} pixels differ"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_opens_arranged_by_what_matters_rather_than_alphabetically() {
        // Which is the whole argument. Alphabetical is still one key away.
        let dir = scratch("modes-default");
        let mut p = pane(&dir);
        assert_eq!(p.mode, Mode::Field);
        p.toggle_mode();
        assert_eq!(p.mode, Mode::Grid);
        p.toggle_mode();
        assert_eq!(p.mode, Mode::Field);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_single_click_selects_and_does_not_open() {
        // A single click that opened whatever was already chosen turned every
        // attempt to re-select into an accidental launch.
        let dir = scratch("single-click");
        std::fs::create_dir(dir.join("inner")).unwrap();
        let mut p = pane(&dir);
        let mut link = Link::new();
        let grid = p.grid_rect(BODY);
        let layout = nous_ui::files::Layout::compute(&p.files, grid.w, grid.h);
        let t = layout.tile_rect(0, 0.0);
        let (x, y) = (grid.x + t.x + t.w / 2.0, grid.y + t.y + t.h / 2.0);
        p.click(x, y, 1, false, false, BODY, &mut link);
        assert_eq!(p.files.selected, 0);
        assert_eq!(p.here(), dir, "a single click walked into the folder");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_sidebar_gives_way_before_it_crushes_the_grid() {
        let dir = scratch("narrow");
        let p = pane(&dir);
        let wide = p.sidebar_rect(Rect::new(0.0, 0.0, 1100.0, 600.0));
        assert!(wide.w > 0.0, "no sidebar in 1100px");
        let narrow = p.sidebar_rect(Rect::new(0.0, 0.0, 500.0, 600.0));
        assert_eq!(narrow.w, 0.0, "the grid was crushed to keep a sidebar");
        assert!(p.grid_rect(Rect::new(0.0, 0.0, 500.0, 600.0)).w > 0.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_grid_does_not_bring_a_second_status_bar() {
        // The pane has one, and it says more: what went wrong, and whether the
        // daemon is even there. Two bars stacked saying nearly the same thing
        // is what a view dropped into a frame looks like when the frame was
        // never told the view brought its own furniture.
        let dir = scratch("one-status");
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        let p = pane(&dir);
        let grid = p.grid_rect(BODY);
        let bare = nous_ui::files::Layout::compute_bare(&p.files, grid.w, grid.h);
        assert_eq!(bare.footer.h, 0.0, "the grid still reserves a footer");
        // And the space is given to the files rather than left as a gap.
        let with = nous_ui::files::Layout::compute(&p.files, grid.w, grid.h);
        assert!(bare.body.h > with.body.h, "the footer's room went nowhere");
        assert!(
            (bare.body.bottom() - grid.h).abs() < 0.001,
            "the grid stops {}px short of the bottom",
            grid.h - bare.body.bottom()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_whole_pane_draws_in_both_themes() {
        let dir = scratch("draw");
        std::fs::create_dir(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.txt"), b"hello").unwrap();
        for theme in [Theme::dark(), Theme::light()] {
            let mut p = pane(&dir);
            let link = Link::new();
            let img = Image::new(1180, 720).unwrap();
            p.render(&img.canvas(), &theme, BODY, &link);
            assert!(
                img.variety(p.sidebar_rect(BODY)) > 3,
                "the sidebar is blank"
            );
            assert!(img.variety(p.grid_rect(BODY)) > 4, "the grid is blank");
            let crumbs = Rect::new(0.0, 0.0, BODY.w, CRUMBS_H);
            assert!(img.variety(crumbs) > 3, "no path bar");
            let status = Rect::new(0.0, BODY.bottom() - STATUS_H, BODY.w, STATUS_H);
            assert!(img.variety(status) > 3, "no status bar");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clicking_a_place_goes_there() {
        let dir = scratch("places-click");
        std::fs::create_dir(dir.join("Music")).unwrap();
        let mut p = pane(&dir);
        let link = Link::new();
        let img = Image::new(1180, 720).unwrap();
        p.render(&img.canvas(), &Theme::dark(), BODY, &link);
        let (path, r) = p
            .drawn_places
            .iter()
            .find(|(path, _)| path.ends_with("Music"))
            .cloned()
            .expect("a Music shortcut was drawn");
        let mut link = Link::new();
        p.click(r.x + 4.0, r.y + 4.0, 1, false, false, BODY, &mut link);
        assert_eq!(p.here(), path);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clicking_a_crumb_walks_back_up_the_path() {
        let dir = scratch("crumb-click");
        std::fs::create_dir_all(dir.join("a/b")).unwrap();
        let mut p = pane(&dir);
        p.go(dir.join("a/b"));
        let link = Link::new();
        let img = Image::new(1180, 720).unwrap();
        p.render(&img.canvas(), &Theme::dark(), BODY, &link);
        let (path, r) = p
            .drawn_crumbs
            .iter()
            .find(|(path, _)| path.ends_with("a"))
            .cloned()
            .expect("an 'a' crumb was drawn");
        let mut link = Link::new();
        p.click(r.x + 2.0, r.y + r.h / 2.0, 1, false, false, BODY, &mut link);
        assert_eq!(p.here(), path, "clicking the path went nowhere");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_status_bar_says_how_many_are_selected() {
        // The fact that changes what the next keystroke does. A Delete pressed
        // without knowing six things are chosen is the mistake this prevents,
        // so it outranks even a warning.
        let dir = scratch("status-count");
        for i in 0..6 {
            std::fs::write(dir.join(format!("f{i}.txt")), vec![b'x'; 1000]).unwrap();
        }
        let mut p = pane(&dir);
        let link = Link::new();
        let status = Rect::new(0.0, BODY.bottom() - STATUS_H, BODY.w, STATUS_H);
        let shot = |p: &mut FilePane| {
            let img = Image::new(1180, 720).unwrap();
            p.render(&img.canvas(), &Theme::dark(), BODY, &link);
            img
        };
        let one = shot(&mut p);
        p.files.choose_only(0);
        p.files.extend_to(3);
        assert_eq!(p.files.chosen_count(), 4);
        let many = shot(&mut p);

        let differing = |a: &Image, b: &Image| {
            let mut n = 0;
            for y in status.y as i32..status.bottom() as i32 {
                for x in status.x as i32..status.right() as i32 {
                    if a.pixel(x, y) != b.pixel(x, y) {
                        n += 1;
                    }
                }
            }
            n
        };
        assert!(
            differing(&one, &many) > 50,
            "the bar says the same thing whether one file or four are chosen"
        );

        // Even with something to complain about, the count wins.
        p.status = Some("that name is taken".into());
        let with_error = shot(&mut p);
        assert!(
            differing(&with_error, &many) < 50,
            "an error hid the fact that four files are selected"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_sidebar_is_a_tint_rather_than_a_slab() {
        // `surface` is white at four per cent. Setting its alpha to a half
        // means fifty per cent white, which painted a grey slab down the side
        // of the window.
        let dir = scratch("sidebar-tint");
        let mut p = pane(&dir);
        let link = Link::new();
        for theme in [Theme::dark(), Theme::light()] {
            let img = Image::new(1180, 720).unwrap();
            p.render(&img.canvas(), &theme, BODY, &link);
            let side = p.sidebar_rect(BODY);
            let bar = img.pixel((side.x + 4.0) as i32, (side.bottom() - 20.0) as i32);
            let back = img.pixel(
                (p.grid_rect(BODY).right() - 4.0) as i32,
                (side.bottom() - 20.0) as i32,
            );
            // Close to the backdrop it sits on, not halfway to white or black.
            let gap = (bar.0 as i32 - back.0 as i32).abs()
                + (bar.1 as i32 - back.1 as i32).abs()
                + (bar.2 as i32 - back.2 as i32).abs();
            assert!(
                gap < 40,
                "the sidebar is {gap} away from the backdrop: {bar:?} vs {back:?}"
            );
            assert!(gap > 0, "the sidebar is invisible");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_the_curator_found_is_not_lost_with_the_grids_own_footer() {
        // The proposal used to be drawn in the grid's footer. Taking that
        // footer away to stop two status bars stacking would have taken the
        // one line saying what could be done about the folder with it.
        let dir = scratch("proposal");
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        let mut p = pane(&dir);
        let link = Link::new();
        let status = Rect::new(0.0, BODY.bottom() - STATUS_H, BODY.w, STATUS_H);

        let shot = |p: &mut FilePane| {
            let img = Image::new(1180, 720).unwrap();
            p.render(&img.canvas(), &Theme::dark(), BODY, &link);
            img
        };
        // Counted rather than measured by colour variety: the bar holds
        // antialiased text either way, so a variety count saturates at the
        // same number whatever the words are.
        let differing = |a: &Image, b: &Image, r: Rect| {
            let mut n = 0;
            for y in r.y as i32..r.bottom() as i32 {
                for x in r.x as i32..r.right() as i32 {
                    if a.pixel(x, y) != b.pixel(x, y) {
                        n += 1;
                    }
                }
            }
            n
        };

        let plain = shot(&mut p);
        p.files.proposal = Some("3 files worth a look here · 1.9 GB could be reclaimed".into());
        let with = shot(&mut p);
        assert!(
            differing(&plain, &with, status) > 50,
            "the curator's line is drawn nowhere"
        );

        // And a real complaint still outranks it: a rename that failed must
        // not be hidden behind a suggestion.
        p.status = Some("that name is taken".into());
        let err = shot(&mut p);
        assert!(
            differing(&err, &with, status) > 50,
            "an error was hidden behind the proposal"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refreshing_drops_opinions_about_files_that_may_have_changed() {
        // Re-reading the folder while keeping the old marks drawn on top would
        // leave "same file as report-2026.pdf" under a file whose twin was
        // deleted a moment ago.
        let dir = scratch("refresh");
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        let mut p = pane(&dir);
        let mut link = Link::new();
        p.files.proposal = Some("3 files worth a look here".into());
        p.files.entries[0].mark = Some(nous_ui::files::Mark {
            risk: nous_ui::theme::Risk::Elevated,
            note: "a duplicate".into(),
        });
        p.act(Action::Refresh, &mut link);
        assert!(p.files.proposal.is_none(), "kept a stale summary");
        assert!(
            p.files.entries[0].mark.is_none(),
            "kept a stale mark on a re-read file"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn opening_a_folder_asks_what_the_curator_makes_of_it() {
        // Navigation cannot make the round trip itself — it happens in the
        // middle of handling a key — so it leaves a note to be picked up.
        let dir = scratch("curate-flag");
        std::fs::create_dir(dir.join("inner")).unwrap();
        let mut p = pane(&dir);
        p.wants_curating = false;
        p.go(dir.join("inner"));
        assert!(
            p.wants_curating,
            "walking into a folder asked nothing about it"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn without_a_daemon_the_curator_is_not_missed() {
        // Looking at your own files needs no daemon, and an interface that
        // complained about it every time you opened a folder would be unusable
        // without one.
        let dir = scratch("curate-none");
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        let mut p = pane(&dir);
        let mut link = Link::new();
        p.curate(&mut link);
        assert!(
            p.status.is_none(),
            "complained about a missing daemon: {:?}",
            p.status
        );
        assert!(p.files.proposal.is_none());
        assert!(p.files.entries.iter().all(|e| e.mark.is_none()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn with_no_daemon_nothing_is_changed_and_the_reason_is_shown() {
        // Every change goes through the broker. With no broker there is no
        // change — and no silent failure either.
        let dir = scratch("no-daemon");
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        let mut p = pane(&dir);
        let mut link = Link::new();
        p.act(Action::Trash, &mut link);
        assert!(
            dir.join("a.txt").exists(),
            "a file was deleted without the daemon"
        );
        assert!(p.status.is_some(), "it failed silently");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
