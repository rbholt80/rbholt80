//! Looking at a file without leaving.
//!
//! Opening something used to mean handing it to another program and losing
//! the window: you came back to find the folder where you left it and nothing
//! you had learned. That is the arrangement every desktop has, and it is why
//! looking through a folder of photographs means opening one, closing it,
//! opening the next.
//!
//! Here, opening stays. Arrow keys walk the folder — the *same* folder, in the
//! same order it is drawn in — and everything the file view can do to a file
//! still works while you are looking at it: rename it, throw it away, ask
//! about it. Handing off to another program remains one keystroke away, for
//! the things this cannot show.

use nous_ui::draw::{Canvas, Picture, Rect};
use nous_ui::files::Entry;
use nous_ui::theme::{Metrics, Theme};

/// What is open, and what is known about it.
pub struct Viewer {
    /// Which entry, by its place in the folder.
    pub index: usize,
    /// How far in it is zoomed. 1.0 fits the window.
    pub zoom: f64,
    /// What could not be shown, and why.
    pub trouble: Option<String>,
    loaded: Option<(String, Option<Picture>)>,
}

/// How far the zoom may be pushed either way.
const MIN_ZOOM: f64 = 1.0;
const MAX_ZOOM: f64 = 8.0;

impl Viewer {
    pub fn open(index: usize) -> Viewer {
        Viewer {
            index,
            zoom: 1.0,
            trouble: None,
            loaded: None,
        }
    }

    /// Move to another file, keeping the zoom sensible.
    pub fn go_to(&mut self, index: usize) {
        self.index = index;
        // Reset: a zoom set for one picture means nothing on the next, and
        // arriving already magnified into a corner is disorienting.
        self.zoom = 1.0;
        self.trouble = None;
    }

    pub fn zoom_by(&mut self, factor: f64) {
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
    }

    pub fn reset_zoom(&mut self) {
        self.zoom = 1.0;
    }

    /// The picture for `path`, loaded once.
    fn picture(&mut self, path: &str) -> Option<&Picture> {
        let stale = self.loaded.as_ref().map(|(p, _)| p != path).unwrap_or(true);
        if stale {
            self.loaded = Some((path.to_string(), Picture::load(path).ok()));
        }
        self.loaded.as_ref().and_then(|(_, p)| p.as_ref())
    }
}

/// Which files the viewer will step through.
///
/// Only the ones it can show. Stepping onto a spreadsheet and displaying "no
/// preview" is a worse answer than skipping it, because the arrow key is for
/// looking at things.
pub fn viewable(entries: &[Entry]) -> Vec<usize> {
    entries
        .iter()
        .enumerate()
        .filter(|(_, e)| !e.is_dir && e.thumb.is_some())
        .map(|(i, _)| i)
        .collect()
}

/// The next or previous viewable file after `from`, wrapping round.
///
/// Wrapping because a folder of photographs has no first and last worth
/// enforcing, and stopping dead at the end of a set someone is flicking
/// through is an interruption with nothing behind it.
pub fn step(order: &[usize], from: usize, delta: i64) -> Option<usize> {
    if order.is_empty() {
        return None;
    }
    let at = order.iter().position(|i| *i == from).unwrap_or(0) as i64;
    let n = order.len() as i64;
    let next = ((at + delta) % n + n) % n;
    Some(order[next as usize])
}

// --- layout ---------------------------------------------------------------

const BAR_H: f64 = 44.0;

pub struct Layout {
    /// The whole area, kept so a caller can test a click against it before
    /// working out which part of the viewer was hit.
    #[allow(dead_code)]
    pub panel: Rect,
    /// Where the picture goes.
    pub stage: Rect,
    /// The strip along the bottom: what this is, and what can be done to it.
    pub bar: Rect,
}

impl Layout {
    pub fn compute(area: Rect) -> Layout {
        let bar_h = BAR_H.min(area.h);
        Layout {
            panel: area,
            stage: Rect::new(area.x, area.y, area.w, (area.h - bar_h).max(0.0)),
            bar: Rect::new(area.x, area.bottom() - bar_h, area.w, bar_h),
        }
    }
}

