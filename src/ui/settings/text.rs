pub(super) fn fit_input_value(value: &str, width: usize) -> String {
    let value_len = value.chars().count();
    if value_len <= width {
        return format!("{value:<width$}");
    }

    if width == 1 {
        return "…".to_string();
    }

    let mut clipped = value.chars().take(width - 1).collect::<String>();
    clipped.push('…');
    clipped
}
