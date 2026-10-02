use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::{Modifier, Style},
    widgets::Widget,
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Single-line presentation. Cursor and mouse targets use UTF-8 byte offsets.
pub(crate) struct TextInput<'a> {
    value: &'a str,
    cursor: Option<usize>,
    padding: usize,
    tab_width: usize,
    style: Style,
}

impl<'a> TextInput<'a> {
    pub(crate) fn new(value: &'a str, cursor: Option<usize>) -> Self {
        Self {
            value,
            cursor,
            padding: 0,
            tab_width: 4,
            style: Style::default(),
        }
    }

    pub(crate) fn padding(mut self, padding: usize) -> Self {
        self.padding = padding;
        self
    }

    pub(crate) fn style(mut self, style: Style) -> Self {
        self.style = style;
        self
    }

    pub(crate) fn tab_width(mut self, tab_width: usize) -> Self {
        self.tab_width = tab_width;
        self
    }

    pub(crate) fn render_with_cursor_map(self, area: Rect, buf: &mut Buffer) -> InputCursorMap {
        let layout = InputLayout::new(&self, area);
        layout.render(self.style, buf);
        layout.cursor_map()
    }
}

impl Widget for TextInput<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        InputLayout::new(&self, area).render(self.style, buf);
    }
}

// Rendering and hit testing share one grapheme-aligned horizontal viewport.
// The retained map contains visible columns, never a copy of the input text.
struct InputLayout<'a> {
    value: &'a str,
    area: Rect,
    start_byte: usize,
    padding: usize,
    cursor: Option<usize>,
    tab_width: usize,
}

impl<'a> InputLayout<'a> {
    fn new(input: &TextInput<'a>, area: Rect) -> Self {
        let area = Rect::new(area.x, area.y, area.width, area.height.min(1));
        let mut layout = Self {
            value: input.value,
            area,
            start_byte: 0,
            padding: input.padding,
            cursor: input.cursor.map(|cursor| cursor.min(input.value.len())),
            tab_width: input.tab_width,
        };
        if area.is_empty() {
            return layout;
        }
        let width = usize::from(area.width);
        let mut cursor_column = input.padding;
        let mut cursor_width = 1;
        for (byte, grapheme) in input.value.grapheme_indices(true) {
            if byte + grapheme.len() > layout.cursor.unwrap_or(0) {
                cursor_width = layout.grapheme_width(grapheme).max(1);
                break;
            }
            cursor_column = cursor_column.saturating_add(layout.grapheme_width(grapheme));
        }
        let mut skip = cursor_column.saturating_sub(width.saturating_sub(cursor_width));
        if layout.cursor.is_none() {
            skip = 0;
        }
        layout.padding = input.padding.saturating_sub(skip).min(width);
        skip = skip.saturating_sub(input.padding);
        for (byte, grapheme) in input.value.grapheme_indices(true) {
            if skip == 0 {
                break;
            }
            skip = skip.saturating_sub(layout.grapheme_width(grapheme));
            layout.start_byte = byte + grapheme.len();
        }
        layout
    }

    fn grapheme_width(&self, grapheme: &str) -> usize {
        if grapheme == "\t" {
            self.tab_width
        } else {
            UnicodeWidthStr::width(grapheme)
        }
    }

    fn cursor_map(&self) -> InputCursorMap {
        let mut columns = vec![
            0;
            if self.area.is_empty() {
                0
            } else {
                usize::from(self.area.width)
            }
        ];
        let mut x = self.padding.min(columns.len());
        let mut index = self.start_byte;
        for grapheme in self.value[self.start_byte..].graphemes(true) {
            if x == columns.len() {
                break;
            }
            let width = self.grapheme_width(grapheme);
            if width > columns.len() - x {
                // A clipped cursor wider than the input gets a cursor blank;
                // other clipped text maps to the end without exposing half a glyph.
                let cursor_clipped = width > columns.len()
                    && self
                        .cursor
                        .is_some_and(|cursor| cursor >= index && cursor < index + grapheme.len());
                columns[x..].fill(if cursor_clipped {
                    index
                } else {
                    self.value.len()
                });
                return InputCursorMap {
                    area: self.area,
                    columns,
                };
            }
            columns[x..x + width].fill(index);
            x += width;
            index += grapheme.len();
        }
        columns[x..].fill(index);
        InputCursorMap {
            area: self.area,
            columns,
        }
    }

    fn render(&self, style: Style, buf: &mut Buffer) {
        if self.area.is_empty() {
            return;
        }
        buf.set_style(self.area, style);
        let mut x = self.area.x.saturating_add(self.padding as u16);
        let mut index = self.start_byte;
        for grapheme in self.value[self.start_byte..].graphemes(true) {
            let width = self.grapheme_width(grapheme);
            if width > usize::from(self.area.right().saturating_sub(x)) {
                if width > usize::from(self.area.width)
                    && self
                        .cursor
                        .is_some_and(|cursor| cursor >= index && cursor < index + grapheme.len())
                    && x < self.area.right()
                {
                    buf[(x, self.area.y)].set_style(style.add_modifier(Modifier::REVERSED));
                }
                return;
            }
            let selected = self
                .cursor
                .is_some_and(|cursor| cursor >= index && cursor < index + grapheme.len());
            let style = if selected {
                style.add_modifier(Modifier::REVERSED)
            } else {
                style
            };
            if grapheme == "\t" {
                for column in x..x.saturating_add(width as u16) {
                    buf[(column, self.area.y)].set_symbol(" ").set_style(style);
                }
            } else if width > 0 {
                buf.set_stringn(x, self.area.y, grapheme, width, style);
            }
            x = x.saturating_add(width as u16);
            index += grapheme.len();
        }
        if self.cursor == Some(index) && x < self.area.right() {
            buf[(x, self.area.y)].set_style(style.add_modifier(Modifier::REVERSED));
        }
    }
}

