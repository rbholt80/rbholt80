//! Asking, in the window, about what is in front of you.
//!
//! The command bar has always existed as an overlay you summon over whatever
//! you are doing. That is the right shape for "do a thing anywhere on this
//! machine" and the wrong shape for "do a thing to *this*" — an overlay does
//! not know what folder you are looking at, and you should not have to tell it.
//!
//! So this: one line at the top of the window that already knows. Ask, and it
//! shows what it would do before it does anything, each step in the colour of
//! its risk. Nothing happens until the plan you were shown is the plan you say
//! yes to — which is why confirming sends that plan back rather than the words
//! that produced it. Asking twice can resolve differently; agreeing to one plan
//! and running another is how a system loses the right to be trusted.

use crate::find::{self, Hit};
use crate::link::Link;
use nous_core::json::{json_obj, Json};
use nous_ui::draw::{Canvas, Rect};
use nous_ui::input::Edit;
use nous_ui::panel::Step;
use nous_ui::theme::{parse_risk, Metrics, Risk, Theme};

/// Where the ask bar has got to.
pub enum State {
    /// Nothing asked. The bar is one line with a hint in it.
    Idle,
    /// Sent, waiting. Not a spinner: the daemon answers or it does not.
    Thinking,
    /// A plan, and the exact document to send back to run it.
    Proposal {
        headline: String,
        steps: Vec<Step>,
        plan: Json,
    },
    /// Something to read: an answer, or what went wrong.
    Said { text: String, trouble: bool },
    /// Things that match what is being typed. Not a mode: the request reading
    /// of the same words is always still there, at the end of the list.
    Found { hits: Vec<Hit> },
}

pub struct Ask {
    pub edit: Edit,
    pub state: State,
    /// Whether the bar has the keyboard. The window's other views want the
    /// arrow keys, so the bar only takes them when it is being typed into.
    pub focused: bool,
    /// Which result the keyboard is on. `None` means the request reading —
    /// the thing that happens when you type a sentence and press Enter
    /// without touching an arrow key.
    pub chosen: Option<usize>,
}

impl Default for Ask {
    fn default() -> Ask {
        Ask::new()
    }
}

impl Ask {
    pub fn new() -> Ask {
        Ask {
            edit: Edit::new(),
            state: State::Idle,
            focused: false,
            chosen: None,
        }
    }

    /// The worst thing the plan would do, for the colour of its marker — the
    /// same rule the panel uses on a plan and the file view uses on a folder.
    pub fn peak_risk(&self) -> Option<Risk> {
        match &self.state {
            State::Proposal { steps, .. } => steps.iter().map(|s| s.risk).max(),
            _ => None,
        }
    }

    pub fn has_proposal(&self) -> bool {
        matches!(self.state, State::Proposal { .. })
    }

    pub fn hits(&self) -> &[Hit] {
        match &self.state {
            State::Found { hits } => hits,
            _ => &[],
        }
    }

    /// The result the keyboard is on, if it is on one.
    pub fn chosen_hit(&self) -> Option<&Hit> {
        self.chosen.and_then(|i| self.hits().get(i))
    }

    /// Look for whatever is being typed, without doing anything about it.
    ///
    /// Runs on every keystroke, so it must be cheap and it must never change
    /// anything. Finding is free; that is what makes it safe to do while
    /// someone is still deciding what they meant.
    pub fn look(
        &mut self,
        link: &mut Link,
        places: &[crate::places::Place],
        deeds: &[crate::history::Deed],
    ) {
        let q = self.edit.text().trim().to_string();
        if q.is_empty() {
            self.state = State::Idle;
            self.chosen = None;
            return;
        }
        // A proposal on screen is not thrown away by more typing: someone
        // refining a question should not lose the answer to the last one until
        // they ask again.
        if self.has_proposal() {
            return;
        }
        let files = link.search(&q, 24).unwrap_or(Json::Null);
        let hits = find::gather(&q, &files, places, deeds, VIEW_NAMES, 8);
        // A sentence that matches nothing is a request, and putting an empty
        // list on screen for it would be answering a question nobody asked.
        self.chosen = match (hits.is_empty(), find::looks_like_a_request(&q)) {
            (true, _) => None,
            // Something matched, but it reads as an instruction: keep the
            // matches visible and leave the keyboard on the request, since
            // guessing wrong here costs one arrow key rather than an outcome.
            (false, true) => None,
            (false, false) => Some(0),
        };
        self.state = State::Found { hits };
    }

