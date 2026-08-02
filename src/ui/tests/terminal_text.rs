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
