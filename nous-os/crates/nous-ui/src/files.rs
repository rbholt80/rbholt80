//! Looking at a folder, with what the system thinks about it drawn on top.
//!
//! This is the difference between being told "413 moves proposed" and being
//! shown your own folder with the clutter marked in it. The same risk colours
//! the panel uses on a plan appear here on the files themselves: a duplicate
//! carries the colour of the action that would remove it, so the plan and the
//! folder are visibly the same claim seen twice.
//!
//! Nothing here acts. A marked file is a proposal about that file, and it is
//! approved the same way a plan is — deliberately, as a whole.

use crate::draw::{Canvas, Picture, Rect, Rgba};
use crate::theme::{Metrics, Risk, Theme};
use std::collections::HashMap;

/// What the system thinks about one file. Absent for most of them: a folder
/// where everything is marked is a folder where nothing stands out.
#[derive(Debug, Clone, PartialEq)]
pub struct Mark {
    /// The risk of the action being proposed for this file, so its colour here
    /// matches its colour in the plan.
    pub risk: Risk,
    /// Why, in the fewest words that are still true: "duplicate of holiday-1",
    /// "no other file has opened this in 3 years", "belongs in Pictures/2026".
    pub note: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    /// When it last changed, in seconds since the epoch.
    ///
    /// Carried rather than looked up, because the field view weighs files by
    /// how recently they were touched and it lays out on every frame. Asking
    /// the disk there would mean one `stat` per file per repaint.
    pub modified: u64,
    /// A cached PNG. Everything the daemon indexes gets one; a file it has not
    /// looked at yet has none and draws its extension instead.
    pub thumb: Option<String>,
    /// The first few lines of a text file, for showing what is in it.
    ///
    /// A folder of documents drawn as coloured rectangles with "PDF" on them
    /// tells you nothing you did not already know from the names. The point of
    /// giving a file room is to put something in it.
    pub blurb: Option<Vec<String>>,
    pub mark: Option<Mark>,
}

impl Entry {
    /// The extension, upper-cased, for a file with no picture. Bounded so a
    /// pathological name cannot push a tile out of shape.
    pub fn kind(&self) -> String {
        if self.is_dir {
            return "FOLDER".into();
        }
        match self.name.rsplit_once('.') {
            Some((stem, ext)) if !stem.is_empty() && ext.len() <= 5 && !ext.is_empty() => {
                ext.to_uppercase()
            }
            _ => "FILE".into(),
        }
    }
}

/// A folder on screen.
pub struct Files {
    pub folder: String,
    pub entries: Vec<Entry>,
    pub selected: usize,
    /// Everything else that is chosen, besides `selected`.
    ///
    /// Kept separate rather than folding `selected` into a set, because the
    /// two answer different questions: `selected` is where the keyboard is and
    /// what a rename or a preview acts on, while this is what a copy or a
    /// deletion acts on. Every file manager keeps both, and the ones that do
    /// not are the ones where shift-clicking loses your place.
    pub also: Vec<usize>,
    /// Vertical offset in pixels. Pixels rather than rows so a drag or a wheel
    /// can move by less than a whole tile.
    pub scroll: f64,
    /// One line summarising what the system would like to do to this folder,
    /// shown along the bottom with a way to say yes.
    pub proposal: Option<String>,
    /// Loaded pictures. A `None` value is a thumbnail that failed to load and
    /// must not be retried every frame.
    cache: HashMap<String, Option<Picture>>,
}

impl Files {
    pub fn new(folder: &str, entries: Vec<Entry>) -> Files {
        Files {
            folder: folder.to_string(),
            entries,
            selected: 0,
            also: Vec::new(),
            scroll: 0.0,
            proposal: None,
            cache: HashMap::new(),
        }
    }

    pub fn selected_entry(&self) -> Option<&Entry> {
        self.entries.get(self.selected)
    }

    /// Everything chosen, in the order it appears in the folder.
    ///
    /// Ordered rather than as it was clicked, because an operation on several
    /// files should happen in an order the person can predict from what they
    /// can see.
    pub fn chosen(&self) -> Vec<usize> {
        let mut all = self.also.clone();
        if self.selected < self.entries.len() {
            all.push(self.selected);
        }
        all.sort_unstable();
        all.dedup();
        all.retain(|i| *i < self.entries.len());
        all
    }

    pub fn is_chosen(&self, i: usize) -> bool {
        i == self.selected || self.also.contains(&i)
    }

    /// How many files an action would touch. One unless several are chosen.
    pub fn chosen_count(&self) -> usize {
        self.chosen().len()
    }

    /// Choose only this one, forgetting anything else. A plain click.
    pub fn choose_only(&mut self, i: usize) {
        self.selected = i;
        self.also.clear();
    }

    /// Add or remove one from the choice, keeping the rest. Ctrl-click.
    pub fn toggle(&mut self, i: usize) {
        if i >= self.entries.len() {
            return;
        }
        if i == self.selected {
            // Un-choosing where the keyboard is: hand that role to another
            // chosen file rather than leaving nothing selected.
            if let Some(next) = self.also.pop() {
                self.selected = next;
            }
            return;
        }
        match self.also.iter().position(|x| *x == i) {
            Some(at) => {
                self.also.remove(at);
            }
            None => {
                // The old selection stays chosen; the keyboard moves to the
                // new one, which is what makes a run of ctrl-clicks build up
                // rather than swap round.
                self.also.push(self.selected);
                self.selected = i;
            }
        }
    }