    /// Move the keyboard through the results, and off the end onto the request.
    pub fn move_choice(&mut self, delta: i64) {
        let n = self.hits().len() as i64;
        if n == 0 {
            self.chosen = None;
            return;
        }
        // `None` sits at the end of the list, so pressing Down from the last
        // result reaches "ask for this" rather than stopping dead.
        let at = self.chosen.map(|i| i as i64).unwrap_or(n);
        let next = (at + delta).clamp(0, n);
        self.chosen = if next >= n { None } else { Some(next as usize) };
    }

    /// Give up on whatever is showing. Escape, or clicking away.
    pub fn dismiss(&mut self) {
        self.state = State::Idle;
        self.focused = false;
        self.edit.clear();
    }

    /// Ask, with the folder currently being looked at as the context.
    ///
    /// The context is the difference between this and the overlay: "sort these
    /// by year" needs to know which "these", and being in the window means it
    /// already does.
    pub fn submit(&mut self, link: &mut Link, context: &str) {
        let text = self.edit.text().trim().to_string();
        if text.is_empty() {
            return;
        }
        self.state = State::Thinking;
        let params = json_obj([
            ("text", text.clone().into()),
            ("context", json_obj([("folder", context.into())])),
        ]);
        match link.call_intent_plan(params) {
            Ok(reply) => self.state = read_plan(&reply, &text),
            Err(e) => {
                self.state = State::Said {
                    text: e,
                    trouble: true,
                }
            }
        }
    }

    /// Say yes to the plan that was shown, sending that plan back.
    pub fn confirm(&mut self, link: &mut Link) {
        let State::Proposal { plan, .. } = &self.state else {
            return;
        };
        let params = json_obj([("plan", plan.clone())]);
        match link.call_intent_confirm(params) {
            Ok(reply) => {
                let said = reply.str_or("summary", "");
                self.state = State::Said {
                    text: if said.is_empty() {
                        "done".to_string()
                    } else {
                        said.to_string()
                    },
                    trouble: false,
                };
                self.edit.clear();
            }
            Err(e) => {
                self.state = State::Said {
                    text: e,
                    trouble: true,
                }
            }
        }
    }
}

/// Turn a preflight reply into something to show.
pub fn read_plan(reply: &Json, asked: &str) -> State {
    // A question it wants answering before it will plan anything.
    let clarify = reply.str_or("clarification", "");
    if !clarify.is_empty() {
        return State::Said {
            text: clarify.to_string(),
            trouble: false,
        };
    }
    let steps: Vec<Step> = reply
        .arr_or_empty("steps")
        .iter()
        .map(|s| Step {
            capability: s.str_or("capability", "?").to_string(),
            summary: s.str_or("summary", "").to_string(),
            // The daemon states the risk its policy evaluated. Working it out
            // again here could disagree with what was actually decided, and
            // the colour would then be describing a different judgement from
            // the one that will be enforced.
            risk: parse_risk(s.str_or("risk", "")),
        })
        .collect();

    if steps.is_empty() {
        // Nothing to do is an answer, and often the right one — but an empty
        // proposal box would read as a failure.
        let said = reply.str_or("answer", "");
        return State::Said {
            text: if said.is_empty() {
                "nothing to do".to_string()
            } else {
                said.to_string()
            },
            trouble: false,
        };
    }
    let plan = reply.get("plan").cloned().unwrap_or(Json::Null);
    if plan.is_null() {
        // Steps with no plan document cannot be confirmed as shown, and
        // re-resolving the words would risk running something else.
        return State::Said {
            text: "the daemon described a plan it did not hand over".to_string(),
            trouble: true,
        };
    }
    State::Proposal {
        headline: headline_for(reply, asked, steps.len()),
        steps,
        plan,
    }
}

