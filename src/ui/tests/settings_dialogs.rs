use super::*;

#[test]
fn unsaved_settings_dialog_renders_centered_action_labels() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .draft_mut_for_tests()
        .recording
        .start_record_on_launch = false;
    app.handle_key_event(key(KeyCode::Esc));

    let (_ui, buffer) = render_to_buffer(&mut app);
    let rendered = (0..buffer.area.height)
        .map(|row| buffer_row(&buffer, row, 0, buffer.area.width))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("Unsaved Settings"));
    assert!(rendered.contains("Save [Enter]"));
    assert!(rendered.contains("Discard [Esc]"));
}

#[test]
fn unsaved_settings_dialog_suppresses_parent_footer_key_hints() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .draft_mut_for_tests()
        .recording
        .start_record_on_launch = false;
    app.handle_key_event(key(KeyCode::Esc));

    let (_ui, buffer) = render_to_buffer(&mut app);
    let footer = settings_popup_footer_row(&buffer);

    assert!(!footer.contains("Save [s]"), "{footer}");
    assert!(!footer.contains("Close [Esc]"), "{footer}");
}

#[test]
fn unsaved_settings_dialog_renders_bordered_buttons_at_bottom() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .draft_mut_for_tests()
        .recording
        .start_record_on_launch = false;
    app.handle_key_event(key(KeyCode::Esc));

    let (_ui, buffer) = render_to_buffer(&mut app);
    let dialog_area = action_dialog_test_area(buffer.area, &app);
    let rendered = (dialog_area.y..dialog_area.bottom())
        .map(|row| buffer_row(&buffer, row, dialog_area.x, dialog_area.width))
        .collect::<Vec<_>>()
        .join("\n");
    let save = find_buffer_text(&buffer, dialog_area, "Save [Enter]")
        .expect("save button label should render");
    let discard = find_buffer_text(&buffer, dialog_area, "Discard [Esc]")
        .expect("discard button label should render");

    assert!(!rendered.contains("[ Save [Enter] ]"));
    assert_eq!(save.y, dialog_area.bottom().saturating_sub(3));
    assert_eq!(discard.y, save.y);
    assert_eq!(buffer[(save.x.saturating_sub(2), save.y - 1)].symbol(), "╭");
    assert_eq!(
        buffer[(discard.x.saturating_sub(2), discard.y - 1)].symbol(),
        "╭"
    );
    assert!(discard.x > save.x + text_width("Save [Enter]") + 4);
    assert_ne!(buffer[save].bg, Color::Green);
}

#[test]
fn unsaved_settings_dialog_centers_message_above_buttons() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .draft_mut_for_tests()
        .recording
        .start_record_on_launch = false;
    app.handle_key_event(key(KeyCode::Esc));

    let (_ui, buffer) = render_to_buffer(&mut app);
    let dialog_area = action_dialog_test_area(buffer.area, &app);
    let message = find_buffer_text(&buffer, dialog_area, "You have unsaved setting changes.")
        .expect("message should render");

    assert_eq!(message.y, dialog_area.y + 3);
}

#[test]
fn unsaved_settings_dialog_buttons_do_not_overwrite_narrow_dialog_border() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .draft_mut_for_tests()
        .recording
        .start_record_on_launch = false;
    app.handle_key_event(key(KeyCode::Esc));

    let (_ui, buffer) = render_to_buffer_with_size(&mut app, 36, 12);
    let dialog_area = action_dialog_test_area(buffer.area, &app);
    let button_label_row = dialog_area.bottom().saturating_sub(3);

    assert_eq!(
        buffer[(dialog_area.right() - 1, button_label_row)].symbol(),
        "│"
    );
}