    /// Choose everything between where the keyboard is and `i`. Shift-click.
    pub fn extend_to(&mut self, i: usize) {
        if i >= self.entries.len() {
            return;
        }
        let (lo, hi) = if i < self.selected {
            (i, self.selected)
        } else {
            (self.selected, i)
        };
        self.also = (lo..=hi).filter(|x| *x != i).collect();
        self.selected = i;
    }

    pub fn choose_all(&mut self) {
        self.also = (0..self.entries.len())
            .filter(|i| *i != self.selected)
            .collect();
    }

    pub fn choose_none(&mut self) {
        self.also.clear();
    }

    /// How many files carry a mark. Drawn in the header, because "3 of 137
    /// need attention" is the sentence someone opening a folder wants.
    pub fn marked(&self) -> usize {
        self.entries.iter().filter(|e| e.mark.is_some()).count()
    }

    /// The worst thing proposed for anything in this folder, for the header
    /// marker — the same rule the panel uses for a plan.
    pub fn peak_risk(&self) -> Option<Risk> {
        self.entries
            .iter()
            .filter_map(|e| e.mark.as_ref())
            .map(|m| m.risk)
            .max()
    }

    /// Move the selection by whole tiles. `columns` comes from the layout, so
    /// Down moves down a visible row rather than an imagined one.
    pub fn move_selection(&mut self, dx: i32, dy: i32, columns: usize) {
        if self.entries.is_empty() {
            return;
        }
        let columns = columns.max(1) as i32;
        let n = self.entries.len() as i32;
        let cur = self.selected as i32;

        let next = if dy != 0 {
            let moved = cur + dy * columns;
            // Stepping past the last row lands on the last file rather than
            // refusing to move, which is what a half-full final row needs.
            if moved >= n {
                if cur / columns == (n - 1) / columns {
                    cur
                } else {
                    n - 1
                }
            } else if moved < 0 {
                cur
            } else {
                moved
            }
        } else {
            (cur + dx).clamp(0, n - 1)
        };
        self.selected = next as usize;
        // An arrow key without shift means "go here", not "add here". Leaving
        // a stale multiple choice behind is how a file manager deletes six
        // files when you meant one.
        self.also.clear();
    }

    /// Pull the scroll along so the selection is on screen. Called after any
    /// move, with the layout that produced the tiles.
    pub fn reveal(&mut self, layout: &Layout) {
        if self.entries.is_empty() {
            return;
        }
        let tile = layout.tile_rect(self.selected, self.scroll);
        let top = layout.body.y;
        let bottom = layout.body.bottom();
        if tile.y < top {
            self.scroll -= top - tile.y;
        } else if tile.bottom() > bottom {
            self.scroll += tile.bottom() - bottom;
        }
        self.clamp_scroll(layout);
    }

    pub fn scroll_by(&mut self, dy: f64, layout: &Layout) {
        self.scroll += dy;
        self.clamp_scroll(layout);
    }

    fn clamp_scroll(&mut self, layout: &Layout) {
        let max = (layout.content_height - layout.body.h).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max);
    }

    /// Which entry is under a point, if any.
    pub fn hit(&self, layout: &Layout, x: f64, y: f64) -> Option<usize> {
        if !layout.body.contains(x, y) {
            return None;
        }
        (0..self.entries.len()).find(|i| {
            layout
                .tile_for(*i, self.scroll)
                .is_some_and(|t| t.contains(x, y))
        })
    }

    fn picture(&mut self, path: &str) -> Option<&Picture> {
        self.cache
            .entry(path.to_string())
            .or_insert_with(|| Picture::load(path).ok())
            .as_ref()
    }
}

// --- layout ---------------------------------------------------------------

/// The width a tile would like to be. Actual width is computed from it: a row
/// divides the space it has exactly, so the grid lines up with the header and
/// the footer instead of floating in the middle with slack on both sides.
const TILE_TARGET_W: f64 = 186.0;
const TILE_GAP: f64 = 14.0;
/// Picture area, as a fraction of tile width. 3:2 is the shape most
/// photographs and video frames already are.
const THUMB_RATIO: f64 = 2.0 / 3.0;
const CAPTION_H: f64 = 46.0;
/// The scroll indicator down the right edge, shown only when there is more.
const SCROLLBAR_W: f64 = 3.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub panel: Rect,
    pub header: Rect,
    /// The composition strip: what this folder is made of, at a glance.
    pub strip: Rect,
    /// The scrolling area the tiles live in.
    pub body: Rect,
    pub footer: Rect,
    pub columns: usize,
    pub tile_w: f64,
    pub tile_h: f64,
    pub thumb_h: f64,
    /// Full height of all the tiles, which is what the scroll is clamped to.
    pub content_height: f64,
}

impl Layout {
    pub fn compute(files: &Files, width: f64, height: f64) -> Layout {
        Layout::compute_inner(files, width, height, true)
    }

    /// The same grid with no footer of its own.
    ///
    /// For a window that already has a status bar. Two bars saying almost the
    /// same thing, one above the other, is what it looks like when a view is
    /// dropped into a frame that was not told the view brought furniture.
    pub fn compute_bare(files: &Files, width: f64, height: f64) -> Layout {
        Layout::compute_inner(files, width, height, false)
    }

