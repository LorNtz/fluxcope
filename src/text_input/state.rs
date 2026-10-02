use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_segmentation::UnicodeSegmentation;

/// Describes text changes; cursor-only movement leaves the text unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InputEditOutcome {
    Changed,
    Unchanged,
    LimitReached,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct TextInputState {
    text: String,
    // UTF-8 byte offset at an extended grapheme boundary.
    cursor: usize,
    max_bytes: Option<usize>,
}

impl TextInputState {
    pub(crate) fn new(text: String) -> Self {
        Self {
            cursor: text.len(),
            text,
            max_bytes: None,
        }
    }

    pub(crate) fn with_max_bytes(max_bytes: usize) -> Self {
        Self {
            max_bytes: Some(max_bytes),
            ..Self::default()
        }
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    pub(crate) fn cursor(&self) -> usize {
        self.cursor
    }

    pub(crate) fn into_text(self) -> String {
        self.text
    }

    pub(crate) fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    /// Clamp to the preceding grapheme boundary, or the end of the text.
    pub(crate) fn set_cursor(&mut self, cursor: usize) -> bool {
        let cursor = if cursor >= self.text.len() {
            self.text.len()
        } else {
            self.text
                .grapheme_indices(true)
                .map(|(start, _)| start)
                .take_while(|&start| start <= cursor)
                .last()
                .unwrap_or(0)
        };
        let changed = self.cursor != cursor;
        self.cursor = cursor;
        changed
    }

    pub(crate) fn insert_char(&mut self, character: char) -> InputEditOutcome {
        if character.is_control() {
            return InputEditOutcome::Unchanged;
        }
        if self.exceeds_limit(character.len_utf8()) {
            return InputEditOutcome::LimitReached;
        }
        let mut encoded = [0_u8; 4];
        self.insert_text(character.encode_utf8(&mut encoded));
        InputEditOutcome::Changed
    }

    pub(crate) fn paste(&mut self, pasted: &str) -> InputEditOutcome {
        let remaining = self
            .max_bytes
            .map_or(usize::MAX, |limit| limit.saturating_sub(self.text.len()));
        let mut bytes = 0;
        for character in pasted.chars().filter(|ch| !ch.is_control()) {
            bytes += character.len_utf8();
            if bytes > remaining {
                return InputEditOutcome::LimitReached;
            }
        }
        if bytes == 0 {
            return InputEditOutcome::Unchanged;
        }
        if bytes == pasted.len() {
            self.insert_text(pasted);
        } else {
            let mut filtered = String::with_capacity(bytes);
            filtered.extend(pasted.chars().filter(|ch| !ch.is_control()));
            self.insert_text(&filtered);
        }
        InputEditOutcome::Changed
    }

    fn exceeds_limit(&self, bytes: usize) -> bool {
        self.max_bytes
            .is_some_and(|limit| bytes > limit.saturating_sub(self.text.len()))
    }

    fn insert_text(&mut self, text: &str) {
        self.text.insert_str(self.cursor, text);
        self.cursor += text.len();
        self.snap_cursor_forward();
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
            .find(|&start| start >= self.cursor)
            .unwrap_or(self.text.len());
    }

    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> InputEditOutcome {
        match key.code {
            KeyCode::Char(ch)
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                return self.insert_char(ch);
            }
            KeyCode::Backspace if key.modifiers.is_empty() => return self.backspace(),
            KeyCode::Delete if key.modifiers.is_empty() => return self.delete(),
            KeyCode::Left if key.modifiers.is_empty() => {
                self.move_left();
            }
            KeyCode::Right if key.modifiers.is_empty() => {
                self.move_right();
            }
            KeyCode::Home if key.modifiers.is_empty() => {
                self.move_home();
            }
            KeyCode::End if key.modifiers.is_empty() => {
                self.move_end();
            }
            _ => {}
        }
        InputEditOutcome::Unchanged
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn movement_and_deletion_use_grapheme_boundaries() {
        let mut input = TextInputState::default();
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
        let mut input = TextInputState::with_max_bytes(8);
        assert_eq!(input.paste("ab\ncd\t\u{7}"), InputEditOutcome::Changed);
        assert_eq!(input.text(), "abcd");
        let before = input.clone();
        assert_eq!(input.paste("界界"), InputEditOutcome::LimitReached);
        assert_eq!(input, before);
        assert_eq!(input.paste("\n\t界x"), InputEditOutcome::Changed);
        assert_eq!(input.text(), "abcd界x");
    }

    #[test]
    fn home_end_and_forward_delete_preserve_graphemes() {
        let mut input = TextInputState::default();
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
        let mut input = TextInputState::with_max_bytes(4);
        input.paste("xx");
        let before = input.clone();
        assert_eq!(input.paste("界"), InputEditOutcome::LimitReached);
        assert_eq!(input, before);
    }

    #[test]
    fn deletion_snaps_cursor_when_neighboring_scalars_form_one_grapheme() {
        let mut input = TextInputState::new("🇦X🇧".into());
        input.move_home();
        input.move_right();
        assert_eq!(input.delete(), InputEditOutcome::Changed);
        assert_eq!(input.text(), "🇦🇧");
        assert_eq!(input.cursor(), input.text().len());
    }
}
