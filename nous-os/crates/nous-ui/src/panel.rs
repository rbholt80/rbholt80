//! The summon panel: what the shell actually looks like.
//!
//! The panel is one prompt line and, underneath it, whatever the system has to
//! say back. There is no title bar, no toolbar and no buttons. Everything is
//! reachable from the keyboard and the available keys are always written along
//! the bottom, so nothing has to be discovered by hovering.
//!
//! The one piece of ornament is the **risk spine**: a coloured bar down the
//! left of every step in a proposal, in the colour of that step's risk. A plan
//! that only reads files is a column of blue; one that deletes something has a
//! red mark in it that is visible before any of the text is read. That is the
//! whole visual identity, and it is carrying information rather than decorating.
//!
//! Layout is computed into [`Layout`] as plain rectangles before anything is
//! drawn, so the arrangement can be tested without a display and the drawing
//! code stays a straight translation of it.

use crate::draw::{Canvas, Rect, Rgba};
use crate::input::Edit;
use crate::theme::{Metrics, Risk, Theme};

/// One action inside a proposal.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub capability: String,
    pub summary: String,
    pub risk: Risk,
}

/// What the panel is currently showing beneath the prompt.
#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    /// Nothing yet. The panel is just a prompt.
    Empty,
    /// A request is in flight. `note` says what is being waited on, because a
    /// spinner that does not say what it is doing is not an answer.
    Working {
        note: String,
    },
    /// Prose from an assistant or from the system.
    Answer {
        text: String,
        source: String,
    },
    /// A plan awaiting approval.
    Proposal {
        headline: String,
        steps: Vec<Step>,
    },
    /// A plan that was carried out. `headline` says what happened; `detail`
    /// is whatever it produced, which is content rather than status and is
    /// therefore not coloured as one.
    Done {
        headline: String,
        detail: String,
        undo_hint: bool,
    },
    Error {
        message: String,
    },
}

impl Body {
    /// The risk of the most dangerous step, if this is a proposal. Drives the
    /// colour of the prompt marker so the panel's own indicator says how
    /// serious the pending plan is.
    pub fn peak_risk(&self) -> Option<Risk> {
        let Body::Proposal { steps, .. } = self else {
            return None;
        };
        // Risk is Ord, ascending in severity, so the maximum is the worst.
        steps.iter().map(|s| s.risk).max()
    }
}

pub struct Panel {
    pub input: Edit,
    pub body: Body,
    /// Which step is highlighted, for a proposal.
    pub selected: usize,
    /// First visible step. Only nonzero when the list is taller than the panel.
    pub scroll: usize,
    /// Advances on every frame while working, to animate the marker.
    pub phase: f64,
    /// Horizontal scroll of the prompt text, in pixels, when it is longer than
    /// the line.
    pub prompt_offset: f64,
    /// What was attached to this request by the file manager or the caller,
    /// described in a few words. Shown so an instruction like "tidy these" can
    /// be read as a whole.
    pub context: Option<String>,
}

impl Default for Panel {
    fn default() -> Self {
        Panel::new()
    }
}

impl Panel {
    pub fn new() -> Panel {
        Panel {
            input: Edit::new(),
            body: Body::Empty,
            selected: 0,
            scroll: 0,
            phase: 0.0,
            prompt_offset: 0.0,
            context: None,
        }
    }

    pub fn set_body(&mut self, body: Body) {
        self.body = body;
        self.selected = 0;
        self.scroll = 0;
    }

    pub fn steps(&self) -> &[Step] {
        match &self.body {
            Body::Proposal { steps, .. } => steps,
            _ => &[],
        }
    }

    /// Move the highlight, keeping it inside the list and pulling the scroll
    /// along with it.
    pub fn move_selection(&mut self, delta: i32, visible: usize) {
        let n = self.steps().len();
        if n == 0 {
            return;
        }
        let next = (self.selected as i32 + delta).clamp(0, n as i32 - 1) as usize;
        self.selected = next;
        let visible = visible.max(1);
        if next < self.scroll {
            self.scroll = next;
        } else if next >= self.scroll + visible {
            self.scroll = next + 1 - visible;
        }
        // A list that shrank under a scrolled view must not leave blank space
        // at the bottom.
        self.scroll = self.scroll.min(n.saturating_sub(visible));
    }