    fn compute_inner(files: &Files, width: f64, height: f64, footer: bool) -> Layout {
        let pad = Metrics::PAD;
        let header_h = 58.0;
        let strip_h = if files.entries.is_empty() { 0.0 } else { 30.0 };
        // The footer always has something worth saying: a pending plan, or what
        // the selected file is. An interface that shows a strip only sometimes
        // reflows under you as you move around.
        let footer_h = if footer { 52.0 } else { 0.0 };

        let inner_w = (width - pad * 2.0).max(0.0);
        let columns = (((inner_w + TILE_GAP) / (TILE_TARGET_W + TILE_GAP)).floor() as usize).max(1);
        // Divide the row exactly, so the grid lines up with the header and the
        // footer rather than floating with slack on both sides. Capped, because
        // a narrow window fits only one column and stretching a single tile to
        // fill it turns a thumbnail into a poster.
        let tile_w = ((inner_w - (columns - 1) as f64 * TILE_GAP) / columns as f64)
            .clamp(1.0, TILE_TARGET_W * 1.35);
        let thumb_h = (tile_w * THUMB_RATIO).round();
        let tile_h = thumb_h + CAPTION_H;

        let rows = files.entries.len().div_ceil(columns);
        let content_height = if rows == 0 {
            0.0
        } else {
            rows as f64 * tile_h + (rows - 1) as f64 * TILE_GAP
        };

        let body_y = header_h + strip_h;
        let body_h = (height - body_y - footer_h).max(0.0);

        Layout {
            panel: Rect::new(0.0, 0.0, width, height),
            header: Rect::new(pad, 0.0, inner_w, header_h),
            strip: Rect::new(pad, header_h, inner_w, strip_h),
            body: Rect::new(pad, body_y, inner_w, body_h),
            footer: Rect::new(pad, body_y + body_h, inner_w, footer_h),
            columns,
            tile_w,
            tile_h,
            thumb_h,
            content_height,
        }
    }

    /// Where tile `i` sits, whether or not that is on screen.
    ///
    /// Separate from [`Layout::tile_for`] because scrolling something into view
    /// has to ask where an *off-screen* tile is -- asking the culling version
    /// returned None precisely when the answer was needed, and the scroll
    /// silently did nothing.
    pub fn tile_rect(&self, i: usize, scroll: f64) -> Rect {
        let col = i % self.columns;
        let row = i / self.columns;
        Rect::new(
            self.body.x + col as f64 * (self.tile_w + TILE_GAP),
            self.body.y + row as f64 * (self.tile_h + TILE_GAP) - scroll,
            self.tile_w,
            self.tile_h,
        )
    }

    /// Where tile `i` sits, or `None` when it is far enough off screen not to
    /// be worth drawing. For rendering and hit-testing only.
    pub fn tile_for(&self, i: usize, scroll: f64) -> Option<Rect> {
        let r = self.tile_rect(i, scroll);
        // One tile of slack either side, so scrolling never shows a gap where a
        // tile should have been drawn.
        if r.bottom() < self.body.y - self.tile_h || r.y > self.body.bottom() + self.tile_h {
            return None;
        }
        Some(r)
    }

    /// How many rows are fully visible, for Page Up and Page Down.
    pub fn visible_rows(&self) -> usize {
        ((self.body.h + TILE_GAP) / (self.tile_h + TILE_GAP))
            .floor()
            .max(1.0) as usize
    }

    pub fn scrollable(&self) -> bool {
        self.content_height > self.body.h + 0.5
    }
}

// --- drawing --------------------------------------------------------------

pub fn render(c: &Canvas, files: &mut Files, theme: &Theme, layout: &Layout) {
    c.fill_rect(layout.panel, theme.backdrop_opaque);

    draw_header(c, files, theme, layout);
    if layout.strip.h > 0.0 {
        draw_strip(c, files, theme, layout);
    }

    if files.entries.is_empty() {
        draw_empty(c, theme, layout.body);
        if layout.footer.h > 0.0 {
            draw_footer(c, files, theme, layout);
        }
        return;
    }

    c.clip_rect(layout.body);
    let scroll = files.scroll;
    let chosen = files.chosen();
    for i in 0..files.entries.len() {
        let Some(tile) = layout.tile_for(i, scroll) else {
            continue;
        };
        draw_tile(c, files, theme, i, tile, chosen.contains(&i));
    }
    c.restore();

    if layout.scrollable() {
        draw_scrollbar(c, files, theme, layout);
    }
    if layout.footer.h > 0.0 {
        draw_footer(c, files, theme, layout);
    }
}

fn draw_header(c: &Canvas, files: &Files, theme: &Theme, layout: &Layout) {
    let h = layout.header;
    let marker = 11.0;
    let cy = h.y + h.h / 2.0 + 2.0;

    // The same diamond the panel uses, in the colour of the worst thing
    // proposed here. A folder with nothing to answer for gets a hollow one.
    let peak = files.peak_risk();
    let colour = peak.map_or(theme.text_faint, |r| theme.risk(r));
    diamond(
        c,
        h.x + marker / 2.0,
        cy,
        marker / 2.0,
        colour,
        peak.is_some(),
    );

    let x = h.x + marker + Metrics::GAP;
    let (_, th) = c.measure("Ag", &theme.title_font(), None);
    c.text(
        folder_name(&files.folder),
        x,
        cy - th / 2.0,
        &theme.title_font(),
        theme.text,
        Some(h.w * 0.55),
    );

    let small = theme.small_font();
    let marked = files.marked();
    let summary = if marked == 0 {
        format!("{} items", files.entries.len())
    } else {
        format!("{} items · {marked} need attention", files.entries.len())
    };
    let (sw, sh) = c.measure(&summary, &small, None);
    c.text(
        &summary,
        h.right() - sw,
        cy - sh / 2.0,
        &small,
        if marked == 0 {
            theme.text_dim
        } else {
            theme.text
        },
        None,
    );
}

