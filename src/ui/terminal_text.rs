use crate::app::BODY_TEXT_TAB_WIDTH;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub(super) fn wrap_text(text: &str, max_width: u16) -> Vec<String> {
    let max_width = max_width as usize;
    if max_width == 0 || text.is_empty() {
        return vec![text.to_string()];
    }

    let mut lines = Vec::new();
    let mut current = String::new();

    for word in text.split_whitespace() {
        let word_width = text_width(word) as usize;
        if current.is_empty() {
            if word_width <= max_width {
                current.push_str(word);
            } else {
                lines.extend(hard_wrap_text(word, max_width));
            }
            continue;
        }

        let next_width = text_width(&current) as usize + 1 + word_width;
        if next_width <= max_width {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(current);
            current = String::new();
            if word_width <= max_width {
                current.push_str(word);
            } else {
                lines.extend(hard_wrap_text(word, max_width));
            }
        }
    }

    if !current.is_empty() {
        lines.push(current);
    }

    if lines.is_empty() {
        vec![text.to_string()]
    } else {
        lines
    }
}

pub(super) fn hard_wrap_text(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![text.to_string()];
    }

    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0_usize;

    for grapheme in text.graphemes(true) {
        let width = terminal_grapheme_width(grapheme);
        if current_width > 0 && current_width.saturating_add(width) > max_width {
            lines.push(current);
            current = String::new();
            current_width = 0;
        }
        current.push_str(grapheme);
        current_width = current_width.saturating_add(width);
        if current_width >= max_width {
            lines.push(current);
            current = String::new();
            current_width = 0;
        }
    }

    if !current.is_empty() {
        lines.push(current);
    }

    lines
}

pub(super) fn text_width(text: &str) -> u16 {
    text.graphemes(true)
        .fold(0_usize, |width, grapheme| {
            width.saturating_add(terminal_grapheme_width(grapheme))
        })
        .try_into()
        .unwrap_or(u16::MAX)
}

fn terminal_grapheme_width(grapheme: &str) -> usize {
    if grapheme == "\t" {
        BODY_TEXT_TAB_WIDTH
    } else {
        UnicodeWidthStr::width(grapheme)
    }
}

pub(super) fn wrap_cell_text(text: &str, max_width: u16) -> Vec<String> {
    let max_width = max_width as usize;
    if max_width == 0 {
        return vec![String::new()];
    }

    let mut lines = Vec::new();
    for source_line in text.split('\n') {
        let wrapped = hard_wrap_text(source_line, max_width);
        if wrapped.is_empty() {
            lines.push(String::new());
        } else {
            lines.extend(wrapped);
        }
    }

    if lines.is_empty() {
        vec![String::new()]
    } else {
        lines
    }
}

pub(super) fn pad_to_width(text: &str, width: u16) -> String {
    let mut padded = text.to_string();
    let padding = width.saturating_sub(text_width(text)) as usize;
    padded.push_str(&" ".repeat(padding));
    padded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_measurement_handles_wide_combining_emoji_and_tabs() {
        assert_eq!(text_width("界"), 2);
        assert_eq!(text_width("e\u{301}"), 1);
        assert_eq!(text_width("👩‍💻"), 2);
        assert_eq!(text_width("\t"), BODY_TEXT_TAB_WIDTH as u16);

        assert_eq!(hard_wrap_text("界界", 2), ["界", "界"]);
        assert_eq!(
            hard_wrap_text("e\u{301}e\u{301}", 1),
            ["e\u{301}", "e\u{301}"]
        );
        assert_eq!(wrap_cell_text("a\n\n界", 2), ["a", "", "界"]);
    }
}
