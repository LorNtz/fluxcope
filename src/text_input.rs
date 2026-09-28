pub(crate) fn byte_index_for_char(value: &str, char_index: usize) -> usize {
    value
        .char_indices()
        .nth(char_index)
        .map_or(value.len(), |(index, _)| index)
}

/// Inserts non-control text at a cursor counted in Unicode scalar values.
pub(crate) fn paste_text_value(pasted: &str, value: &mut String, cursor: &mut usize) -> bool {
    let (characters, bytes) = pasted
        .chars()
        .filter(|ch| !ch.is_control())
        .fold((0, 0), |(characters, bytes), ch| {
            (characters + 1, bytes + ch.len_utf8())
        });
    if characters == 0 {
        return false;
    }

    let mut filtered;
    let text = if bytes == pasted.len() {
        pasted
    } else {
        filtered = String::with_capacity(bytes);
        filtered.extend(pasted.chars().filter(|ch| !ch.is_control()));
        &filtered
    };
    let index = byte_index_for_char(value, *cursor);
    value.insert_str(index, text);
    *cursor += characters;
    true
}