// --- drawing --------------------------------------------------------------

pub fn render(c: &Canvas, v: &mut Viewer, entries: &[Entry], theme: &Theme, layout: &Layout) {
    // Black behind a picture, whatever the theme: a photograph is judged
    // against black, and a pale surround changes what it looks like.
    c.fill_rect(layout.stage, nous_ui::draw::Rgba::rgb(0, 0, 0));

    let Some(e) = entries.get(v.index).cloned() else {
        return;
    };
    let zoom = v.zoom;
    let stage = layout.stage;

    let mut drew = false;
    if let Some(path) = e.thumb.clone() {
        if let Some(pic) = v.picture(&path) {
            // Fitted, then magnified about the middle. Beyond the edges is
            // clipped rather than letting the picture escape the stage.
            let fit = pic.contain(stage);
            let into = Rect::new(
                stage.x + (stage.w - fit.w * zoom) / 2.0,
                stage.y + (stage.h - fit.h * zoom) / 2.0,
                fit.w * zoom,
                fit.h * zoom,
            );
            c.clip_rect(stage);
            c.picture(pic, into);
            c.restore();
            drew = true;
        }
    }
    if !drew {
        let msg = v
            .trouble
            .clone()
            .unwrap_or_else(|| "nothing to show for this one".to_string());
        let f = theme.title_font();
        let (w, h) = c.measure(&msg, &f, None);
        c.text(
            &msg,
            stage.x + (stage.w - w) / 2.0,
            stage.y + (stage.h - h) / 2.0,
            &f,
            theme.text_dim,
            None,
        );
    }

    // The bar: what it is on the left, what the keys do on the right.
    let bar = layout.bar;
    c.fill_rect(bar, theme.backdrop_opaque);
    c.line(
        bar.x,
        bar.y + 0.5,
        bar.right(),
        bar.y + 0.5,
        1.0,
        theme.hairline,
    );
    let body = theme.body_font();
    let small = theme.small_font();
    let cy = bar.y + bar.h / 2.0;

    let (_, nh) = c.measure(&e.name, &body, None);
    c.text(
        &e.name,
        bar.x + Metrics::PAD,
        cy - nh / 2.0,
        &body,
        theme.text,
        Some(bar.w * 0.45),
    );

    let mut note = match &e.mark {
        Some(m) => m.note.clone(),
        None => e.kind(),
    };
    if zoom > 1.001 {
        note = format!("{note} · {:.0}%", zoom * 100.0);
    }
    let (mw, mh) = c.measure(&note, &small, None);
    c.text(
        &note,
        bar.x + bar.w * 0.5 - mw / 2.0,
        cy - mh / 2.0,
        &small,
        match &e.mark {
            Some(m) => theme.risk(m.risk),
            None => theme.text_faint,
        },
        None,
    );

    // Always both ways out, and always said.
    let keys = "← → · + − · o opens it · Esc back";
    let (kw, kh) = c.measure(keys, &small, None);
    c.text(
        keys,
        bar.right() - kw - Metrics::PAD,
        cy - kh / 2.0,
        &small,
        theme.text_faint,
        None,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use nous_ui::draw::Image;

    fn entry(name: &str, thumb: bool) -> Entry {
        Entry {
            name: name.into(),
            path: format!("/d/{name}"),
            is_dir: false,
            size: 1000,
            modified: 0,
            thumb: thumb.then(|| format!("/thumbs/{name}.png")),
            blurb: None,
            mark: None,
        }
    }

    #[test]
    fn only_files_it_can_show_are_stepped_through() {
        // Landing on a spreadsheet and saying "no preview" is a worse answer
        // than not landing on it: the arrow key is for looking at things.
        let entries = vec![
            entry("a.jpg", true),
            entry("notes.txt", false),
            entry("b.png", true),
            Entry {
                is_dir: true,
                ..entry("Folder", true)
            },
        ];
        assert_eq!(
            viewable(&entries),
            vec![0, 2],
            "stepped onto something unshowable"
        );
    }

    #[test]
    fn stepping_wraps_rather_than_stopping_dead() {
        // A folder of photographs has no first and last worth enforcing.
        let order = vec![0usize, 2, 5];
        assert_eq!(step(&order, 0, 1), Some(2));
        assert_eq!(step(&order, 5, 1), Some(0), "stopped at the end");
        assert_eq!(step(&order, 0, -1), Some(5), "stopped at the start");
        assert_eq!(step(&[], 0, 1), None);
    }

    #[test]
    fn stepping_from_a_file_that_cannot_be_shown_still_goes_somewhere() {
        // The viewer can be opened on anything; walking on from it must work.
        let order = vec![2usize, 4];
        assert_eq!(step(&order, 99, 1), Some(4));
    }

    #[test]
    fn zoom_is_bounded_at_both_ends() {
        let mut v = Viewer::open(0);
        assert_eq!(v.zoom, 1.0);
        for _ in 0..40 {
            v.zoom_by(1.5);
        }
        assert_eq!(v.zoom, MAX_ZOOM, "zoomed past the limit");
        for _ in 0..40 {
            v.zoom_by(0.5);
        }
        assert_eq!(v.zoom, MIN_ZOOM, "zoomed out past fitting the window");
    }

    #[test]
    fn moving_to_another_file_starts_it_fitting_the_window() {
        // A zoom set for one picture means nothing on the next, and arriving
        // magnified into a corner of it is disorienting.
        let mut v = Viewer::open(0);
        v.zoom_by(4.0);
        v.trouble = Some("something".into());
        v.go_to(3);
        assert_eq!(v.index, 3);
        assert_eq!(v.zoom, 1.0);
        assert!(
            v.trouble.is_none(),
            "carried a complaint to a different file"
        );
    }

    #[test]
    fn the_bar_leaves_the_picture_the_rest_of_the_window() {
        let l = Layout::compute(Rect::new(0.0, 0.0, 900.0, 600.0));
        assert!(l.stage.h > 500.0);
        assert!(
            l.bar.y >= l.stage.bottom() - 0.001,
            "the bar covers the picture"
        );
        assert!(l.bar.bottom() <= 600.0 + 0.001);
        // And a window too short for a bar does not produce a negative stage.
        let tiny = Layout::compute(Rect::new(0.0, 0.0, 400.0, 20.0));
        assert!(tiny.stage.h >= 0.0);
    }

    #[test]
    fn a_file_with_nothing_to_show_says_so_rather_than_going_black() {
        let theme = Theme::dark();
        let entries = vec![entry("mystery.bin", false)];
        let mut v = Viewer::open(0);
        let area = Rect::new(0.0, 0.0, 900.0, 600.0);
        let l = Layout::compute(area);
        let img = Image::new(900, 600).unwrap();
        render(&img.canvas(), &mut v, &entries, &theme, &l);
        assert!(
            img.variety(l.stage) > 2,
            "an unshowable file is a black rectangle"
        );
        assert!(img.variety(l.bar) > 3, "no bar");
    }

    #[test]
    fn the_bar_names_the_file_and_says_how_to_get_out() {
        let theme = Theme::dark();
        let entries = vec![entry("holiday.jpg", false)];
        let mut v = Viewer::open(0);
        let l = Layout::compute(Rect::new(0.0, 0.0, 900.0, 600.0));
        let img = Image::new(900, 600).unwrap();
        render(&img.canvas(), &mut v, &entries, &theme, &l);
        // Something is written at both ends of the bar.
        let left = Rect::new(l.bar.x, l.bar.y, 200.0, l.bar.h);
        let right = Rect::new(l.bar.right() - 260.0, l.bar.y, 260.0, l.bar.h);
        assert!(img.variety(left) > 3, "the file is not named");
        assert!(img.variety(right) > 3, "no way out is offered");
    }
}