/// What this folder is made of, as one bar.
///
/// A file manager tells you a folder has 137 things in it. This says what they
/// are and how much room each kind takes, which is the question actually being
/// asked when someone opens a folder they mean to tidy. Anything marked is
/// drawn in the colour of what would happen to it, so the part worth acting on
/// is visible as a proportion of the whole.
fn draw_strip(c: &Canvas, files: &Files, theme: &Theme, layout: &Layout) {
    let s = layout.strip;
    let bar = Rect::new(s.x, s.y + 6.0, s.w, 5.0);
    c.fill_rounded(bar, 2.5, theme.surface);

    let total: u64 = files.entries.iter().map(|e| e.size).sum();
    if total == 0 {
        return;
    }

    // Marked bytes first, in risk order, so the thing to act on is at the left
    // edge where the eye starts. Then everything else, by kind.
    let mut segments: Vec<(u64, Rgba)> = Vec::new();
    for risk in [Risk::Critical, Risk::Elevated, Risk::Write, Risk::Read] {
        let bytes: u64 = files
            .entries
            .iter()
            .filter(|e| e.mark.as_ref().is_some_and(|m| m.risk == risk))
            .map(|e| e.size)
            .sum();
        if bytes > 0 {
            segments.push((bytes, theme.risk(risk)));
        }
    }
    let unmarked: u64 = files
        .entries
        .iter()
        .filter(|e| e.mark.is_none())
        .map(|e| e.size)
        .sum();
    if unmarked > 0 {
        segments.push((unmarked, theme.text_faint));
    }

    let mut x = bar.x;
    for (bytes, colour) in &segments {
        let w = bar.w * (*bytes as f64 / total as f64);
        if w < 0.5 {
            continue;
        }
        c.fill_rounded(Rect::new(x, bar.y, w, bar.h), 2.5, *colour);
        x += w;
    }

    // One line naming the reclaimable part, which is the number worth reading.
    let reclaimable: u64 = files
        .entries
        .iter()
        .filter(|e| e.mark.is_some())
        .map(|e| e.size)
        .sum();
    let small = theme.small_font();
    let label = if reclaimable == 0 {
        format!("{} in this folder", nous_core::journal::human_bytes(total))
    } else {
        format!(
            "{} in this folder · {} could be freed",
            nous_core::journal::human_bytes(total),
            nous_core::journal::human_bytes(reclaimable)
        )
    };
    c.text(
        &label,
        s.x,
        bar.bottom() + 4.0,
        &small,
        theme.text_dim,
        Some(s.w),
    );
}

fn draw_tile(c: &Canvas, files: &mut Files, theme: &Theme, i: usize, tile: Rect, selected: bool) {
    // Copy out what drawing needs before borrowing the cache mutably.
    let (name, kind, is_dir, size, thumb, mark) = {
        let e = &files.entries[i];
        (
            e.name.clone(),
            e.kind(),
            e.is_dir,
            e.size,
            e.thumb.clone(),
            e.mark.clone(),
        )
    };

    let radius = Metrics::RADIUS_SMALL;
    let thumb_h = tile.h - CAPTION_H;
    c.fill_rounded(tile, radius, theme.surface);
    if selected {
        c.fill_rounded(tile, radius, theme.surface_active);
    }

    let thumb_rect = Rect::new(tile.x, tile.y, tile.w, thumb_h);
    let mut drew_picture = false;
    if let Some(path) = thumb.as_deref() {
        if let Some(pic) = files.picture(path) {
            // Only the top corners are round; the caption sits flush below.
            c.clip_rect(thumb_rect);
            c.picture_rounded(
                pic,
                Rect::new(tile.x, tile.y, tile.w, thumb_h + radius),
                radius,
            );
            c.restore();
            drew_picture = true;
        }
    }
    if !drew_picture {
        // No picture: the recessed well plus the file's kind, which is more
        // use than a generic icon and costs nothing to draw.
        c.fill_rounded(thumb_rect.inset(1.0), radius, theme.backdrop_opaque);
        let badge = theme.small_font();
        let (kw, kh) = c.measure(&kind, &badge, None);
        c.text(
            &kind,
            thumb_rect.x + (thumb_rect.w - kw) / 2.0,
            thumb_rect.y + (thumb_rect.h - kh) / 2.0,
            &badge,
            if is_dir {
                theme.voice
            } else {
                theme.text_faint
            },
            None,
        );
    }

    let pad = 10.0;
    let tx = tile.x + pad;
    let tw = tile.w - pad * 2.0;
    c.text(
        &name,
        tx,
        thumb_rect.bottom() + 7.0,
        &theme.font,
        theme.text,
        Some(tw),
    );

    let small = theme.small_font();
    match &mark {
        // A marked file says why in the colour of what would be done to it, in
        // place of its size. The size is not the interesting fact about a file
        // the system wants to move. The whole note is readable in the footer
        // when the tile is selected, so an ellipsis here loses nothing.
        Some(m) => {
            c.text(
                &m.note,
                tx,
                thumb_rect.bottom() + 25.0,
                &small,
                theme.risk(m.risk),
                Some(tw),
            );
            // A spine along the bottom, matching the plan's rows. Held clear of
            // the edge so the selection ring, which traces that edge, stays
            // unbroken underneath it.
            let bar = Rect::new(
                tile.x + radius,
                tile.bottom() - 5.0,
                tile.w - radius * 2.0,
                3.0,
            );
            c.fill_rounded(bar, 1.5, theme.risk(m.risk));
        }
        None => {
            let meta = if is_dir {
                String::new()
            } else {
                nous_core::journal::human_bytes(size)
            };
            if !meta.is_empty() {
                c.text(
                    &meta,
                    tx,
                    thumb_rect.bottom() + 25.0,
                    &small,
                    theme.text_dim,
                    Some(tw),
                );
            }
        }
    }

    // The ring last, so nothing drawn inside the tile crosses it.
    if selected {
        c.stroke_rounded(tile.inset(0.75), radius, 1.5, theme.voice);
    }
}