    /// The keys the panel currently responds to, in the order they are shown.
    /// Written out rather than left to be discovered.
    pub fn hints(&self) -> Vec<(&'static str, &'static str)> {
        match &self.body {
            Body::Proposal { .. } => vec![
                ("enter", "approve"),
                ("esc", "discard"),
                ("up down", "review"),
            ],
            // Not "cancel": the request is already with the daemon and will
            // finish there. This gives the panel back and discards the answer.
            Body::Working { .. } => vec![("esc", "stop waiting")],
            Body::Done {
                undo_hint: true, ..
            } => {
                vec![("ctrl z", "undo"), ("esc", "close")]
            }
            Body::Empty => vec![("enter", "ask"), ("esc", "close")],
            _ => vec![("esc", "close")],
        }
    }
}

// --- layout ---------------------------------------------------------------

/// Where everything goes. Computed before drawing so it can be asserted on.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub panel: Rect,
    /// The prompt line, including its padding.
    pub prompt: Rect,
    /// Just the editable text area within the prompt.
    pub prompt_text: Rect,
    /// The state marker at the left of the prompt.
    pub marker: Rect,
    /// The chip naming what the file manager attached, if anything.
    pub context: Option<Rect>,
    /// The area holding the body, empty when there is no body.
    pub body: Rect,
    /// One rect per visible step, in order. Empty unless showing a proposal.
    pub rows: Vec<Rect>,
    /// Index of the first step in `rows`.
    pub first_row: usize,
    pub footer: Rect,
    /// The full height the panel wants to be.
    pub height: f64,
}

/// The height the body needs, given the width available for wrapping text.
///
/// Measured through the same canvas that will draw it, with the same wrapping.
/// Measuring one way and drawing the other is how a body ends up clipped, so
/// there is deliberately no way to pass a different measurer.
fn body_height(body: &Body, width: f64, c: &Canvas, theme: &Theme) -> f64 {
    let h = |s: &str, font: &crate::draw::Font| c.measure_wrapped(s, font, width).1;
    match body {
        Body::Empty => 0.0,
        Body::Working { note } => h(note, &theme.font).max(20.0) + Metrics::GAP,
        Body::Answer { text, .. } => {
            // Source line plus the prose itself.
            22.0 + h(text, &theme.font) + Metrics::GAP
        }
        Body::Proposal { headline, steps } => {
            proposal_head(headline, width, c, theme)
                + steps.len() as f64 * Metrics::ROW_HEIGHT
                + Metrics::GAP
        }
        Body::Done {
            headline, detail, ..
        } => {
            let mut n = h(headline, &theme.font);
            if !detail.is_empty() {
                n += Metrics::UNIT + h(detail, &theme.font);
            }
            n + Metrics::GAP
        }
        Body::Error { message } => h(message, &theme.font) + Metrics::GAP,
    }
}

/// Height of the "PROPOSED" label and the headline above the step list.
fn proposal_head(headline: &str, width: f64, c: &Canvas, theme: &Theme) -> f64 {
    18.0 + c.measure_wrapped(headline, &theme.title_font(), width).1 + Metrics::UNIT
}

