//! Terminal text uses display-cell widths rather than bytes or scalar-value counts.
//!
//! Extended grapheme clusters are indivisible, wide and joined emoji occupy their
//! Unicode terminal width, combining marks stay attached to their base grapheme,
//! and tabs use the same fixed width as body rendering when measured. Fitting callers
//! select a tab width explicitly; tabs are expanded before clipping so the measured
//! and rendered widths agree. Ellipsis clipping reserves one cell for `…`;
//! exact-width fitting pads any unused cells after clipping.

use crate::app::BODY_TEXT_TAB_WIDTH;
use std::borrow::Cow;
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
    terminal_text_width(text).try_into().unwrap_or(u16::MAX)
}

pub(super) fn text_width_bounded(text: &str, limit: usize) -> usize {
    if limit == 0 {
        return 0;
    }
    let mut width = 0_usize;
    for grapheme in text.graphemes(true) {
        width = width.saturating_add(terminal_grapheme_width(grapheme));
        if width >= limit {
            return limit;
        }
    }
    width
}

fn terminal_text_width(text: &str) -> usize {
    text.graphemes(true).fold(0_usize, |width, grapheme| {
        width.saturating_add(terminal_grapheme_width(grapheme))
    })
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

struct FittedText<'a> {
    text: Cow<'a, str>,
    cell_width: usize,
}

fn fitted_text(value: &str, width: usize, tab_width: usize) -> FittedText<'_> {
    if width == 0 {
        return FittedText {
            text: Cow::Borrowed(""),
            cell_width: 0,
        };
    }

    let ellipsis_width = terminal_grapheme_width("…");
    let max_prefix_width = width.saturating_sub(ellipsis_width);
    let mut value_width = 0_usize;
    let mut prefix_input_end = 0_usize;
    let mut prefix_output_end = 0_usize;
    let mut prefix_width = 0_usize;
    let mut output = None::<String>;
    for (start, grapheme) in value.grapheme_indices(true) {
        if grapheme == "\t" {
            let first_tab = output.is_none();
            let expanded = output.get_or_insert_with(|| {
                let mut expanded = String::with_capacity(value.len().min(width));
                expanded.push_str(&value[..start]);
                expanded
            });
            if first_tab {
                prefix_output_end = prefix_input_end;
            }
            expanded.reserve(tab_width.min(width.saturating_sub(value_width)));
            for _ in 0..tab_width {
                if value_width >= width {
                    expanded.truncate(prefix_output_end);
                    expanded.push('…');
                    return FittedText {
                        text: Cow::Owned(std::mem::take(expanded)),
                        cell_width: prefix_width.saturating_add(ellipsis_width),
                    };
                }
                expanded.push(' ');
                value_width = value_width.saturating_add(1);
                if value_width <= max_prefix_width {
                    prefix_output_end = expanded.len();
                    prefix_width = value_width;
                }
            }
            continue;
        }

        let grapheme_width = terminal_grapheme_width(grapheme);
        let next_width = value_width.saturating_add(grapheme_width);
        if next_width > width {
            let mut clipped = match output {
                Some(mut expanded) => {
                    expanded.truncate(prefix_output_end);
                    expanded
                }
                None => String::from(&value[..prefix_input_end]),
            };
            clipped.reserve('…'.len_utf8());
            clipped.push('…');
            return FittedText {
                text: Cow::Owned(clipped),
                cell_width: prefix_width.saturating_add(ellipsis_width),
            };
        }

        if let Some(expanded) = output.as_mut() {
            expanded.push_str(grapheme);
        }
        value_width = next_width;
        if value_width <= max_prefix_width {
            prefix_input_end = start.saturating_add(grapheme.len());
            if let Some(expanded) = output.as_ref() {
                prefix_output_end = expanded.len();
            }
            prefix_width = value_width;
        }
    }

    FittedText {
        text: output.map_or(Cow::Borrowed(value), Cow::Owned),
        cell_width: value_width,
    }
}

pub(super) fn fit_text(value: &str, width: usize, tab_width: usize) -> Cow<'_, str> {
    fitted_text(value, width, tab_width).text
}

pub(super) fn fit_text_to_width(value: &str, width: usize, tab_width: usize) -> String {
    let fitted = fitted_text(value, width, tab_width);
    let padding = width.saturating_sub(fitted.cell_width);
    let mut padded = fitted.text.into_owned();
    padded.reserve(padding);
    for _ in 0..padding {
        padded.push(' ');
    }
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

    #[test]
    fn ellipsis_clipping_uses_terminal_cells_without_splitting_graphemes() {
        assert_eq!(fit_text("界ab", 3, BODY_TEXT_TAB_WIDTH), "界…");
        assert_eq!(fit_text("👩‍💻ab", 3, BODY_TEXT_TAB_WIDTH), "👩‍💻…");
        assert_eq!(fit_text("e\u{301}xy", 2, BODY_TEXT_TAB_WIDTH), "e\u{301}…");
        assert_eq!(fit_text("界a", 2, BODY_TEXT_TAB_WIDTH), "…");
        assert_eq!(fit_text("a\tb", 3, 2), "a …");
        assert_eq!(fit_text("a\tb\tc", 5, 2), "a  b…");
    }

    #[test]
    fn exact_width_fitting_handles_zero_one_and_unused_cells() {
        assert_eq!(fit_text_to_width("abc", 0, BODY_TEXT_TAB_WIDTH), "");
        assert_eq!(fit_text_to_width("abc", 1, BODY_TEXT_TAB_WIDTH), "…");
        assert_eq!(fit_text_to_width("界a", 2, BODY_TEXT_TAB_WIDTH), "… ");
        assert_eq!(
            fit_text_to_width("e\u{301}", 2, BODY_TEXT_TAB_WIDTH),
            "e\u{301} "
        );
        assert_eq!(fit_text_to_width("\t", 3, 2), "   ");
    }
}