/// A hairline down the right edge showing how much of the folder is on screen.
/// Without it a row sliced by the bottom of the window reads as broken rather
/// than as "there is more below".
fn draw_scrollbar(c: &Canvas, files: &Files, theme: &Theme, layout: &Layout) {
    let track = Rect::new(
        layout.body.right() - SCROLLBAR_W,
        layout.body.y + 2.0,
        SCROLLBAR_W,
        (layout.body.h - 4.0).max(0.0),
    );
    c.fill_rounded(track, SCROLLBAR_W / 2.0, theme.surface);

    let visible = (layout.body.h / layout.content_height).clamp(0.05, 1.0);
    let max_scroll = (layout.content_height - layout.body.h).max(1.0);
    let at = (files.scroll / max_scroll).clamp(0.0, 1.0);
    let thumb_h = (track.h * visible).max(24.0);
    let thumb_y = track.y + (track.h - thumb_h) * at;
    c.fill_rounded(
        Rect::new(track.x, thumb_y, track.w, thumb_h),
        SCROLLBAR_W / 2.0,
        theme.text_faint,
    );
}

/// The footer says one of two things, and always says something: what the
/// system wants to do to this folder, or — when there is no plan — what the
/// file under the cursor actually is, at full width where a long note fits.
/// A folder with nothing in it.
///
/// An empty grid and an empty grid that failed to load look identical, and both
/// read as a program that has broken. Saying which it is costs one line.
fn draw_empty(c: &Canvas, theme: &Theme, body: Rect) {
    let msg = "This folder is empty";
    let f = theme.title_font();
    let (w, h) = c.measure(msg, &f, None);
    c.text(
        msg,
        body.x + (body.w - w) / 2.0,
        body.y + (body.h - h) / 2.0,
        &f,
        theme.text_faint,
        None,
    );
}

fn draw_footer(c: &Canvas, files: &Files, theme: &Theme, layout: &Layout) {
    let f = layout.footer;
    c.line(
        f.x,
        f.y + 0.5,
        f.right(),
        f.y + 0.5,
        Metrics::HAIRLINE,
        theme.hairline,
    );

    let small = theme.small_font();
    let (_, th) = c.measure("Ag", &theme.font, None);
    let ty = f.y + (f.h - th) / 2.0;

    match files.proposal.as_deref() {
        Some(text) => {
            let colour = files.peak_risk().map_or(theme.voice, |r| theme.risk(r));
            c.fill_rounded(
                Rect::new(f.x, f.y + 14.0, Metrics::ACCENT_BAR, f.h - 28.0),
                Metrics::ACCENT_BAR / 2.0,
                colour,
            );
            let hint = "enter  approve";
            let (hw, hh) = c.measure(hint, &small, None);
            let pill = Rect::new(
                f.right() - hw - 16.0,
                f.y + (f.h - hh - 8.0) / 2.0,
                hw + 16.0,
                hh + 8.0,
            );
            c.fill_rounded(pill, Metrics::RADIUS_SMALL, theme.surface_active);
            c.text(hint, pill.x + 8.0, pill.y + 4.0, &small, theme.text, None);

            let tx = f.x + Metrics::ACCENT_BAR + Metrics::GAP;
            c.text(
                text,
                tx,
                ty,
                &theme.font,
                theme.text,
                Some((pill.x - tx - Metrics::GAP).max(0.0)),
            );
        }
        None => {
            let Some(e) = files.selected_entry() else {
                return;
            };
            // Right-hand side first, so the name knows how much room it has.
            let meta = if e.is_dir {
                "folder".to_string()
            } else {
                nous_core::journal::human_bytes(e.size)
            };
            let (mw, mh) = c.measure(&meta, &small, None);
            c.text(
                &meta,
                f.right() - mw,
                f.y + (f.h - mh) / 2.0,
                &small,
                theme.text_dim,
                None,
            );

            let avail = (f.w - mw - Metrics::GAP * 2.0).max(0.0);
            match &e.mark {
                Some(m) => {
                    // The whole reason, in full, in the colour of the action.
                    c.fill_rounded(
                        Rect::new(f.x, f.y + 14.0, Metrics::ACCENT_BAR, f.h - 28.0),
                        Metrics::ACCENT_BAR / 2.0,
                        theme.risk(m.risk),
                    );
                    let tx = f.x + Metrics::ACCENT_BAR + Metrics::GAP;
                    let (nw, _) = c.measure(&e.name, &theme.font, None);
                    c.text(&e.name, tx, ty, &theme.font, theme.text, Some(avail));
                    let nx = tx + nw + Metrics::GAP;
                    c.text(
                        &m.note,
                        nx,
                        ty + 1.0,
                        &small,
                        theme.risk(m.risk),
                        Some((f.right() - mw - nx - Metrics::GAP).max(0.0)),
                    );
                }
                None => {
                    c.text(&e.name, f.x, ty, &theme.font, theme.text_dim, Some(avail));
                }
            }
        }
    }
}

fn diamond(c: &Canvas, cx: f64, cy: f64, r: f64, colour: Rgba, fill: bool) {
    let pts = [(cx, cy - r), (cx + r, cy), (cx, cy + r), (cx - r, cy)];
    for i in 0..4 {
        let (ax, ay) = pts[i];
        let (bx, by) = pts[(i + 1) % 4];
        if fill {
            c.line(cx, cy, (ax + bx) / 2.0, (ay + by) / 2.0, r * 1.5, colour);
        } else {
            c.line(ax, ay, bx, by, 1.4, colour);
        }
    }
}