impl Layout {
    /// Lay the panel out for a given width.
    ///
    /// The panel is exactly as tall as its content up to
    /// [`Metrics::PANEL_MAX_HEIGHT`], past which the step list scrolls. A panel
    /// that is always the same height wastes the screen when it has one line to
    /// show and truncates when it has twenty.
    pub fn compute(panel: &Panel, width: f64, c: &Canvas, theme: &Theme) -> Layout {
        let pad = Metrics::PAD;
        let inner_w = (width - pad * 2.0).max(0.0);
        let marker_size = 12.0;
        // The prompt text starts clear of the marker and its gap.
        let text_x = pad + marker_size + Metrics::GAP;

        // What the file manager attached sits at the right of the prompt line,
        // and the text yields the space rather than running underneath it.
        // "tidy these" is a blind instruction if the panel does not say what
        // "these" is.
        let small = theme.small_font();
        let context = panel.context.as_ref().map(|label| {
            let (tw, th) = c.measure(label, &small, Some(inner_w / 2.0));
            Rect::new(
                width - pad - (tw + 16.0),
                (Metrics::PROMPT_HEIGHT - (th + 8.0)) / 2.0,
                tw + 16.0,
                th + 8.0,
            )
        });
        let text_right = match &context {
            Some(chip) => chip.x - Metrics::GAP,
            None => width - pad,
        };
        let text_w = (text_right - text_x).max(0.0);

        let prompt = Rect::new(0.0, 0.0, width, Metrics::PROMPT_HEIGHT);
        let prompt_text = Rect::new(text_x, 0.0, text_w, Metrics::PROMPT_HEIGHT);
        let marker = Rect::new(
            pad,
            (Metrics::PROMPT_HEIGHT - marker_size) / 2.0,
            marker_size,
            marker_size,
        );

        let has_body = panel.body != Body::Empty;
        let footer_h = if has_body { 30.0 } else { 0.0 };

        let wanted_body = if has_body {
            body_height(&panel.body, inner_w, c, theme)
        } else {
            0.0
        };

        // Reserve room for the fixed parts first; whatever is left is the most
        // the body may occupy.
        let max_body =
            (Metrics::PANEL_MAX_HEIGHT - Metrics::PROMPT_HEIGHT - footer_h - pad).max(0.0);
        let body_h = wanted_body.min(max_body);

        let body_rect = Rect::new(pad, Metrics::PROMPT_HEIGHT, inner_w, body_h);

        let mut rows = Vec::new();
        let mut first_row = panel.scroll;
        if let Body::Proposal { headline, steps } = &panel.body {
            let list_top = body_rect.y + proposal_head(headline, inner_w, c, theme);
            let list_h = (body_rect.bottom() - list_top).max(0.0);
            let visible = (list_h / Metrics::ROW_HEIGHT).floor().max(0.0) as usize;
            first_row = panel.scroll.min(steps.len().saturating_sub(visible.max(1)));
            for i in 0..visible.min(steps.len().saturating_sub(first_row)) {
                rows.push(Rect::new(
                    body_rect.x,
                    list_top + i as f64 * Metrics::ROW_HEIGHT,
                    inner_w,
                    Metrics::ROW_HEIGHT,
                ));
            }
        }

        let footer = Rect::new(pad, body_rect.bottom(), inner_w, footer_h);
        let height = Metrics::PROMPT_HEIGHT + body_h + footer_h + if has_body { pad } else { 0.0 };

        Layout {
            panel: Rect::new(0.0, 0.0, width, height),
            prompt,
            prompt_text,
            marker,
            context,
            body: body_rect,
            rows,
            first_row,
            footer,
            height,
        }
    }

    /// How many steps fit in the list at this layout. Used to page the
    /// selection by the same amount the eye can see.
    pub fn visible_rows(&self) -> usize {
        self.rows.len()
    }
}

// --- drawing --------------------------------------------------------------

/// Draw the whole panel. `focused` dims the caret when the panel has lost the
/// keyboard, which is the only way to tell from a screenshot whether typing
/// would go here.
pub fn render(c: &Canvas, panel: &Panel, theme: &Theme, layout: &Layout, focused: bool) {
    let r = Rect::new(0.0, 0.0, layout.panel.w, layout.height);
    c.fill_rounded(r, Metrics::RADIUS, theme.backdrop);
    c.stroke_rounded(
        r.inset(0.5),
        Metrics::RADIUS,
        Metrics::HAIRLINE,
        theme.hairline,
    );

    draw_marker(c, panel, theme, layout);
    draw_prompt(c, panel, theme, layout, focused);
    if let (Some(chip), Some(label)) = (layout.context, panel.context.as_ref()) {
        c.fill_rounded(chip, Metrics::RADIUS_SMALL, theme.surface_active);
        c.text(
            label,
            chip.x + 8.0,
            chip.y + 4.0,
            &theme.small_font(),
            theme.text_dim,
            Some(chip.w - 16.0),
        );
    }

    if panel.body != Body::Empty {
        // A hairline under the prompt, rather than a heavier divider: the
        // prompt and its answer are one thing, not two panes.
        c.line(
            Metrics::PAD,
            Metrics::PROMPT_HEIGHT - 0.5,
            layout.panel.w - Metrics::PAD,
            Metrics::PROMPT_HEIGHT - 0.5,
            Metrics::HAIRLINE,
            theme.hairline,
        );
        c.clip_rect(layout.body);
        draw_body(c, panel, theme, layout);
        c.restore();
        draw_footer(c, panel, theme, layout);
    }
}