fn headline_for(reply: &Json, asked: &str, n: usize) -> String {
    let given = reply.str_or("summary", "");
    if !given.is_empty() {
        return given.to_string();
    }
    let noun = if n == 1 { "step" } else { "steps" };
    format!("{asked} — {n} {noun}")
}

/// What the views are called, for finding them by name.
const VIEW_NAMES: &[(&str, &str)] = &[
    ("Files", "your folders"),
    ("Player", "music and video"),
    ("Edit", "the cutting room"),
    ("History", "what has been done"),
];

// --- layout ----------------------------------------------------------------

pub const BAR_H: f64 = 46.0;
const STEP_H: f64 = 34.0;
const HEADLINE_H: f64 = 30.0;
const FOOTER_H: f64 = 26.0;

pub struct Layout {
    pub bar: Rect,
    /// The dropped panel under the bar. Zero-height when nothing is showing.
    pub sheet: Rect,
    pub steps: Vec<Rect>,
    /// Where "Enter to run · Esc to leave it" goes.
    pub footer: Rect,
}

impl Layout {
    pub fn compute(ask: &Ask, width: f64, top: f64) -> Layout {
        let pad = Metrics::PAD;
        let bar = Rect::new(0.0, top, width, BAR_H);
        let (sheet_h, n) = match &ask.state {
            State::Proposal { steps, .. } => (
                HEADLINE_H + steps.len() as f64 * STEP_H + FOOTER_H + pad,
                steps.len(),
            ),
            // Results, plus one row at the end that is always there: whatever
            // was typed, as something to ask for. That row is the reason this
            // is not a search box — the other reading never goes away.
            State::Found { hits } => (
                (hits.len() + 1) as f64 * STEP_H + FOOTER_H + pad,
                hits.len() + 1,
            ),
            State::Said { .. } | State::Thinking => (HEADLINE_H + pad, 0),
            State::Idle => (0.0, 0),
        };
        let sheet = Rect::new(0.0, bar.bottom(), width, sheet_h);
        let mut steps = Vec::new();
        // A proposal has a headline above its steps; a result list starts at
        // the top, because the first result should be under the cursor.
        let mut y = sheet.y
            + if matches!(ask.state, State::Found { .. }) {
                4.0
            } else {
                HEADLINE_H
            };
        for _ in 0..n {
            steps.push(Rect::new(pad, y, width - pad * 2.0, STEP_H - 4.0));
            y += STEP_H;
        }
        Layout {
            bar,
            sheet,
            steps,
            footer: Rect::new(pad, y, width - pad * 2.0, FOOTER_H),
        }
    }

    /// How much room the bar and anything under it take from the views.
    pub fn height(&self) -> f64 {
        BAR_H + self.sheet.h
    }
}

// --- drawing ---------------------------------------------------------------