pub(crate) struct InputCursorMap {
    area: Rect,
    columns: Vec<usize>,
}

impl InputCursorMap {
    pub(crate) fn area(&self) -> Rect {
        self.area
    }

    pub(crate) fn cursor_at(&self, position: Position) -> Option<usize> {
        self.area
            .contains(position)
            .then(|| self.columns[usize::from(position.x - self.area.x)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text_input::TextInputState;

    #[test]
    fn wide_and_combined_characters_share_their_start_cursor() {
        let area = Rect::new(4, 2, 8, 1);
        let mut buffer = Buffer::empty(area);
        let map = TextInput::new("界e\u{301}z", None)
            .padding(1)
            .render_with_cursor_map(area, &mut buffer);
        assert_eq!(map.cursor_at(Position::new(5, 2)), Some(0));
        assert_eq!(map.cursor_at(Position::new(6, 2)), Some(0));
        assert_eq!(map.cursor_at(Position::new(7, 2)), Some("界".len()));
        assert_eq!(map.cursor_at(Position::new(8, 2)), Some("界e\u{301}".len()));
        assert_eq!(
            map.cursor_at(Position::new(11, 2)),
            Some("界e\u{301}z".len())
        );
        assert_eq!(map.cursor_at(Position::new(12, 2)), None);
    }

    #[test]
    fn cursor_on_wide_grapheme_scrolls_the_entire_character_into_view() {
        let area = Rect::new(0, 0, 5, 1);
        let mut buffer = Buffer::empty(area);
        let map = TextInput::new("abc界z", Some(3))
            .padding(1)
            .render_with_cursor_map(area, &mut buffer);
        assert_eq!(buffer[(3, 0)].symbol(), "界");
        assert!(buffer[(3, 0)].modifier.contains(Modifier::REVERSED));
        assert_eq!(map.cursor_at(Position::new(3, 0)), Some(3));
        assert_eq!(map.cursor_at(Position::new(4, 0)), Some(3));
    }

    #[test]
    fn narrower_than_cursor_grapheme_keeps_an_unsplit_cursor_target() {
        let area = Rect::new(0, 0, 1, 1);
        let mut buffer = Buffer::empty(area);
        let map = TextInput::new("a界z", Some(1))
            .padding(1)
            .render_with_cursor_map(area, &mut buffer);
        assert_eq!(buffer[(0, 0)].symbol(), " ");
        assert!(buffer[(0, 0)].modifier.contains(Modifier::REVERSED));
        assert_eq!(map.cursor_at(Position::new(0, 0)), Some(1));
    }

    #[test]
    fn clipping_never_renders_or_targets_half_a_wide_character() {
        let area = Rect::new(0, 0, 2, 1);
        let mut buffer = Buffer::empty(area);
        let map = TextInput::new("a界", None).render_with_cursor_map(area, &mut buffer);
        assert_eq!(buffer[(0, 0)].symbol(), "a");
        assert_eq!(buffer[(1, 0)].symbol(), " ");
        assert_eq!(map.cursor_at(Position::new(1, 0)), Some("a界".len()));
        assert_eq!(map.cursor_at(Position::new(2, 0)), None);
    }

    #[test]
    fn scrolled_input_maps_the_same_graphemes_it_renders() {
        let area = Rect::new(0, 0, 5, 1);
        let value = "abc界e\u{301}z";
        let mut buffer = Buffer::empty(area);
        let map = TextInput::new(value, Some(value.len()))
            .padding(1)
            .render_with_cursor_map(area, &mut buffer);
        assert_eq!(buffer[(0, 0)].symbol(), "界");
        assert_eq!(map.cursor_at(Position::new(0, 0)), Some(3));
        assert_eq!(map.cursor_at(Position::new(1, 0)), Some(3));
        assert_eq!(map.cursor_at(Position::new(2, 0)), Some("abc界".len()));
        assert_eq!(map.cursor_at(Position::new(4, 0)), Some(value.len()));
    }

    #[test]
    fn scrolled_mouse_insertion_uses_the_visible_grapheme_boundary() {
        let mut input = TextInputState::new("ab界cd".into());
        let area = Rect::new(0, 0, 4, 1);
        let mut buffer = Buffer::empty(area);
        let map = TextInput::new(input.text(), Some(input.cursor()))
            .render_with_cursor_map(area, &mut buffer);
        assert_eq!(buffer[(0, 0)].symbol(), "c");
        input.set_cursor(map.cursor_at(Position::ORIGIN).unwrap());
        input.insert_char('X');
        assert_eq!(input.text(), "ab界Xcd");
        input.move_left();
        input.backspace();
        assert_eq!(input.text(), "abXcd");
    }
}