/// The state marker: a diamond. Hollow when waiting for input, filled and
/// pulsing while working, and in the risk colour when a plan is pending.
fn draw_marker(c: &Canvas, panel: &Panel, theme: &Theme, layout: &Layout) {
    let m = layout.marker;
    let (cx, cy) = (m.x + m.w / 2.0, m.y + m.h / 2.0);
    let (colour, fill, scale) = match &panel.body {
        Body::Working { .. } => {
            // Breathes between 0.7 and 1.0 so it is clearly alive without
            // spinning like every other loading indicator.
            let t = (panel.phase * 2.0).sin() * 0.5 + 0.5;
            (theme.voice, true, 0.7 + 0.3 * t)
        }
        Body::Error { .. } => (theme.danger, true, 1.0),
        Body::Done { .. } => (theme.ok, true, 1.0),
        _ => match panel.body.peak_risk() {
            Some(risk) => (theme.risk(risk), true, 1.0),
            None => (theme.text_faint, false, 1.0),
        },
    };
    diamond(c, cx, cy, m.w / 2.0 * scale, colour, fill);
}

fn diamond(c: &Canvas, cx: f64, cy: f64, r: f64, colour: Rgba, fill: bool) {
    let pts = [(cx, cy - r), (cx + r, cy), (cx, cy + r), (cx - r, cy)];
    if fill {
        // Four triangles from the centre: no polygon primitive is needed for a
        // shape this simple, and it keeps Canvas small.
        for i in 0..4 {
            let (ax, ay) = pts[i];
            let (bx, by) = pts[(i + 1) % 4];
            c.line(cx, cy, (ax + bx) / 2.0, (ay + by) / 2.0, r * 1.5, colour);
        }
    } else {
        for i in 0..4 {
            let (ax, ay) = pts[i];
            let (bx, by) = pts[(i + 1) % 4];
            c.line(ax, ay, bx, by, 1.4, colour);
        }
    }
}

fn draw_prompt(c: &Canvas, panel: &Panel, theme: &Theme, layout: &Layout, focused: bool) {
    let font = theme.prompt_font();
    let area = layout.prompt_text;
    let text = panel.input.text();
    let (_, th) = c.measure("Ag", &font, None);
    let ty = area.y + (area.h - th) / 2.0;

    c.clip_rect(area);
    if text.is_empty() {
        c.text(
            "ask, or say what you want done",
            area.x,
            ty,
            &font,
            theme.text_faint,
            None,
        );
    } else {
        let caret_x = c.measure(&text[..panel.input.caret()], &font, None).0;
        // Keep the caret on screen by sliding the text left once it would run
        // past the end of the line.
        let shift = (caret_x - area.w + 2.0).max(0.0);
        let x = area.x - shift;

        if let Some((s, e)) = panel.input.selection() {
            let sx = c.measure(&text[..s], &font, None).0;
            let ex = c.measure(&text[..e], &font, None).0;
            c.fill_rect(
                Rect::new(x + sx, ty - 2.0, ex - sx, th + 4.0),
                theme.voice.with_alpha(0.25),
            );
        }
        c.text(text, x, ty, &font, theme.text, None);
        if focused {
            c.fill_rect(Rect::new(x + caret_x, ty - 1.0, 2.0, th + 2.0), theme.voice);
        }
    }
    c.restore();
}

