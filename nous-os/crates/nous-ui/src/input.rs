//! An editable line of text: caret, selection, and the editing operations.
//!
//! This is the part a browser used to provide. It is pure logic with no
//! drawing and no X11, so every behaviour below is tested directly.
//!
//! Positions are byte offsets into the buffer and are always on a character
//! boundary. Byte offsets are what Pango wants for cursor geometry, so keeping
//! them avoids a conversion on every keystroke; the invariant is maintained by
//! only ever moving the caret through [`Edit::next_boundary`] and
//! [`Edit::prev_boundary`].

/// Where a horizontal move should stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Char,
    Word,
    Line,
}

#[derive(Debug, Clone, Default)]
pub struct Edit {
    text: String,
    caret: usize,
    /// The fixed end of the selection. `None` means nothing is selected.
    anchor: Option<usize>,
}

impl Edit {
    pub fn new() -> Edit {
        Edit::default()
    }

    pub fn from(s: &str) -> Edit {
        Edit {
            text: s.to_string(),
            caret: s.len(),
            anchor: None,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn caret(&self) -> usize {
        self.caret
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// The selected range as `(start, end)` byte offsets, if any.
    pub fn selection(&self) -> Option<(usize, usize)> {
        let a = self.anchor?;
        if a == self.caret {
            None
        } else {
            Some((a.min(self.caret), a.max(self.caret)))
        }
    }

    pub fn selected_text(&self) -> Option<&str> {
        let (s, e) = self.selection()?;
        Some(&self.text[s..e])
    }

    pub fn set(&mut self, s: &str) {
        self.text = s.to_string();
        self.caret = self.text.len();
        self.anchor = None;
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.caret = 0;
        self.anchor = None;
    }

    /// Insert text at the caret, replacing the selection if there is one.
    ///
    /// Control characters are dropped: an input method or a paste can deliver
    /// them, and a newline or a NUL in a single-line field corrupts the value
    /// silently. Tab is dropped too — it moves focus, it is not content.
    pub fn insert(&mut self, s: &str) {
        self.delete_selection();
        let cleaned: String = s.chars().filter(|c| !c.is_control()).collect();
        if cleaned.is_empty() {
            return;
        }
        self.text.insert_str(self.caret, &cleaned);
        self.caret += cleaned.len();
        self.anchor = None;
    }

    /// Delete the selection, or the character before the caret.
    pub fn backspace(&mut self, step: Step) {
        if self.delete_selection() {
            return;
        }
        let to = self.target(self.caret, -1, step);
        self.text.replace_range(to..self.caret, "");
        self.caret = to;
    }

    /// Delete the selection, or the character after the caret.
    pub fn delete(&mut self, step: Step) {
        if self.delete_selection() {
            return;
        }
        let to = self.target(self.caret, 1, step);
        self.text.replace_range(self.caret..to, "");
    }

    /// Move the caret. With `extend`, the selection grows instead.
    pub fn move_caret(&mut self, dir: i32, step: Step, extend: bool) {
        if !extend {
            // Collapsing a selection with a plain arrow puts the caret at the
            // end you moved towards, not wherever it happened to be.
            if let Some((s, e)) = self.selection() {
                self.caret = if dir < 0 { s } else { e };
                self.anchor = None;
                if step == Step::Char {
                    return;
                }
            }
            self.anchor = None;
        } else if self.anchor.is_none() {
            self.anchor = Some(self.caret);
        }
        self.caret = self.target(self.caret, dir, step);
    }

    pub fn select_all(&mut self) {
        self.anchor = Some(0);
        self.caret = self.text.len();
    }

    pub fn deselect(&mut self) {
        self.anchor = None;
    }

    /// Place the caret at a byte offset, snapping to a character boundary.
    /// Used when the pointer clicks into the text.
    pub fn place(&mut self, byte: usize, extend: bool) {
        let mut b = byte.min(self.text.len());
        while b > 0 && !self.text.is_char_boundary(b) {
            b -= 1;
        }
        if extend {
            if self.anchor.is_none() {
                self.anchor = Some(self.caret);
            }
        } else {
            self.anchor = None;
        }
        self.caret = b;
    }

    /// Remove the selection. Returns whether there was one.
    fn delete_selection(&mut self) -> bool {
        let Some((s, e)) = self.selection() else {
            self.anchor = None;
            return false;
        };
        self.text.replace_range(s..e, "");
        self.caret = s;
        self.anchor = None;
        true
    }

    /// Where the caret lands moving `dir` by `step` from `at`.
    fn target(&self, at: usize, dir: i32, step: Step) -> usize {
        match (step, dir < 0) {
            (Step::Line, true) => 0,
            (Step::Line, false) => self.text.len(),
            (Step::Char, true) => self.prev_boundary(at),
            (Step::Char, false) => self.next_boundary(at),
            (Step::Word, true) => {
                // Skip the whitespace immediately behind the caret, then the
                // word itself. Deleting a word should not stop on the space.
                let mut i = at;
                while i > 0 && self.char_before(i).is_some_and(char::is_whitespace) {
                    i = self.prev_boundary(i);
                }
                while i > 0 && self.char_before(i).is_some_and(|c| !c.is_whitespace()) {
                    i = self.prev_boundary(i);
                }
                i
            }
            (Step::Word, false) => {
                let mut i = at;
                let n = self.text.len();
                while i < n && self.char_at(i).is_some_and(|c| !c.is_whitespace()) {
                    i = self.next_boundary(i);
                }
                while i < n && self.char_at(i).is_some_and(char::is_whitespace) {
                    i = self.next_boundary(i);
                }
                i
            }
        }
    }

    fn next_boundary(&self, at: usize) -> usize {
        if at >= self.text.len() {
            return self.text.len();
        }
        let mut i = at + 1;
        while i < self.text.len() && !self.text.is_char_boundary(i) {
            i += 1;
        }
        i
    }

    fn prev_boundary(&self, at: usize) -> usize {
        if at == 0 {
            return 0;
        }
        let mut i = at - 1;
        while i > 0 && !self.text.is_char_boundary(i) {
            i -= 1;
        }
        i
    }

    fn char_at(&self, at: usize) -> Option<char> {
        self.text[at..].chars().next()
    }

    fn char_before(&self, at: usize) -> Option<char> {
        self.text[..at].chars().next_back()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_and_deleting_ascii() {
        let mut e = Edit::new();
        e.insert("tidy");
        assert_eq!(e.text(), "tidy");
        assert_eq!(e.caret(), 4);
        e.backspace(Step::Char);
        assert_eq!(e.text(), "tid");
        e.move_caret(-1, Step::Line, false);
        assert_eq!(e.caret(), 0);
        e.insert("un");
        assert_eq!(e.text(), "untid");
        assert_eq!(e.caret(), 2);
    }

    #[test]
    fn backspace_at_the_start_and_delete_at_the_end_do_nothing() {
        let mut e = Edit::from("x");
        e.move_caret(-1, Step::Line, false);
        e.backspace(Step::Char);
        assert_eq!(
            e.text(),
            "x",
            "backspace at offset 0 must not panic or wrap"
        );
        e.move_caret(1, Step::Line, false);
        e.delete(Step::Char);
        assert_eq!(e.text(), "x", "delete at the end must not panic");
    }

    #[test]
    fn the_caret_never_lands_inside_a_multibyte_character() {
        // "é" is 2 bytes, "→" is 3, "🙂" is 4 — one of each, so a naive ±1
        // would split all three.
        let mut e = Edit::from("aé→🙂b");
        let n = e.text().len();
        assert_eq!(n, 1 + 2 + 3 + 4 + 1);

        e.move_caret(-1, Step::Line, false);
        let mut seen = vec![e.caret()];
        while e.caret() < n {
            e.move_caret(1, Step::Char, false);
            assert!(
                e.text().is_char_boundary(e.caret()),
                "caret at {} splits a char",
                e.caret()
            );
            seen.push(e.caret());
        }
        assert_eq!(seen, vec![0, 1, 3, 6, 10, 11]);

        // And back again, hitting the same boundaries.
        let mut back = vec![e.caret()];
        while e.caret() > 0 {
            e.move_caret(-1, Step::Char, false);
            back.push(e.caret());
        }
        back.reverse();
        assert_eq!(back, seen);
    }

    #[test]
    fn backspace_removes_one_whole_character_not_one_byte() {
        let mut e = Edit::from("a🙂");
        e.backspace(Step::Char);
        assert_eq!(e.text(), "a", "the whole emoji goes, leaving valid UTF-8");
        let mut e = Edit::from("café");
        e.backspace(Step::Char);
        assert_eq!(e.text(), "caf");
    }

    #[test]
    fn word_motion_skips_the_space_with_the_word() {
        let mut e = Edit::from("tidy my downloads");
        e.backspace(Step::Word);
        assert_eq!(e.text(), "tidy my ");
        e.backspace(Step::Word);
        assert_eq!(e.text(), "tidy ");
        e.backspace(Step::Word);
        assert_eq!(e.text(), "");
        // On an empty buffer it must stop rather than loop.
        e.backspace(Step::Word);
        assert_eq!(e.text(), "");
    }

    #[test]
    fn forward_word_motion_lands_at_the_start_of_the_next_word() {
        let mut e = Edit::from("tidy my downloads");
        e.move_caret(-1, Step::Line, false);
        e.move_caret(1, Step::Word, false);
        assert_eq!(e.caret(), 5, "past 'tidy' and its space");
        e.move_caret(1, Step::Word, false);
        assert_eq!(e.caret(), 8);
        e.move_caret(1, Step::Word, false);
        assert_eq!(e.caret(), 17, "the last word ends at the end of the line");
        e.move_caret(1, Step::Word, false);
        assert_eq!(e.caret(), 17, "and stays there");
    }

    #[test]
    fn selecting_then_typing_replaces_the_selection() {
        let mut e = Edit::from("tidy my downloads");
        e.select_all();
        assert_eq!(e.selected_text(), Some("tidy my downloads"));
        e.insert("open notes");
        assert_eq!(e.text(), "open notes");
        assert_eq!(e.selection(), None);
        assert_eq!(e.caret(), 10);
    }

    #[test]
    fn selecting_then_backspacing_removes_only_the_selection() {
        let mut e = Edit::from("tidy my downloads");
        e.move_caret(-1, Step::Line, false);
        e.move_caret(1, Step::Word, true);
        assert_eq!(e.selected_text(), Some("tidy "));
        e.backspace(Step::Char);
        assert_eq!(
            e.text(),
            "my downloads",
            "backspace ate the selection, not one extra char"
        );
        assert_eq!(e.caret(), 0);
    }

    #[test]
    fn a_plain_arrow_collapses_the_selection_to_the_side_it_moved() {
        let mut e = Edit::from("abcdef");
        e.place(2, false);
        e.move_caret(1, Step::Char, true);
        e.move_caret(1, Step::Char, true);
        assert_eq!(e.selection(), Some((2, 4)));

        e.move_caret(-1, Step::Char, false);
        assert_eq!(e.caret(), 2, "left collapses to the selection start");
        assert_eq!(e.selection(), None);

        e.place(2, false);
        e.move_caret(1, Step::Char, true);
        e.move_caret(1, Step::Char, true);
        e.move_caret(1, Step::Char, false);
        assert_eq!(e.caret(), 4, "right collapses to the selection end");
    }

    #[test]
    fn a_zero_width_selection_is_no_selection() {
        let mut e = Edit::from("abc");
        e.move_caret(-1, Step::Char, true);
        e.move_caret(1, Step::Char, true);
        assert_eq!(e.selection(), None, "anchor and caret coincide");
        assert_eq!(e.selected_text(), None);
    }

    #[test]
    fn control_characters_are_never_inserted() {
        let mut e = Edit::new();
        // A paste from a terminal, or an input method flushing a newline.
        e.insert("open\nnotes\ttoday\r\x00");
        assert_eq!(e.text(), "opennotestoday");
        // Text that is nothing but control characters must leave the caret
        // alone rather than silently advancing it.
        let before = e.caret();
        e.insert("\n\n");
        assert_eq!(e.caret(), before);
    }

    #[test]
    fn clicking_into_text_snaps_to_a_character_boundary() {
        let mut e = Edit::from("a🙂b");
        // Byte 2 is inside the emoji (which occupies 1..5).
        e.place(2, false);
        assert_eq!(e.caret(), 1, "snapped back to the start of the emoji");
        assert!(e.text().is_char_boundary(e.caret()));
        e.place(999, false);
        assert_eq!(
            e.caret(),
            e.text().len(),
            "a click past the end goes to the end"
        );
    }

    #[test]
    fn shift_click_extends_from_the_existing_caret() {
        let mut e = Edit::from("abcdef");
        e.place(1, false);
        e.place(4, true);
        assert_eq!(e.selection(), Some((1, 4)));
        assert_eq!(e.selected_text(), Some("bcd"));
    }
}
