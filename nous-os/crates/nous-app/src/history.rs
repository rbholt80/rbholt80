//! What was done, and how to take it back.
//!
//! Every change this system makes goes through the broker, which writes down
//! what happened and how to reverse it before doing it. That promise is the
//! reason it is safe to let something clever near your files — and until now it
//! had no window, which made it a claim rather than a feature. A safety net
//! nobody can look at is not a safety net.
//!
//! So: the ledger, most recent first, each line saying what happened in words
//! and carrying its own way back. Nothing here is clever. It is the plainest
//! view in the system on purpose, because it is the one people will come to
//! when something has gone wrong and they are not in a mood to learn anything.

use nous_core::json::Json;
use nous_ui::draw::{Canvas, Rect};
use nous_ui::theme::{Metrics, Risk, Theme};

/// One thing that happened.
#[derive(Debug, Clone, PartialEq)]
pub struct Deed {
    pub seq: u64,
    pub ts: u64,
    /// What was asked for, in the words it was asked in.
    pub intent: String,
    /// What actually happened.
    pub detail: String,
    pub capability: String,
    pub risk: Risk,
    /// Whether it worked. A refusal is worth showing: it is the system saying
    /// no, and someone wondering why nothing happened deserves to find it here.
    pub outcome: String,
    /// Whether there is a way back, and whether it has been taken already.
    pub undoable: bool,
    pub undone_by: Option<u64>,
    /// For an undo that must be done by hand, what to do.
    pub manual_note: Option<String>,
}

impl Deed {
    /// The line to show. The intent where there was one, because "tidy my
    /// downloads" is what the person will be looking for; the detail otherwise,
    /// because a capability string is not a sentence.
    pub fn headline(&self) -> &str {
        if !self.intent.is_empty() {
            &self.intent
        } else if !self.detail.is_empty() {
            &self.detail
        } else {
            &self.capability
        }
    }

    /// The second line, when it says something the first does not.
    pub fn subline(&self) -> Option<&str> {
        if self.detail.is_empty() || self.detail == self.intent {
            return None;
        }
        if self.intent.is_empty() {
            return None;
        }
        Some(&self.detail)
    }

    pub fn succeeded(&self) -> bool {
        self.outcome == "ok" || self.outcome == "applied" || self.outcome == "success"
    }

    /// Whether the undo button should be there and live.
    pub fn can_undo(&self) -> bool {
        self.undoable && self.undone_by.is_none() && self.succeeded()
    }
}

/// Read the daemon's journal reply.
pub fn read(reply: &Json) -> Vec<Deed> {
    reply.arr_or_empty("records").iter().map(deed_of).collect()
}

fn deed_of(r: &Json) -> Deed {
    let undo = r.get("undo");
    let kind = undo.map(|u| u.str_or("kind", "")).unwrap_or("");
    // A null undo, or one of kind "none", means the broker had no way back for
    // this. "manual" means there is one but a person has to walk it.
    let undoable = !matches!(kind, "" | "none");
    Deed {
        seq: r.get("seq").and_then(|v| v.as_u64()).unwrap_or(0),
        ts: r.get("ts").and_then(|v| v.as_u64()).unwrap_or(0),
        intent: r.str_or("intent", "").to_string(),
        detail: r.str_or("detail", "").to_string(),
        capability: r.str_or("capability", "").to_string(),
        risk: risk_named(r.str_or("risk", "")),
        outcome: r.str_or("outcome", "").to_string(),
        undoable: undoable && kind != "manual",
        undone_by: r.get("undone_by").and_then(|v| v.as_u64()),
        manual_note: if kind == "manual" {
            Some(
                undo.map(|u| u.str_or("note", "by hand"))
                    .unwrap_or("by hand")
                    .to_string(),
            )
        } else {
            None
        },
    }
}

/// The risk as the journal spells it.
///
/// An unrecognised name is treated as the most dangerous, matching the rule the
/// rest of the system uses: a risk level nobody understands is not a mild one.
fn risk_named(s: &str) -> Risk {
    match s {
        "read" => Risk::Read,
        "write" => Risk::Write,
        "elevated" => Risk::Elevated,
        _ => Risk::Critical,
    }
}

/// The most recent thing that can still be taken back.
///
/// What a plain "undo" means. Skips refusals, things with no way back, and
/// things already undone — pressing undo twice should reach two different
/// actions, not fail on the same one.
pub fn newest_undoable(deeds: &[Deed]) -> Option<&Deed> {
    deeds.iter().find(|d| d.can_undo())
}