fn draw_body(c: &Canvas, panel: &Panel, theme: &Theme, layout: &Layout) {
    let b = layout.body;
    let small = theme.small_font();
    match &panel.body {
        Body::Empty => {}
        Body::Working { note } => {
            c.text_wrapped(note, b.x, b.y, &theme.font, theme.text_dim, b.w);
        }
        Body::Answer { text, source } => {
            c.text(
                &source.to_uppercase(),
                b.x,
                b.y,
                &small,
                theme.voice,
                Some(b.w),
            );
            c.text_wrapped(text, b.x, b.y + 22.0, &theme.font, theme.text, b.w);
        }
        Body::Done {
            headline, detail, ..
        } => {
            let used = c.text_wrapped(headline, b.x, b.y, &theme.font, theme.ok, b.w);
            if !detail.is_empty() {
                c.text_wrapped(
                    detail,
                    b.x,
                    b.y + used + Metrics::UNIT,
                    &theme.font,
                    theme.text,
                    b.w,
                );
            }
        }
        Body::Error { message } => {
            c.text_wrapped(message, b.x, b.y, &theme.font, theme.danger, b.w);
        }
        Body::Proposal { headline, steps } => {
            c.text("PROPOSED", b.x, b.y, &small, theme.text_dim, Some(b.w));
            c.text_wrapped(
                headline,
                b.x,
                b.y + 18.0,
                &theme.title_font(),
                theme.text,
                b.w,
            );
            for (i, row) in layout.rows.iter().enumerate() {
                let Some(step) = steps.get(layout.first_row + i) else {
                    break;
                };
                draw_step(c, step, theme, *row, layout.first_row + i == panel.selected);
            }
            if layout.first_row + layout.rows.len() < steps.len() {
                let more = steps.len() - layout.first_row - layout.rows.len();
                let last = layout.rows.last().copied().unwrap_or(b);
                c.text(
                    &format!("{more} more"),
                    b.x + Metrics::GAP,
                    last.bottom() - 14.0,
                    &small,
                    theme.text_faint,
                    Some(b.w),
                );
            }
        }
    }
}

/// One step: the risk spine, then what it will do, then the capability that
/// permits it. The capability is shown because it is the thing being granted,
/// and hiding it would make the approval meaningless.
fn draw_step(c: &Canvas, step: &Step, theme: &Theme, row: Rect, selected: bool) {
    let inner = Rect::new(row.x, row.y + 2.0, row.w, row.h - 4.0);
    if selected {
        c.fill_rounded(inner, Metrics::RADIUS_SMALL, theme.surface_active);
    }
    let colour = theme.risk(step.risk);
    c.fill_rounded(
        Rect::new(inner.x, inner.y + 4.0, Metrics::ACCENT_BAR, inner.h - 8.0),
        Metrics::ACCENT_BAR / 2.0,
        colour,
    );

    let tx = inner.x + Metrics::ACCENT_BAR + Metrics::GAP;
    let tw = (inner.right() - tx - Metrics::GAP).max(0.0);
    c.text(
        &step.summary,
        tx,
        inner.y + 5.0,
        &theme.font,
        theme.text,
        Some(tw),
    );
    c.text(
        &step.capability,
        tx,
        inner.y + 23.0,
        &theme.font_mono,
        theme.text_dim,
        Some(tw),
    );
}

