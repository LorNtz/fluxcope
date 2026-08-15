use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub(crate) const MAX_SINGLE_LINE_INPUT_BYTES: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InputEditOutcome {
    Changed,
    Unchanged,
    LimitReached,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SingleLineInput {
    text: String,
    cursor: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InputViewport<'a> {
    pub before_cursor: &'a str,
    pub cursor: &'a str,
    pub after_cursor: &'a str,
    pub start: usize,
}

impl SingleLineInput {
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    #[cfg(test)]
    pub(crate) fn cursor(&self) -> usize {
        self.cursor
    }

    pub(crate) fn insert_char(&mut self, character: char) -> InputEditOutcome {
        if character.is_control() {
            return InputEditOutcome::Unchanged;
        }
        let mut encoded = [0_u8; 4];
        self.insert_atom(character.encode_utf8(&mut encoded))
    }

    pub(crate) fn paste(&mut self, pasted: &str) -> InputEditOutcome {
        let remaining = MAX_SINGLE_LINE_INPUT_BYTES.saturating_sub(self.text.len());
        let mut sanitized = String::with_capacity(pasted.len().min(remaining));
        for character in pasted.chars().filter(|character| !character.is_control()) {
            if sanitized.len().saturating_add(character.len_utf8()) > remaining {
                return InputEditOutcome::LimitReached;
            }
            sanitized.push(character);
        }
        if sanitized.is_empty() {
            InputEditOutcome::Unchanged
        } else {
            self.insert_atom(&sanitized)
        }
    }

    fn insert_atom(&mut self, value: &str) -> InputEditOutcome {
        if self.text.len().saturating_add(value.len()) > MAX_SINGLE_LINE_INPUT_BYTES {
            return InputEditOutcome::LimitReached;
        }
        self.text.insert_str(self.cursor, value);
        self.cursor += value.len();
        self.snap_cursor_forward();
        InputEditOutcome::Changed
    }

    pub(crate) fn move_left(&mut self) -> bool {
        let Some((start, _)) = self.text[..self.cursor].grapheme_indices(true).next_back() else {
            return false;
        };
        self.cursor = start;
        true
    }

    pub(crate) fn move_right(&mut self) -> bool {
        let Some(grapheme) = self.text[self.cursor..].graphemes(true).next() else {
            return false;
        };
        self.cursor += grapheme.len();
        true
    }

    pub(crate) fn move_home(&mut self) -> bool {
        let changed = self.cursor != 0;
        self.cursor = 0;
        changed
    }

    pub(crate) fn move_end(&mut self) -> bool {
        let changed = self.cursor != self.text.len();
        self.cursor = self.text.len();
        changed
    }

    pub(crate) fn backspace(&mut self) -> InputEditOutcome {
        let Some((start, _)) = self.text[..self.cursor].grapheme_indices(true).next_back() else {
            return InputEditOutcome::Unchanged;
        };
        self.text.drain(start..self.cursor);
        self.cursor = start;
        self.snap_cursor_forward();
        InputEditOutcome::Changed
    }

    pub(crate) fn delete(&mut self) -> InputEditOutcome {
        let Some(grapheme) = self.text[self.cursor..].graphemes(true).next() else {
            return InputEditOutcome::Unchanged;
        };
        self.text.drain(self.cursor..self.cursor + grapheme.len());
        self.snap_cursor_forward();
        InputEditOutcome::Changed
    }

    fn snap_cursor_forward(&mut self) {
        if self.cursor == self.text.len() {
            return;
        }
        self.cursor = self
            .text
            .grapheme_indices(true)
            .map(|(start, _)| start)
            .find(|start| *start >= self.cursor)
            .unwrap_or(self.text.len());
    }

    pub(crate) fn viewport(&self, width: usize) -> InputViewport<'_> {
        if width == 0 {
            return InputViewport {
                before_cursor: "",
                cursor: "",
                after_cursor: "",
                start: self.cursor,
            };
        }

        let cursor_grapheme = self.text[self.cursor..]
            .graphemes(true)
            .next()
            .unwrap_or(" ");
        let cursor_width = UnicodeWidthStr::width(cursor_grapheme).max(1).min(width);
        let available_before = width.saturating_sub(cursor_width);
        let mut start = self.cursor;
        let mut before_width = 0_usize;
        for (grapheme_start, grapheme) in self.text[..self.cursor].grapheme_indices(true).rev() {
            let grapheme_width = UnicodeWidthStr::width(grapheme);
            if before_width.saturating_add(grapheme_width) > available_before {
                break;
            }
            before_width = before_width.saturating_add(grapheme_width);
            start = grapheme_start;
        }

        let cursor_end = if self.cursor < self.text.len() {
            self.cursor + cursor_grapheme.len()
        } else {
            self.cursor
        };
        let mut end = cursor_end;
        let mut used_width = before_width.saturating_add(cursor_width);
        for (relative_start, grapheme) in self.text[cursor_end..].grapheme_indices(true) {
            let grapheme_width = UnicodeWidthStr::width(grapheme);
            if used_width.saturating_add(grapheme_width) > width {
                break;
            }
            used_width = used_width.saturating_add(grapheme_width);
            end = cursor_end + relative_start + grapheme.len();
        }

        InputViewport {
            before_cursor: &self.text[start..self.cursor],
            cursor: if self.cursor < self.text.len() {
                &self.text[self.cursor..cursor_end]
            } else {
                " "
            },
            after_cursor: &self.text[cursor_end..end],
            start,
        }
    }

    pub(crate) fn set_cursor_from_column(&mut self, width: usize, column: usize) -> bool {
        let viewport = self.viewport(width);
        let visible_start = viewport.start;
        let visible_end = visible_start
            + viewport.before_cursor.len()
            + if self.cursor < self.text.len() {
                viewport.cursor.len()
            } else {
                0
            }
            + viewport.after_cursor.len();
        let mut display_column = 0_usize;

        for (relative_start, grapheme) in
            self.text[visible_start..visible_end].grapheme_indices(true)
        {
            let grapheme_width = UnicodeWidthStr::width(grapheme).max(1);
            let midpoint = display_column.saturating_add(grapheme_width / 2);
            let new_cursor = if column <= midpoint {
                visible_start + relative_start
            } else {
                visible_start + relative_start + grapheme.len()
            };
            if column < display_column.saturating_add(grapheme_width) {
                let changed = self.cursor != new_cursor;
                self.cursor = new_cursor;
                return changed;
            }
            display_column = display_column.saturating_add(grapheme_width);
        }

        let changed = self.cursor != visible_end;
        self.cursor = visible_end;
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn movement_and_deletion_use_grapheme_boundaries() {
        let mut input = SingleLineInput::default();
        assert_eq!(input.paste("a👨‍👩‍👧b"), InputEditOutcome::Changed);
        assert!(input.move_left());
        assert_eq!(input.backspace(), InputEditOutcome::Changed);
        assert_eq!(input.text(), "ab");
        assert_eq!(input.cursor(), 1);
        assert_eq!(input.delete(), InputEditOutcome::Changed);
        assert_eq!(input.text(), "a");
    }

    #[test]
    fn paste_discards_controls_and_overflow_is_atomic() {
        let mut input = SingleLineInput::default();
        assert_eq!(input.paste("ab\ncd\t\u{7}"), InputEditOutcome::Changed);
        assert_eq!(input.text(), "abcd");

        let before = input.clone();
        assert_eq!(
            input.paste(&"x".repeat(MAX_SINGLE_LINE_INPUT_BYTES)),
            InputEditOutcome::LimitReached
        );
        assert_eq!(input, before);
    }

    #[test]
    fn viewport_keeps_cursor_visible_and_click_moves_at_grapheme_boundaries() {
        let mut input = SingleLineInput::default();
        input.paste("ab界cd");
        let viewport = input.viewport(4);
        assert_eq!(viewport.before_cursor, "cd");
        assert_eq!(viewport.cursor, " ");

        assert!(input.set_cursor_from_column(4, 0));
        assert_eq!(input.cursor(), "ab界".len());
        assert!(input.move_left());
        assert_eq!(input.cursor(), 2);
    }

    #[test]
    fn home_end_and_forward_delete_preserve_graphemes() {
        let mut input = SingleLineInput::default();
        input.paste("a界e\u{301}z");
        assert!(input.move_home());
        assert!(!input.move_left());
        assert!(input.move_right());
        assert_eq!(input.delete(), InputEditOutcome::Changed);
        assert_eq!(input.text(), "ae\u{301}z");
        assert!(input.move_end());
        assert!(!input.move_right());
        assert_eq!(input.backspace(), InputEditOutcome::Changed);
        assert_eq!(input.text(), "ae\u{301}");
    }

    #[test]
    fn multibyte_grapheme_overflow_is_rejected_as_one_atom() {
        let mut input = SingleLineInput::default();
        input.paste(&"x".repeat(MAX_SINGLE_LINE_INPUT_BYTES - 2));
        let before = input.clone();

        assert_eq!(input.paste("界"), InputEditOutcome::LimitReached);
        assert_eq!(input, before);
    }

    #[test]
    fn deletion_snaps_cursor_when_neighboring_scalars_form_one_grapheme() {
        let mut input = SingleLineInput::default();
        input.paste("🇦X🇧");
        input.move_home();
        input.move_right();

        assert_eq!(input.delete(), InputEditOutcome::Changed);
        assert_eq!(input.text(), "🇦🇧");
        assert_eq!(input.cursor(), input.text().len());
    }
}