/// "just now", "6 minutes ago", "yesterday".
///
/// A clock time would make the reader work out how long ago that was, and the
/// only question anyone asks of this list is "was that the thing I just did?".
pub fn when(ts: u64, now: u64) -> String {
    let ago = now.saturating_sub(ts);
    match ago {
        0..=45 => "just now".to_string(),
        46..=5399 => {
            let m = (ago + 30) / 60;
            format!("{} minute{} ago", m.max(1), if m == 1 { "" } else { "s" })
        }
        5400..=79199 => {
            let h = (ago + 1800) / 3600;
            format!("{} hour{} ago", h.max(1), if h == 1 { "" } else { "s" })
        }
        79200..=172_799 => "yesterday".to_string(),
        _ => {
            let d = ago / 86400;
            format!("{} days ago", d)
        }
    }
}

// --- layout ----------------------------------------------------------------

const ROW_H: f64 = 62.0;
const UNDO_W: f64 = 76.0;
const HEADER_H: f64 = 58.0;

pub struct Layout {
    pub panel: Rect,
    pub header: Rect,
    pub body: Rect,
    pub rows: Vec<Rect>,
    /// The undo button on each row, empty where there is nothing to undo.
    pub undos: Vec<Option<Rect>>,
    pub content_height: f64,
}

impl Layout {
    pub fn compute(deeds: &[Deed], width: f64, height: f64, scroll: f64) -> Layout {
        let pad = Metrics::PAD;
        let inner = (width - pad * 2.0).max(0.0);
        let body = Rect::new(pad, HEADER_H, inner, (height - HEADER_H).max(0.0));
        let mut rows = Vec::new();
        let mut undos = Vec::new();
        let mut y = body.y - scroll;
        for d in deeds {
            let row = Rect::new(body.x, y, inner, ROW_H - 6.0);
            undos.push(if d.can_undo() {
                Some(Rect::new(
                    row.right() - UNDO_W - 8.0,
                    row.y + (row.h - 28.0) / 2.0,
                    UNDO_W,
                    28.0,
                ))
            } else {
                None
            });
            rows.push(row);
            y += ROW_H;
        }
        Layout {
            panel: Rect::new(0.0, 0.0, width, height),
            header: Rect::new(pad, 0.0, inner, HEADER_H),
            body,
            rows,
            undos,
            content_height: deeds.len() as f64 * ROW_H,
        }
    }

    pub fn max_scroll(&self) -> f64 {
        (self.content_height - self.body.h).max(0.0)
    }

    /// Which row's undo button is under a point. Only rows on screen: a button
    /// scrolled out of sight must not still be pressable.
    pub fn undo_at(&self, x: f64, y: f64) -> Option<usize> {
        self.undos.iter().position(|u| {
            u.is_some_and(|r| {
                r.contains(x, y) && r.bottom() > self.body.y && r.y < self.body.bottom()
            })
        })
    }

    pub fn row_at(&self, x: f64, y: f64) -> Option<usize> {
        self.rows
            .iter()
            .position(|r| r.contains(x, y) && r.bottom() > self.body.y && r.y < self.body.bottom())
    }
}

// --- drawing ---------------------------------------------------------------