fn draw_footer(c: &Canvas, panel: &Panel, theme: &Theme, layout: &Layout) {
    let f = layout.footer;
    let small = theme.small_font();
    let mut x = f.x;
    for (key, what) in panel.hints() {
        let (kw, kh) = c.measure(key, &small, None);
        let pill = Rect::new(x, f.y + (f.h - kh - 6.0) / 2.0, kw + 12.0, kh + 6.0);
        c.fill_rounded(pill, Metrics::RADIUS_SMALL / 2.0, theme.surface);
        c.text(
            key,
            pill.x + 6.0,
            pill.y + 3.0,
            &small,
            theme.text_dim,
            None,
        );
        x = pill.right() + 6.0;

        let (ww, _) = c.measure(what, &small, None);
        c.text(what, x, pill.y + 3.0, &small, theme.text_dim, None);
        x += ww + Metrics::GAP + Metrics::UNIT;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::draw::Image;

    fn step(cap: &str, risk: Risk) -> Step {
        Step {
            capability: cap.to_string(),
            summary: format!("do {cap}"),
            risk,
        }
    }

    #[test]
    fn an_empty_panel_is_only_a_prompt() {
        let img = Image::new(1, 1).unwrap();
        let p = Panel::new();
        let l = Layout::compute(&p, Metrics::PANEL_WIDTH, &img.canvas(), &Theme::dark());
        assert_eq!(l.height, Metrics::PROMPT_HEIGHT);
        assert_eq!(l.body.h, 0.0);
        assert_eq!(
            l.footer.h, 0.0,
            "no keys to show when there is nothing to act on"
        );
        assert!(l.rows.is_empty());
    }

    #[test]
    fn the_panel_grows_for_a_body_but_never_past_its_maximum() {
        let img = Image::new(1, 1).unwrap();
        let theme = Theme::dark();

        let mut p = Panel::new();
        p.set_body(Body::Done {
            headline: "moved 3 files".into(),
            detail: String::new(),
            undo_hint: true,
        });
        let small = Layout::compute(&p, Metrics::PANEL_WIDTH, &img.canvas(), &theme);
        assert!(
            small.height > Metrics::PROMPT_HEIGHT,
            "a body makes the panel taller"
        );
        assert!(small.height < Metrics::PANEL_MAX_HEIGHT);

        p.set_body(Body::Proposal {
            headline: "413 moves".into(),
            steps: (0..200)
                .map(|i| step(&format!("fs.move:~/a{i}"), Risk::Write))
                .collect(),
        });
        let big = Layout::compute(&p, Metrics::PANEL_WIDTH, &img.canvas(), &theme);
        assert!(
            big.height <= Metrics::PANEL_MAX_HEIGHT,
            "200 steps must not make a panel {} tall",
            big.height
        );
        assert!(
            !big.rows.is_empty(),
            "and some of them must still be visible"
        );
        assert!(big.rows.len() < 200, "with the rest scrolled");
    }

    #[test]
    fn nothing_in_the_layout_overlaps_or_escapes_the_panel() {
        let img = Image::new(1, 1).unwrap();
        let mut p = Panel::new();
        p.set_body(Body::Proposal {
            headline: "413 moves across 137 files".into(),
            steps: (0..12)
                .map(|i| step(&format!("fs.move:~/x{i}"), Risk::Write))
                .collect(),
        });
        let l = Layout::compute(&p, Metrics::PANEL_WIDTH, &img.canvas(), &Theme::dark());

        assert!(
            l.marker.right() <= l.prompt_text.x,
            "the marker must not sit under the text"
        );
        assert!(l.prompt.bottom() <= l.body.y);
        assert!(l.body.bottom() <= l.footer.y);
        assert!(l.footer.bottom() <= l.height);
        for (i, row) in l.rows.iter().enumerate() {
            assert!(row.y >= l.body.y, "row {i} starts above the body");
            assert!(
                row.bottom() <= l.body.bottom() + 0.001,
                "row {i} spills out of the body"
            );
            assert!(row.right() <= l.panel.right());
            if i > 0 {
                assert!(
                    row.y >= l.rows[i - 1].bottom() - 0.001,
                    "row {i} overlaps the one above"
                );
            }
        }
    }

    #[test]
    fn an_attached_selection_takes_room_from_the_prompt_rather_than_covering_it() {
        let img = Image::new(1, 1).unwrap();
        let theme = Theme::dark();

        let mut bare = Panel::new();
        bare.input.set("tidy these");
        let without = Layout::compute(&bare, Metrics::PANEL_WIDTH, &img.canvas(), &theme);
        assert!(without.context.is_none());

        let mut attached = Panel::new();
        attached.input.set("tidy these");
        attached.context = Some("3 files · Downloads".into());
        let with = Layout::compute(&attached, Metrics::PANEL_WIDTH, &img.canvas(), &theme);

        let chip = with.context.expect("the attachment must be shown");
        assert!(chip.w > 0.0 && chip.h > 0.0);
        assert!(
            chip.right() <= Metrics::PANEL_WIDTH - Metrics::PAD + 0.001,
            "the chip runs off the panel"
        );
        assert!(
            with.prompt_text.right() <= chip.x,
            "the prompt text would be drawn under the chip"
        );
        assert!(
            with.prompt_text.w < without.prompt_text.w,
            "the text did not yield any space"
        );
        // The chip is vertically centred on the prompt line, not floating.
        assert!(chip.y > 0.0 && chip.bottom() < Metrics::PROMPT_HEIGHT);
    }

    #[test]
    fn an_attached_selection_is_actually_drawn() {
        let theme = Theme::dark();
        let mut p = Panel::new();
        p.input.set("tidy these");
        p.context = Some("3 files · Downloads".into());

        let img = Image::new(Metrics::PANEL_WIDTH as i32, 80).unwrap();
        let c = img.canvas();
        let l = Layout::compute(&p, Metrics::PANEL_WIDTH, &c, &theme);
        render(&c, &p, &theme, &l, true);

        let chip = l.context.expect("laid out");
        assert!(
            img.variety(chip) > 3,
            "the chip region is a flat fill, so nothing was drawn in it"
        );
        // And the chip is distinguishable from the panel behind it.
        let inside = img.pixel((chip.x + chip.w / 2.0) as i32, (chip.y + 2.0) as i32);
        let outside = img.pixel((chip.x - 4.0) as i32, (chip.y + 2.0) as i32);
        assert_ne!(inside, outside, "the chip has no visible edge");
    }

    #[test]
    fn scrolling_follows_the_selection_and_stops_at_the_ends() {
        let mut p = Panel::new();
        p.set_body(Body::Proposal {
            headline: "many".into(),
            steps: (0..10)
                .map(|i| step(&format!("fs.move:~/x{i}"), Risk::Write))
                .collect(),
        });
        let visible = 4;

        for _ in 0..3 {
            p.move_selection(1, visible);
        }
        assert_eq!(p.selected, 3);
        assert_eq!(p.scroll, 0, "still on the first page");

        p.move_selection(1, visible);
        assert_eq!(p.selected, 4);
        assert_eq!(p.scroll, 1, "the view followed by exactly one row");

        for _ in 0..20 {
            p.move_selection(1, visible);
        }
        assert_eq!(p.selected, 9, "clamped at the last step");
        assert_eq!(p.scroll, 6, "showing the last four");

        for _ in 0..20 {
            p.move_selection(-1, visible);
        }
        assert_eq!(p.selected, 0);
        assert_eq!(p.scroll, 0);
    }

    #[test]
    fn selection_moves_do_nothing_when_there_are_no_steps() {
        let mut p = Panel::new();
        p.move_selection(1, 4);
        assert_eq!(p.selected, 0);
        p.set_body(Body::Answer {
            text: "hello".into(),
            source: "claude".into(),
        });
        p.move_selection(5, 4);
        assert_eq!(p.selected, 0, "an answer has no rows to select");
    }

    #[test]
    fn the_marker_takes_the_colour_of_the_worst_step_not_the_first() {
        let mut p = Panel::new();
        p.set_body(Body::Proposal {
            headline: "clean up".into(),
            steps: vec![
                step("fs.read:~/**", Risk::Read),
                step("fs.delete:~/**", Risk::Critical),
                step("fs.move:~/**", Risk::Write),
            ],
        });
        assert_eq!(
            p.body.peak_risk(),
            Some(Risk::Critical),
            "a plan is as dangerous as its worst step"
        );
        p.set_body(Body::Proposal {
            headline: "look".into(),
            steps: vec![step("fs.read:~/**", Risk::Read)],
        });
        assert_eq!(p.body.peak_risk(), Some(Risk::Read));
        p.set_body(Body::Empty);
        assert_eq!(p.body.peak_risk(), None);
    }

    #[test]
    fn a_proposal_always_says_how_to_approve_and_how_to_refuse() {
        let mut p = Panel::new();
        p.set_body(Body::Proposal {
            headline: "x".into(),
            steps: vec![],
        });
        let hints = p.hints();
        assert!(hints.iter().any(|(k, _)| *k == "enter"));
        assert!(hints.iter().any(|(k, _)| *k == "esc"));
    }

    #[test]
    fn a_rendered_proposal_shows_the_risk_colour_of_its_steps() {
        let theme = Theme::dark();
        let mut p = Panel::new();
        p.input.set("delete my old logs");
        p.set_body(Body::Proposal {
            headline: "3 deletions".into(),
            steps: vec![step("fs.delete:~/logs/**", Risk::Critical)],
        });

        let img = Image::new(
            Metrics::PANEL_WIDTH as i32,
            Metrics::PANEL_MAX_HEIGHT as i32,
        )
        .expect("offscreen surface");
        let c = img.canvas();
        let l = Layout::compute(&p, Metrics::PANEL_WIDTH, &c, &theme);
        render(&c, &p, &theme, &l, true);

        assert!(!l.rows.is_empty(), "the step must be laid out to be drawn");
        let row = l.rows[0];
        // Sample the middle of the accent bar. This is the claim that matters:
        // a critical step is visibly red before any text is read.
        let (r, g, b, a) = img.pixel(
            (row.x + Metrics::ACCENT_BAR / 2.0) as i32,
            (row.y + row.h / 2.0) as i32,
        );
        assert!(a > 200, "the risk spine was not drawn (alpha {a})");
        assert!(
            r > 200 && g < 140 && b < 140,
            "the spine is not red: ({r},{g},{b})"
        );

        // And the step's text was drawn, not merely its coloured bar.
        assert!(
            img.variety(row) > 8,
            "the step row is a flat fill, so its text never appeared"
        );
    }

    #[test]
    fn a_read_only_plan_renders_in_a_different_colour_from_a_destructive_one() {
        let theme = Theme::dark();
        let sample = |risk: Risk| {
            let mut p = Panel::new();
            p.set_body(Body::Proposal {
                headline: "plan".into(),
                steps: vec![step("fs.x:~/**", risk)],
            });
            let img = Image::new(
                Metrics::PANEL_WIDTH as i32,
                Metrics::PANEL_MAX_HEIGHT as i32,
            )
            .unwrap();
            let c = img.canvas();
            let l = Layout::compute(&p, Metrics::PANEL_WIDTH, &c, &theme);
            render(&c, &p, &theme, &l, true);
            let row = l.rows[0];
            img.pixel(
                (row.x + Metrics::ACCENT_BAR / 2.0) as i32,
                (row.y + row.h / 2.0) as i32,
            )
        };
        let read = sample(Risk::Read);
        let critical = sample(Risk::Critical);
        assert_ne!(read, critical, "risk is not distinguishable on screen");
        assert!(
            read.2 > read.0,
            "a read step should be the blue one: {read:?}"
        );
        assert!(
            critical.0 > critical.2,
            "a critical step should be the red one: {critical:?}"
        );
    }

    #[test]
    fn an_empty_prompt_shows_a_hint_and_a_filled_one_shows_the_text() {
        let theme = Theme::dark();
        // Counting colours in the text area, not pixels on the whole panel: the
        // panel paints an opaque backdrop first, so "some pixel is set" is true
        // before anything is drawn.
        let variety_of = |text: &str| {
            let mut p = Panel::new();
            p.input.set(text);
            let img = Image::new(Metrics::PANEL_WIDTH as i32, 80).unwrap();
            let c = img.canvas();
            let l = Layout::compute(&p, Metrics::PANEL_WIDTH, &c, &theme);
            render(&c, &p, &theme, &l, true);
            img.variety(l.prompt_text)
        };
        assert!(variety_of("") > 3, "the empty prompt drew no placeholder");
        assert!(
            variety_of("tidy my downloads") > 3,
            "the typed text was not drawn"
        );
    }

    #[test]
    fn the_caret_stays_inside_the_prompt_when_the_text_is_too_long() {
        let theme = Theme::dark();
        let mut p = Panel::new();
        // Far longer than the prompt line can show.
        p.input.set(&"long text that keeps going ".repeat(20));

        let img = Image::new(Metrics::PANEL_WIDTH as i32, 80).unwrap();
        let c = img.canvas();
        let l = Layout::compute(&p, Metrics::PANEL_WIDTH, &c, &theme);
        render(&c, &p, &theme, &l, true);

        // The caret is at the end of the text. It must be drawn inside the
        // prompt area, near its right edge, not off the side of the panel.
        let area = l.prompt_text;
        let y = (area.y + area.h / 2.0) as i32;
        let lit = (area.x as i32..area.right() as i32)
            .filter(|x| img.pixel(*x, y).3 > 0)
            .max();
        assert!(lit.is_some(), "nothing was drawn on the prompt line");
        let lit = lit.unwrap() as f64;
        assert!(
            lit > area.right() - 12.0,
            "the caret should be at the right edge, last ink was at {lit} of {}",
            area.right()
        );
    }
}