pub fn render(c: &Canvas, ask: &Ask, theme: &Theme, layout: &Layout, folder: &str) {
    let bar = layout.bar;
    c.fill_rect(bar, theme.backdrop_opaque);
    let f = theme.body_font();
    let small = theme.small_font();
    let cy = bar.y + bar.h / 2.0;

    // The marker: a dot in the colour of the worst thing being proposed, or
    // quiet when nothing is. Same rule as the panel and the folder view, so a
    // colour means one thing everywhere.
    let dot = match ask.peak_risk() {
        Some(r) => theme.risk(r),
        None if ask.focused => theme.voice,
        None => theme.text_faint,
    };
    c.fill_circle(Metrics::PAD + 4.0, cy, 4.0, dot);

    let text_x = Metrics::PAD + 18.0;
    let text_w = bar.w - text_x - Metrics::PAD;
    if ask.edit.is_empty() && !ask.focused {
        // The hint names the folder, because that is what makes asking here
        // different from asking anywhere.
        let hint = if folder.is_empty() {
            "Ask for something".to_string()
        } else {
            format!("Ask for something — about {folder}")
        };
        c.text(&hint, text_x, cy - 9.0, &f, theme.text_faint, Some(text_w));
    } else {
        let text = ask.edit.text();
        c.text(text, text_x, cy - 9.0, &f, theme.text, Some(text_w));
        if ask.focused {
            let caret = ask.edit.caret().min(text.len());
            let (cw, _) = c.measure(&text[..caret], &f, None);
            c.line(
                text_x + cw,
                cy - 10.0,
                text_x + cw,
                cy + 10.0,
                1.5,
                theme.voice,
            );
        }
    }
    c.line(
        0.0,
        bar.bottom() - 0.5,
        bar.w,
        bar.bottom() - 0.5,
        1.0,
        theme.hairline,
    );

    if layout.sheet.h <= 0.0 {
        return;
    }
    let sheet = layout.sheet;
    c.fill_rect(sheet, theme.floating());
    c.line(
        0.0,
        sheet.bottom() - 0.5,
        sheet.w,
        sheet.bottom() - 0.5,
        1.0,
        theme.hairline,
    );

    match &ask.state {
        State::Thinking => {
            c.text(
                "thinking…",
                Metrics::PAD,
                sheet.y + 8.0,
                &f,
                theme.text_dim,
                None,
            );
        }
        State::Said { text, trouble } => {
            c.text(
                text,
                Metrics::PAD,
                sheet.y + 8.0,
                &f,
                if *trouble { theme.warn } else { theme.text },
                Some(sheet.w - Metrics::PAD * 2.0),
            );
        }
        State::Proposal {
            headline, steps, ..
        } => {
            c.text(
                headline,
                Metrics::PAD,
                sheet.y + 8.0,
                &f,
                theme.text,
                Some(sheet.w - Metrics::PAD * 2.0),
            );
            for (i, r) in layout.steps.iter().enumerate() {
                let Some(s) = steps.get(i) else { continue };
                // The spine again, so a step here and the same step in the
                // ledger afterwards are visibly one thing.
                c.fill_rounded(
                    Rect::new(r.x, r.y + 4.0, 3.0, r.h - 8.0),
                    1.5,
                    theme.risk(s.risk),
                );
                c.text(
                    &s.summary,
                    r.x + 14.0,
                    r.y + 2.0,
                    &f,
                    theme.text,
                    Some(r.w * 0.62),
                );
                let (cw, ch) = c.measure(&s.capability, &small, None);
                c.text(
                    &s.capability,
                    r.right() - cw,
                    r.y + (r.h - ch) / 2.0,
                    &small,
                    theme.text_faint,
                    None,
                );
            }
            // Always both ways out, and always said: a proposal whose only
            // documented gesture is the one that runs it is a trap.
            let note = "Enter to run it · Esc to leave it";
            c.text(
                note,
                layout.footer.x,
                layout.footer.y,
                &small,
                theme.text_faint,
                None,
            );
        }
        State::Found { hits } => {
            for (i, r) in layout.steps.iter().enumerate() {
                let on = ask.chosen == Some(i) || (ask.chosen.is_none() && i == hits.len());
                if on {
                    c.fill_rounded(*r, Metrics::RADIUS_SMALL / 2.0, theme.surface_active);
                }
                match hits.get(i) {
                    Some(h) => {
                        c.text(
                            &h.title,
                            r.x + 12.0,
                            r.y + 2.0,
                            &f,
                            theme.text,
                            Some(r.w * 0.55),
                        );
                        let (dw, dh) = c.measure(&h.detail, &small, Some(r.w * 0.3));
                        c.text(
                            &h.detail,
                            r.right() - dw - 62.0,
                            r.y + (r.h - dh) / 2.0,
                            &small,
                            theme.text_faint,
                            Some(r.w * 0.3),
                        );
                        // What kind of thing it is, so one list of mixed
                        // things still reads as a list.
                        let (kw, kh) = c.measure(h.sort.label(), &small, None);
                        c.text(
                            h.sort.label(),
                            r.right() - kw - 8.0,
                            r.y + (r.h - kh) / 2.0,
                            &small,
                            theme.text_dim,
                            None,
                        );
                    }
                    // The last row: what was typed, as a request.
                    None => {
                        let asked = ask.edit.text().trim();
                        c.fill_circle(r.x + 6.0, r.y + r.h / 2.0, 3.0, theme.voice);
                        c.text(
                            &format!("Ask for “{asked}”"),
                            r.x + 16.0,
                            r.y + 2.0,
                            &f,
                            theme.text,
                            Some(r.w * 0.7),
                        );
                        let (kw, kh) = c.measure("request", &small, None);
                        c.text(
                            "request",
                            r.right() - kw - 8.0,
                            r.y + (r.h - kh) / 2.0,
                            &small,
                            theme.voice,
                            None,
                        );
                    }
                }
            }
            let note = if ask.chosen.is_some() {
                "Enter opens it · ↓ to ask instead"
            } else {
                "Enter asks for it · ↑ for what matched"
            };
            c.text(
                note,
                layout.footer.x,
                layout.footer.y,
                &small,
                theme.text_faint,
                None,
            );
        }
        State::Idle => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nous_core::json::parse;
    use nous_ui::draw::Image;

    fn preflight(steps: &str) -> Json {
        parse(&format!(
            r#"{{"steps":[{steps}],"plan":{{"intent_id":"i1","steps":[{steps}]}}}}"#
        ))
        .unwrap()
    }

    const MOVE: &str =
        r#"{"capability":"fs.move","summary":"move 84 images into Pictures/2026","risk":"write"}"#;
    const DEL: &str =
        r#"{"capability":"fs.delete","summary":"remove 22 empty folders","risk":"elevated"}"#;

    #[test]
    fn a_plan_is_shown_before_anything_is_done() {
        let s = read_plan(&preflight(&format!("{MOVE},{DEL}")), "tidy my downloads");
        match s {
            State::Proposal { steps, plan, .. } => {
                assert_eq!(steps.len(), 2);
                assert_eq!(steps[0].risk, Risk::Write);
                assert_eq!(steps[1].risk, Risk::Elevated);
                assert!(!plan.is_null(), "nothing to confirm with");
            }
            _ => panic!("a plan was not offered"),
        }
    }

    #[test]
    fn the_marker_takes_the_colour_of_the_worst_step() {
        let mut a = Ask::new();
        a.state = read_plan(&preflight(&format!("{MOVE},{DEL}")), "x");
        assert_eq!(
            a.peak_risk(),
            Some(Risk::Elevated),
            "the first step, not the worst"
        );
    }

    #[test]
    fn a_plan_with_no_document_is_refused_rather_than_re_resolved() {
        // Confirming has to send back the plan that was shown. Re-resolving
        // the words could produce a different plan from the one agreed to,
        // which is how a system loses the right to be trusted.
        let r = parse(&format!(r#"{{"steps":[{MOVE}]}}"#)).unwrap();
        match read_plan(&r, "x") {
            State::Said { trouble, text } => {
                assert!(trouble, "a plan that cannot be run was shown as normal");
                assert!(text.contains("did not hand over"), "{text}");
            }
            _ => panic!("offered to run a plan it cannot send back"),
        }
    }

    #[test]
    fn a_question_back_is_shown_rather_than_planned_around() {
        let r = parse(r#"{"clarification":"which downloads folder?","steps":[]}"#).unwrap();
        match read_plan(&r, "tidy it") {
            State::Said { text, trouble } => {
                assert_eq!(text, "which downloads folder?");
                assert!(!trouble, "a question is not an error");
            }
            _ => panic!("planned around a question"),
        }
    }

    #[test]
    fn nothing_to_do_says_so_rather_than_showing_an_empty_box() {
        let r = parse(r#"{"steps":[],"answer":"that folder is already tidy"}"#).unwrap();
        match read_plan(&r, "tidy it") {
            State::Said { text, trouble } => {
                assert_eq!(text, "that folder is already tidy");
                assert!(!trouble);
            }
            _ => panic!("showed an empty proposal"),
        }
        // And with nothing said either.
        let r = parse(r#"{"steps":[]}"#).unwrap();
        assert!(matches!(read_plan(&r, "x"), State::Said { .. }));
    }

    #[test]
    fn a_risk_the_daemon_names_is_taken_from_the_daemon() {
        // Working it out again here could disagree with what the policy
        // actually decided, and the colour would describe a different
        // judgement from the one that will be enforced.
        let r = preflight(r#"{"capability":"fs.read","summary":"read it","risk":"critical"}"#);
        match read_plan(&r, "x") {
            State::Proposal { steps, .. } => assert_eq!(
                steps[0].risk,
                Risk::Critical,
                "recomputed the risk from the capability name"
            ),
            _ => panic!(),
        }
    }

    #[test]
    fn an_idle_bar_takes_no_room_from_the_views() {
        let a = Ask::new();
        let l = Layout::compute(&a, 1000.0, 44.0);
        assert_eq!(l.sheet.h, 0.0);
        assert_eq!(l.height(), BAR_H);
        assert!(l.steps.is_empty());
    }

    #[test]
    fn a_proposal_makes_room_for_every_step_and_both_ways_out() {
        let mut a = Ask::new();
        a.state = read_plan(&preflight(&format!("{MOVE},{DEL}")), "x");
        let l = Layout::compute(&a, 1000.0, 44.0);
        assert_eq!(l.steps.len(), 2);
        assert!(l.steps[1].y >= l.steps[0].bottom(), "steps overlap");
        assert!(
            l.footer.y >= l.steps[1].bottom(),
            "the way out is under a step"
        );
        assert!(
            l.footer.bottom() <= l.sheet.bottom() + 0.001,
            "the way out falls off the sheet"
        );
        assert!(l.height() > BAR_H);
    }

    #[test]
    fn the_bar_and_its_sheet_are_actually_drawn_in_both_themes() {
        for theme in [Theme::dark(), Theme::light()] {
            let mut a = Ask::new();
            a.state = read_plan(&preflight(&format!("{MOVE},{DEL}")), "tidy my downloads");
            let img = Image::new(1000, 300).unwrap();
            let l = Layout::compute(&a, 1000.0, 0.0);
            render(&img.canvas(), &a, &theme, &l, "Downloads");
            assert!(img.variety(l.bar) > 3, "the bar is blank");
            assert!(img.variety(l.sheet) > 4, "the plan is blank");
            // The two steps carry different risk colours.
            let spine = |r: Rect| img.pixel((r.x + 1.0) as i32, (r.y + r.h / 2.0) as i32);
            assert_ne!(
                spine(l.steps[0]),
                spine(l.steps[1]),
                "a move and a deletion are drawn the same"
            );
        }
    }

    #[test]
    fn the_sheet_hides_what_is_under_it() {
        // It floats over a view. Drawn in the surface tint it would let the
        // files read through it, the way the context menu did.
        let theme = Theme::dark();
        let mut a = Ask::new();
        a.state = read_plan(&preflight(MOVE), "x");
        let img = Image::new(1000, 300).unwrap();
        let c = img.canvas();
        // Something busy underneath.
        for i in 0..30 {
            c.fill_rect(
                Rect::new(i as f64 * 33.0, 0.0, 18.0, 300.0),
                theme.risk(Risk::Critical),
            );
        }
        let l = Layout::compute(&a, 1000.0, 0.0);
        render(&c, &a, &theme, &l, "Downloads");
        // A clear band inside the sheet, below the headline and clear of the
        // step text.
        let y = (l.sheet.bottom() - 6.0) as i32;
        let row: Vec<_> = (200..800).step_by(7).map(|x| img.pixel(x, y)).collect();
        assert!(
            row.iter().all(|p| *p == row[0]),
            "the sheet is see-through: {:?}…",
            &row[..4]
        );
    }

    #[test]
    fn an_empty_question_asks_nothing() {
        let mut a = Ask::new();
        let mut link = Link::new();
        a.edit.set("   ");
        a.submit(&mut link, "/home/j");
        assert!(matches!(a.state, State::Idle), "sent an empty question");
    }

    #[test]
    fn dismissing_clears_the_question_and_the_plan() {
        let mut a = Ask::new();
        a.edit.set("tidy it");
        a.state = read_plan(&preflight(MOVE), "tidy it");
        a.focused = true;
        a.dismiss();
        assert!(matches!(a.state, State::Idle));
        assert!(a.edit.is_empty());
        assert!(!a.focused);
    }

    #[test]
    fn confirming_with_nothing_proposed_does_nothing() {
        let mut a = Ask::new();
        let mut link = Link::new();
        a.confirm(&mut link);
        assert!(matches!(a.state, State::Idle), "ran something with no plan");
    }
}