pub fn render(
    c: &Canvas,
    deeds: &[Deed],
    selected: usize,
    theme: &Theme,
    layout: &Layout,
    now: u64,
    connected: bool,
) {
    c.fill_rect(layout.panel, theme.backdrop_opaque);
    let title = theme.title_font();
    let body_f = theme.body_font();
    let small = theme.small_font();

    c.text("History", layout.header.x, 20.0, &title, theme.text, None);
    let undoable = deeds.iter().filter(|d| d.can_undo()).count();
    // What is on screen decides what the header says. Announcing "nothing to
    // show" above eight visible entries is a contradiction a reader has to
    // resolve, and they will resolve it by trusting neither.
    let note = if !deeds.is_empty() {
        if undoable == 0 {
            format!("{} entries", deeds.len())
        } else {
            format!("{} entries · {} can be taken back", deeds.len(), undoable)
        }
    } else if connected {
        "nothing has happened yet".to_string()
    } else {
        "no daemon — nothing is being recorded".to_string()
    };
    let (nw, nh) = c.measure(&note, &small, None);
    c.text(
        &note,
        layout.header.right() - nw,
        (HEADER_H - nh) / 2.0,
        &small,
        theme.text_faint,
        None,
    );

    if deeds.is_empty() {
        let msg = if connected {
            "Nothing has been done yet."
        } else {
            "The daemon keeps this record. Start it to see what has been done."
        };
        let (w, h) = c.measure(msg, &body_f, None);
        c.text(
            msg,
            layout.body.x + (layout.body.w - w) / 2.0,
            layout.body.y + (layout.body.h - h) / 2.0,
            &body_f,
            theme.text_faint,
            None,
        );
        return;
    }

    c.clip_rect(layout.body);
    for (i, row) in layout.rows.iter().enumerate() {
        if row.bottom() < layout.body.y || row.y > layout.body.bottom() {
            continue;
        }
        let Some(d) = deeds.get(i) else { continue };
        if i == selected {
            c.fill_rounded(*row, Metrics::RADIUS_SMALL / 2.0, theme.surface);
        }

        // The same spine the plan view uses, in the risk colour of what was
        // done — so an entry here and the step that made it are one thing.
        let spine = Rect::new(row.x, row.y + 6.0, 3.0, row.h - 12.0);
        c.fill_rounded(spine, 1.5, theme.risk(d.risk));

        let dim = d.undone_by.is_some() || !d.succeeded();
        let text_x = row.x + 16.0;
        let text_w = (row.w - 24.0 - UNDO_W - 20.0).max(40.0);
        c.text(
            d.headline(),
            text_x,
            row.y + 9.0,
            &body_f,
            if dim { theme.text_faint } else { theme.text },
            Some(text_w),
        );

        // Underneath: when, and what became of it.
        let mut under = when(d.ts, now);
        if let Some(by) = d.undone_by {
            under.push_str(&format!(" · taken back by #{by}"));
        } else if !d.succeeded() {
            under.push_str(&format!(" · {}", d.outcome));
        } else if let Some(note) = &d.manual_note {
            under.push_str(&format!(" · undo by hand: {note}"));
        } else if let Some(s) = d.subline() {
            under.push_str(" · ");
            under.push_str(s);
        }
        c.text(
            &under,
            text_x,
            row.y + 30.0,
            &small,
            if d.succeeded() {
                theme.text_faint
            } else {
                theme.warn
            },
            Some(text_w),
        );

        if let Some(b) = layout.undos[i] {
            c.fill_rounded(b, b.h / 2.0, theme.surface_active);
            let (bw, bh) = c.measure("Undo", &small, None);
            c.text(
                "Undo",
                b.x + (b.w - bw) / 2.0,
                b.y + (b.h - bh) / 2.0,
                &small,
                theme.text,
                None,
            );
        }
    }
    c.restore();

    if layout.max_scroll() > 0.0 {
        // A hairline down the right edge, showing how far through the list this
        // is. Three pixels, like the file view's.
        let track = layout.body;
        let frac = (track.h / layout.content_height).clamp(0.05, 1.0);
        let bar_h = track.h * frac;
        let at = 0.0_f64.max(track.h - bar_h);
        c.fill_rounded(
            Rect::new(track.right() - 3.0, track.y + at * 0.0, 3.0, bar_h),
            1.5,
            theme.hairline,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nous_core::json::parse;

    fn reply(records: &str) -> Json {
        parse(&format!(r#"{{"records":[{records}],"count":1}}"#)).unwrap()
    }

    const MOVE: &str = r#"{"seq":7,"ts":1000,"intent":"tidy my downloads",
        "detail":"moved 84 images into Pictures/2026","capability":"fs.move",
        "risk":"write","outcome":"ok","undo":{"kind":"move_path","from":"/a","to":"/b"}}"#;

    #[test]
    fn a_record_with_a_way_back_offers_one() {
        let d = &read(&reply(MOVE))[0];
        assert!(d.can_undo());
        assert_eq!(
            d.headline(),
            "tidy my downloads",
            "showed a capability string to a person"
        );
        assert_eq!(d.subline(), Some("moved 84 images into Pictures/2026"));
        assert_eq!(d.risk, Risk::Write);
    }

    #[test]
    fn a_record_with_no_way_back_does_not_pretend_to_have_one() {
        // A button that looks live and fails is worse than no button.
        for undo in [r#""undo":null"#, r#""undo":{"kind":"none"}"#] {
            let d = &read(&reply(&format!(
                r#"{{"seq":1,"ts":1,"intent":"read it","capability":"fs.read","risk":"read","outcome":"ok",{undo}}}"#
            )))[0];
            assert!(!d.can_undo(), "offered to undo {undo}");
        }
    }

    #[test]
    fn an_undo_that_needs_a_person_says_so_instead_of_offering_a_button() {
        let d = &read(&reply(
            r#"{"seq":3,"ts":1,"intent":"play the film","capability":"media.play","risk":"write",
                "outcome":"ok","undo":{"kind":"manual","note":"stop playback"}}"#,
        ))[0];
        assert!(
            !d.can_undo(),
            "offered a button for something a person must do"
        );
        assert_eq!(d.manual_note.as_deref(), Some("stop playback"));
    }

    #[test]
    fn something_already_taken_back_cannot_be_taken_back_again() {
        let d = &read(&reply(
            r#"{"seq":7,"ts":1,"intent":"moved it","capability":"fs.move","risk":"write",
                "outcome":"ok","undo":{"kind":"move_path"},"undone_by":9}"#,
        ))[0];
        assert!(!d.can_undo());
        assert_eq!(d.undone_by, Some(9));
    }

    #[test]
    fn a_refusal_is_shown_but_not_undoable() {
        // Someone wondering why nothing happened should find the answer here.
        let d = &read(&reply(
            r#"{"seq":4,"ts":1,"intent":"delete everything","capability":"fs.delete",
                "risk":"elevated","outcome":"refused","undo":{"kind":"move_path"}}"#,
        ))[0];
        assert!(!d.succeeded());
        assert!(
            !d.can_undo(),
            "offered to undo something that never happened"
        );
        assert_eq!(d.headline(), "delete everything");
    }

    #[test]
    fn undo_reaches_the_newest_thing_that_can_still_be_taken_back() {
        // Not simply the newest: pressing undo twice must reach two different
        // actions rather than failing on the same one.
        let deeds = read(&reply(&format!(
            r#"{{"seq":9,"ts":9,"intent":"looked","capability":"fs.read","risk":"read","outcome":"ok","undo":null}},
               {{"seq":8,"ts":8,"intent":"already undone","capability":"fs.move","risk":"write","outcome":"ok","undo":{{"kind":"move_path"}},"undone_by":10}},
               {MOVE}"#
        )));
        let n = newest_undoable(&deeds).expect("something can be taken back");
        assert_eq!(
            n.seq, 7,
            "undo would have failed on a record with no way back"
        );
    }

    #[test]
    fn with_nothing_undoable_undo_finds_nothing_rather_than_guessing() {
        let deeds = read(&reply(
            r#"{"seq":1,"ts":1,"capability":"fs.read","risk":"read","outcome":"ok","undo":null}"#,
        ));
        assert!(newest_undoable(&deeds).is_none());
    }

    #[test]
    fn a_risk_nobody_recognises_is_treated_as_the_worst() {
        let d = &read(&reply(
            r#"{"seq":1,"ts":1,"capability":"x.y","risk":"newfangled","outcome":"ok","undo":null}"#,
        ))[0];
        assert_eq!(
            d.risk,
            Risk::Critical,
            "an unknown risk drawn as a mild one"
        );
    }

    #[test]
    fn an_entry_with_no_words_still_says_what_it_was() {
        let d = &read(&reply(
            r#"{"seq":1,"ts":1,"capability":"fs.mkdir","risk":"write","outcome":"ok","undo":null}"#,
        ))[0];
        assert_eq!(d.headline(), "fs.mkdir");
        assert_eq!(d.subline(), None, "repeated itself on a second line");
    }

    #[test]
    fn times_are_said_the_way_people_say_them() {
        assert_eq!(when(1000, 1000), "just now");
        assert_eq!(when(1000, 1030), "just now");
        assert_eq!(when(1000, 1000 + 60), "1 minute ago");
        assert_eq!(when(1000, 1000 + 600), "10 minutes ago");
        assert_eq!(when(1000, 1000 + 7200), "2 hours ago");
        assert_eq!(when(1000, 1000 + 90_000), "yesterday");
        assert_eq!(when(1000, 1000 + 400_000), "4 days ago");
        // A record from the future — a clock that moved — must not wrap round
        // to "50 years ago".
        assert_eq!(when(9999, 1000), "just now");
    }

    #[test]
    fn the_header_describes_what_is_on_screen() {
        // It said "no daemon — nothing to show" above eight visible entries.
        // A reader made to resolve a contradiction resolves it by trusting
        // neither half.
        use nous_ui::draw::Image;
        let deeds = read(&reply(MOVE));
        let img = Image::new(900, 600).unwrap();
        let l = Layout::compute(&deeds, 900.0, 600.0, 0.0);
        // Drawn with the daemon gone, which is how the ledger looks a moment
        // after it goes away.
        render(&img.canvas(), &deeds, 0, &Theme::dark(), &l, 1200, false);

        let same = Image::new(900, 600).unwrap();
        let l2 = Layout::compute(&deeds, 900.0, 600.0, 0.0);
        render(&same.canvas(), &deeds, 0, &Theme::dark(), &l2, 1200, true);
        let mut differs = 0;
        for y in 0..HEADER_H as i32 {
            for x in 0..900 {
                if img.pixel(x, y) != same.pixel(x, y) {
                    differs += 1;
                }
            }
        }
        assert_eq!(
            differs, 0,
            "the header changed its story about entries that are on screen either way"
        );
    }

    #[test]
    fn an_empty_journal_is_a_sentence_not_a_blank_panel() {
        use nous_ui::draw::Image;
        for connected in [true, false] {
            let img = Image::new(900, 600).unwrap();
            let l = Layout::compute(&[], 900.0, 600.0, 0.0);
            render(&img.canvas(), &[], 0, &Theme::dark(), &l, 0, connected);
            assert!(
                img.variety(l.body) > 2,
                "blank panel when connected={connected}"
            );
        }
    }

    #[test]
    fn the_rows_are_drawn_with_their_risk_and_their_way_back() {
        use nous_ui::draw::Image;
        let deeds = read(&reply(&format!(
            r#"{MOVE},
               {{"seq":6,"ts":900,"intent":"emptied the trash","capability":"fs.delete",
                 "risk":"elevated","outcome":"ok","undo":null}}"#
        )));
        for theme in [Theme::dark(), Theme::light()] {
            let img = Image::new(900, 600).unwrap();
            let l = Layout::compute(&deeds, 900.0, 600.0, 0.0);
            render(&img.canvas(), &deeds, 0, &theme, &l, 1200, true);
            assert!(img.variety(l.rows[0]) > 4, "the first entry is blank");
            // The two rows carry different risk colours, so the spines differ.
            let spine = |r: Rect| img.pixel((r.x + 1.0) as i32, (r.y + r.h / 2.0) as i32);
            assert_ne!(
                spine(l.rows[0]),
                spine(l.rows[1]),
                "a write and a deletion are drawn the same colour"
            );
            // Only the undoable one has a button.
            assert!(l.undos[0].is_some());
            assert!(
                l.undos[1].is_none(),
                "offered a button with nothing behind it"
            );
        }
    }

    #[test]
    fn a_button_scrolled_out_of_sight_cannot_be_pressed() {
        let many: String = (0..40)
            .map(|i| {
                format!(
                    r#"{{"seq":{i},"ts":1,"intent":"move {i}","capability":"fs.move","risk":"write","outcome":"ok","undo":{{"kind":"move_path"}}}},"#
                )
            })
            .collect();
        let deeds = read(&reply(many.trim_end_matches(',')));
        let l = Layout::compute(&deeds, 900.0, 400.0, 0.0);
        let first = l.undos[0].expect("the first row has a button");
        assert_eq!(l.undo_at(first.x + 2.0, first.y + 2.0), Some(0));

        // Scrolled past, the same button is no longer there to press.
        let l = Layout::compute(&deeds, 900.0, 400.0, 300.0);
        let moved = l.undos[0].expect("still laid out");
        assert!(moved.bottom() < l.body.y, "the row is still on screen");
        assert_eq!(
            l.undo_at(moved.x + 2.0, moved.y + 2.0),
            None,
            "pressed a button that is not there"
        );
    }

    #[test]
    fn a_short_window_does_not_produce_a_negative_body() {
        let l = Layout::compute(&[], 400.0, 20.0, 0.0);
        assert!(l.body.h >= 0.0, "body height {}", l.body.h);
        assert_eq!(l.max_scroll(), 0.0);
    }
}