/// The last component of a path, for the header. The full path is not the
/// thing worth the biggest type on screen.
fn folder_name(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
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
    use crate::draw::Image;

    fn file(name: &str, size: u64) -> Entry {
        Entry {
            name: name.into(),
            path: format!("/home/j/Downloads/{name}"),
            is_dir: false,
            size,
            modified: 0,
            thumb: None,
            blurb: None,
            mark: None,
        }
    }

    fn marked(name: &str, risk: Risk, note: &str) -> Entry {
        Entry {
            mark: Some(Mark {
                risk,
                note: note.into(),
            }),
            ..file(name, 1024)
        }
    }

    fn folder_of(n: usize) -> Files {
        Files::new(
            "/home/j/Downloads",
            (0..n)
                .map(|i| file(&format!("file{i}.jpg"), 2048))
                .collect(),
        )
    }

    #[test]
    fn the_grid_gets_more_columns_as_the_window_widens() {
        let f = folder_of(20);
        let narrow = Layout::compute(&f, 420.0, 600.0);
        let wide = Layout::compute(&f, 1400.0, 600.0);
        assert!(
            wide.columns > narrow.columns,
            "{} vs {}",
            wide.columns,
            narrow.columns
        );
        // Even a window too narrow for one tile lays out rather than dividing
        // by zero.
        let tiny = Layout::compute(&f, 80.0, 600.0);
        assert_eq!(tiny.columns, 1);
    }

    #[test]
    fn tiles_do_not_overlap_and_stay_inside_the_window() {
        let f = folder_of(24);
        let l = Layout::compute(&f, 900.0, 700.0);
        let mut seen: Vec<Rect> = Vec::new();
        for i in 0..f.entries.len() {
            let Some(t) = l.tile_for(i, 0.0) else {
                continue;
            };
            assert!(t.x >= l.body.x - 0.001, "tile {i} starts left of the body");
            assert!(
                t.right() <= l.body.right() + 0.001,
                "tile {i} runs past the right edge"
            );
            for (j, other) in seen.iter().enumerate() {
                let apart = t.right() <= other.x + 0.001
                    || t.x >= other.right() - 0.001
                    || t.bottom() <= other.y + 0.001
                    || t.y >= other.bottom() - 0.001;
                assert!(apart, "tile {i} overlaps tile {j}");
            }
            seen.push(t);
        }
        assert!(!seen.is_empty());
    }

    #[test]
    fn arrow_keys_walk_the_grid_the_way_it_looks() {
        let mut f = folder_of(10);
        let columns = 4;

        f.move_selection(1, 0, columns);
        assert_eq!(f.selected, 1, "right moves one");
        f.move_selection(0, 1, columns);
        assert_eq!(f.selected, 5, "down moves a whole row");
        f.move_selection(0, -1, columns);
        assert_eq!(f.selected, 1, "and up comes back");

        // Left at the start stays put rather than wrapping to the end.
        f.selected = 0;
        f.move_selection(-1, 0, columns);
        assert_eq!(f.selected, 0);
        // Right at the end likewise.
        f.selected = 9;
        f.move_selection(1, 0, columns);
        assert_eq!(f.selected, 9);
    }

    #[test]
    fn down_from_a_half_full_last_row_lands_on_the_last_file() {
        // 10 files in rows of 4: the last row holds 8 and 9. From 6, down
        // would be 10, which does not exist -- it should land on 9, not refuse.
        let mut f = folder_of(10);
        f.selected = 6;
        f.move_selection(0, 1, 4);
        assert_eq!(f.selected, 9);
        // And from the last row it stays put.
        f.move_selection(0, 1, 4);
        assert_eq!(f.selected, 9);
    }

    #[test]
    fn selecting_offscreen_scrolls_it_into_view() {
        let mut f = folder_of(60);
        let l = Layout::compute(&f, 900.0, 500.0);
        assert!(
            l.content_height > l.body.h,
            "the test needs a scrolling folder"
        );

        f.selected = 55;
        f.reveal(&l);
        let tile = l.tile_for(55, f.scroll).expect("visible after reveal");
        assert!(tile.y >= l.body.y - 0.001, "scrolled too far");
        assert!(
            tile.bottom() <= l.body.bottom() + 0.001,
            "still below the fold"
        );

        f.selected = 0;
        f.reveal(&l);
        assert_eq!(f.scroll, 0.0, "back at the top");
    }

    #[test]
    fn scrolling_stops_at_both_ends() {
        let mut f = folder_of(60);
        let l = Layout::compute(&f, 900.0, 500.0);
        f.scroll_by(-500.0, &l);
        assert_eq!(f.scroll, 0.0, "cannot scroll above the first row");
        f.scroll_by(100000.0, &l);
        assert!(
            (f.scroll - (l.content_height - l.body.h)).abs() < 0.001,
            "cannot scroll past the last row"
        );
    }

    #[test]
    fn a_short_folder_does_not_scroll_at_all() {
        let mut f = folder_of(3);
        let l = Layout::compute(&f, 900.0, 700.0);
        f.scroll_by(400.0, &l);
        assert_eq!(f.scroll, 0.0);
    }

    #[test]
    fn clicking_a_tile_selects_that_file_and_missing_it_selects_nothing() {
        let f = folder_of(12);
        let l = Layout::compute(&f, 900.0, 700.0);
        let t = l.tile_for(5, 0.0).unwrap();
        assert_eq!(f.hit(&l, t.x + 4.0, t.y + 4.0), Some(5));
        // The gap between tiles is not a tile.
        assert_eq!(f.hit(&l, t.right() + TILE_GAP / 2.0, t.y + 4.0), None);
        // Neither is the header.
        assert_eq!(f.hit(&l, t.x + 4.0, l.header.y + 2.0), None);
    }

    #[test]
    fn the_header_counts_what_needs_attention_not_just_what_is_there() {
        let mut f = folder_of(5);
        assert_eq!(f.marked(), 0);
        assert_eq!(f.peak_risk(), None);

        f.entries
            .push(marked("dupe.jpg", Risk::Write, "duplicate of file0.jpg"));
        f.entries
            .push(marked("old.log", Risk::Elevated, "untouched for 3 years"));
        assert_eq!(f.marked(), 2);
        // The header marker takes the worst of them, as a plan's does.
        assert_eq!(f.peak_risk(), Some(Risk::Elevated));
    }

    #[test]
    fn a_files_kind_comes_from_its_name_and_survives_odd_ones() {
        assert_eq!(file("holiday.jpg", 1).kind(), "JPG");
        assert_eq!(file("archive.tar.gz", 1).kind(), "GZ");
        assert_eq!(file("README", 1).kind(), "FILE");
        assert_eq!(
            file(".bashrc", 1).kind(),
            "FILE",
            "a dotfile is not a BASHRC"
        );
        assert_eq!(
            file("backup.20260826", 1).kind(),
            "FILE",
            "a long trailing number is not an extension"
        );
        let mut d = file("Pictures", 0);
        d.is_dir = true;
        assert_eq!(d.kind(), "FOLDER");
    }

    #[test]
    fn a_marked_file_is_drawn_in_the_colour_of_what_would_happen_to_it() {
        let theme = Theme::dark();
        let sample = |risk: Risk| {
            let mut f = Files::new(
                "/home/j/Downloads",
                vec![marked("x.jpg", risk, "duplicate")],
            );
            let img = Image::new(900, 520).unwrap();
            let c = img.canvas();
            let l = Layout::compute(&f, 900.0, 520.0);
            render(&c, &mut f, &theme, &l);
            let t = l.tile_for(0, 0.0).unwrap();
            assert!(
                t.bottom() <= l.body.bottom(),
                "the test window must fit a whole tile"
            );
            // The spine sits just inside the bottom edge, clear of the ring.
            img.pixel((t.x + t.w / 2.0) as i32, (t.bottom() - 4.0) as i32)
        };
        let write = sample(Risk::Write);
        let critical = sample(Risk::Critical);
        assert_ne!(write, critical, "risk is not distinguishable on a tile");
        assert!(
            critical.0 > critical.2,
            "a critical mark should be red: {critical:?}"
        );
        assert!(write.3 > 200, "the mark was not drawn at all");
    }

    #[test]
    fn an_unmarked_folder_draws_no_spines() {
        let theme = Theme::dark();
        let mut f = folder_of(4);
        let img = Image::new(900, 400).unwrap();
        let c = img.canvas();
        let l = Layout::compute(&f, 900.0, 400.0);
        render(&c, &mut f, &theme, &l);
        // Tile 1, not tile 0: tile 0 is selected, and the selection ring traces
        // the same edge the spine would.
        assert!(
            l.columns > 1,
            "the test needs a second tile on the first row"
        );
        let t = l.tile_for(1, 0.0).unwrap();
        let px = img.pixel((t.x + t.w / 2.0) as i32, (t.bottom() - 4.0) as i32);
        assert!(
            px.0 < 80 && px.1 < 80 && px.2 < 90,
            "an unmarked file was given a mark: {px:?}"
        );
    }

    #[test]
    fn a_spine_survives_the_selection_ring_on_the_same_tile() {
        // Both draw at the bottom edge of a tile. The mark is the more
        // important of the two and must not be hidden by the highlight.
        let theme = Theme::dark();
        let mut f = Files::new(
            "/home/j/Downloads",
            vec![marked("x.jpg", Risk::Critical, "duplicate")],
        );
        f.selected = 0;
        let img = Image::new(900, 520).unwrap();
        let c = img.canvas();
        let l = Layout::compute(&f, 900.0, 520.0);
        render(&c, &mut f, &theme, &l);
        let t = l.tile_for(0, 0.0).unwrap();
        let px = img.pixel((t.x + t.w / 2.0) as i32, (t.bottom() - 4.0) as i32);
        assert!(
            px.0 > 180 && px.2 < 140,
            "the mark was covered by the highlight: {px:?}"
        );
    }

    #[test]
    fn the_folder_name_is_shown_not_the_whole_path() {
        assert_eq!(folder_name("/home/joey/Downloads"), "Downloads");
        assert_eq!(folder_name("/home/joey/Downloads/"), "Downloads");
        assert_eq!(folder_name("/"), "/");
        assert_eq!(folder_name(""), "/");
        assert_eq!(folder_name("Downloads"), "Downloads");
    }

    #[test]
    fn an_empty_folder_says_so_rather_than_looking_broken() {
        // An empty grid and a grid that failed to load are the same picture,
        // and both read as a program that has stopped working.
        let theme = Theme::dark();
        let mut f = Files::new("/home/j/Downloads", Vec::new());
        let img = Image::new(900, 600).unwrap();
        let l = Layout::compute(&f, 900.0, 600.0);
        render(&img.canvas(), &mut f, &theme, &l);
        assert!(
            img.variety(l.body) > 2,
            "an empty folder draws nothing at all"
        );
        // The header still names where you are, which is how you get back out.
        // The footer stays blank on purpose: it reports the selected file, and
        // an empty folder has none to report.
        assert!(img.variety(l.header) > 2, "no header on an empty folder");
    }

    #[test]
    fn several_files_can_be_chosen_at_once() {
        // Without this you can copy one file at a time, which is not a file
        // manager.
        let mut f = Files::new(
            "/d",
            (0..8).map(|i| file(&format!("f{i}.txt"), 1)).collect(),
        );
        f.choose_only(2);
        assert_eq!(f.chosen(), vec![2]);
        f.toggle(5);
        assert_eq!(
            f.chosen(),
            vec![2, 5],
            "ctrl-click did not add to the choice"
        );
        f.toggle(7);
        assert_eq!(f.chosen(), vec![2, 5, 7]);
        // Ctrl-clicking a chosen one takes it away again.
        f.toggle(5);
        assert_eq!(f.chosen(), vec![2, 7]);
        // And the result is always in folder order, whatever order it was
        // clicked in, so an action on it happens in an order you can predict.
        f.choose_only(6);
        f.toggle(1);
        f.toggle(4);
        assert_eq!(f.chosen(), vec![1, 4, 6]);
    }

    #[test]
    fn shift_takes_everything_in_between() {
        let mut f = Files::new(
            "/d",
            (0..10).map(|i| file(&format!("f{i}.txt"), 1)).collect(),
        );
        f.choose_only(2);
        f.extend_to(6);
        assert_eq!(f.chosen(), vec![2, 3, 4, 5, 6]);
        // Backwards too, and it replaces rather than accumulating.
        f.choose_only(8);
        f.extend_to(5);
        assert_eq!(f.chosen(), vec![5, 6, 7, 8]);
    }

    #[test]
    fn moving_the_keyboard_forgets_a_stale_choice() {
        // Six files chosen, then an arrow key, then Delete: without this that
        // deletes six files when you meant one.
        let mut f = Files::new(
            "/d",
            (0..8).map(|i| file(&format!("f{i}.txt"), 1)).collect(),
        );
        f.choose_only(0);
        f.extend_to(5);
        assert_eq!(f.chosen().len(), 6);
        f.move_selection(1, 0, 4);
        assert_eq!(
            f.chosen().len(),
            1,
            "an arrow key kept the old choice: {:?}",
            f.chosen()
        );
    }

    #[test]
    fn un_choosing_where_the_keyboard_is_leaves_it_somewhere_real() {
        let mut f = Files::new(
            "/d",
            (0..5).map(|i| file(&format!("f{i}.txt"), 1)).collect(),
        );
        f.choose_only(1);
        f.toggle(3);
        assert_eq!(f.selected, 3);
        f.toggle(3);
        assert!(
            f.chosen().contains(&f.selected),
            "the keyboard is on nothing"
        );
        assert_eq!(f.chosen(), vec![1]);
    }

    #[test]
    fn choosing_everything_and_nothing() {
        let mut f = Files::new(
            "/d",
            (0..6).map(|i| file(&format!("f{i}.txt"), 1)).collect(),
        );
        f.choose_all();
        assert_eq!(f.chosen_count(), 6);
        f.choose_none();
        assert_eq!(f.chosen(), vec![f.selected]);
        // An empty folder has nothing to choose and does not panic saying so.
        let mut empty = Files::new("/d", Vec::new());
        empty.choose_all();
        assert_eq!(empty.chosen_count(), 0);
        empty.toggle(3);
        empty.extend_to(9);
        assert_eq!(empty.chosen_count(), 0);
    }

    #[test]
    fn a_folder_renders_its_files_rather_than_an_empty_box() {
        let theme = Theme::dark();
        let mut f = folder_of(6);
        let img = Image::new(900, 500).unwrap();
        let c = img.canvas();
        let l = Layout::compute(&f, 900.0, 500.0);
        render(&c, &mut f, &theme, &l);
        // The tile area is not a flat fill: it has surfaces, names and sizes.
        let t = l.tile_for(0, 0.0).unwrap();
        assert!(img.variety(t) > 6, "the tile is blank");
        assert!(img.variety(l.header) > 4, "the header is blank");
    }

    #[test]
    fn the_footer_is_always_there_so_the_grid_never_reflows_under_you() {
        // It shows a pending plan when there is one and the selected file
        // otherwise. A strip that appears and disappears would shift every tile
        // on screen as the selection moved.
        let mut f = folder_of(4);
        let bare = Layout::compute(&f, 900.0, 500.0);
        assert!(bare.footer.h > 0.0);

        f.proposal = Some("84 images into Pictures/2026".into());
        let with = Layout::compute(&f, 900.0, 500.0);
        assert_eq!(
            with.footer.h, bare.footer.h,
            "the footer must not change height"
        );
        assert_eq!(with.body.h, bare.body.h, "so the grid must not move");
        assert!(
            with.body.bottom() <= with.footer.y + 0.001,
            "the grid runs under the footer"
        );
    }

    #[test]
    fn a_narrow_window_does_not_stretch_one_tile_into_a_poster() {
        let f = folder_of(6);
        let narrow = Layout::compute(&f, 420.0, 600.0);
        assert_eq!(narrow.columns, 1);
        assert!(
            narrow.tile_w <= TILE_TARGET_W * 1.36,
            "one tile ballooned to {}",
            narrow.tile_w
        );
        // And at a normal width the grid still spans the content exactly.
        let wide = Layout::compute(&f, 900.0, 600.0);
        let span = wide.columns as f64 * wide.tile_w + (wide.columns - 1) as f64 * TILE_GAP;
        assert!(
            (span - wide.body.w).abs() < 0.5,
            "grid spans {span} of {}",
            wide.body.w
        );
    }
}
