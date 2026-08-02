use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use edtui::{
    EditorEventHandler, EditorMode,
    actions::motion::{MoveToFirstRow, MoveToLastRow},
    actions::search::StartSearch,
    actions::{
        Action, CopyLine, CopySelection, FindNext, FindPrevious, MoveBackward, MoveDown,
        MoveForward, MoveHalfPageDown, MoveHalfPageUp, MoveToEndOfLine, MoveToFirst,
        MoveToMatchinBracket, MoveToStartOfLine, MoveUp, MoveWordBackward, MoveWordForward,
        MoveWordForwardToEndOfWord, RemoveCharFromSearch, SelectInnerBetween, SelectInnerWord,
        SelectLine, SwitchMode, TriggerSearch,
    },
    events::{KeyEvent as EditorKeyEvent, KeyEventHandler, KeyEventRegister},
};
use std::collections::HashMap;

pub(super) fn read_only_event_handler() -> EditorEventHandler {
    EditorEventHandler::new(KeyEventHandler::new(read_only_keymap()))
}

fn read_only_keymap() -> HashMap<KeyEventRegister, Action> {
    let mut keymap = HashMap::new();

    bind(
        &mut keymap,
        KeyEventRegister::n([EditorKeyEvent::Char('/')]),
        StartSearch,
    );
    bind(
        &mut keymap,
        KeyEventRegister::s([EditorKeyEvent::Enter]),
        TriggerSearch,
    );
    bind(
        &mut keymap,
        KeyEventRegister::s([EditorKeyEvent::Backspace]),
        RemoveCharFromSearch,
    );
    bind(
        &mut keymap,
        KeyEventRegister::n([EditorKeyEvent::Char('n')]),
        FindNext,
    );
    bind(
        &mut keymap,
        KeyEventRegister::n([EditorKeyEvent::Char('N')]),
        FindPrevious,
    );

    bind_normal_and_visual(&mut keymap, EditorKeyEvent::Char('h'), MoveBackward(1));
    bind_normal_and_visual(&mut keymap, EditorKeyEvent::Left, MoveBackward(1));
    bind_normal_and_visual(&mut keymap, EditorKeyEvent::Char('l'), MoveForward(1));
    bind_normal_and_visual(&mut keymap, EditorKeyEvent::Right, MoveForward(1));
    bind_normal_and_visual(&mut keymap, EditorKeyEvent::Char('k'), MoveUp(1));
    bind_normal_and_visual(&mut keymap, EditorKeyEvent::Up, MoveUp(1));
    bind_normal_and_visual(&mut keymap, EditorKeyEvent::Char('j'), MoveDown(1));
    bind_normal_and_visual(&mut keymap, EditorKeyEvent::Down, MoveDown(1));
    bind_normal_and_visual(&mut keymap, EditorKeyEvent::Char('w'), MoveWordForward(1));
    bind_normal_and_visual(
        &mut keymap,
        EditorKeyEvent::Char('e'),
        MoveWordForwardToEndOfWord(1),
    );
    bind_normal_and_visual(&mut keymap, EditorKeyEvent::Char('b'), MoveWordBackward(1));
    bind_normal_and_visual(&mut keymap, EditorKeyEvent::Char('0'), MoveToStartOfLine());
    bind_normal_and_visual(&mut keymap, EditorKeyEvent::Home, MoveToStartOfLine());
    bind_normal_and_visual(&mut keymap, EditorKeyEvent::Char('_'), MoveToFirst());
    bind_normal_and_visual(&mut keymap, EditorKeyEvent::Char('$'), MoveToEndOfLine());
    bind_normal_and_visual(&mut keymap, EditorKeyEvent::End, MoveToEndOfLine());
    bind_normal_and_visual(&mut keymap, EditorKeyEvent::Ctrl('d'), MoveHalfPageDown());
    bind_normal_and_visual(&mut keymap, EditorKeyEvent::Ctrl('u'), MoveHalfPageUp());
    bind_normal_and_visual(&mut keymap, EditorKeyEvent::Char('G'), MoveToLastRow());
    bind_normal_and_visual(
        &mut keymap,
        EditorKeyEvent::Char('%'),
        MoveToMatchinBracket(),
    );
    bind(
        &mut keymap,
        KeyEventRegister::n([EditorKeyEvent::Char('g'), EditorKeyEvent::Char('g')]),
        MoveToFirstRow(),
    );
    bind(
        &mut keymap,
        KeyEventRegister::v([EditorKeyEvent::Char('g'), EditorKeyEvent::Char('g')]),
        MoveToFirstRow(),
    );

    bind(
        &mut keymap,
        KeyEventRegister::n([EditorKeyEvent::Char('v')]),
        SwitchMode(EditorMode::Visual),
    );
    bind(
        &mut keymap,
        KeyEventRegister::n([EditorKeyEvent::Char('V')]),
        SelectLine,
    );
    bind(
        &mut keymap,
        KeyEventRegister::v([EditorKeyEvent::Char('i'), EditorKeyEvent::Char('w')]),
        SelectInnerWord,
    );
    for (opening, closing) in [
        ('"', '"'),
        ('\'', '\''),
        ('(', ')'),
        (')', ')'),
        ('{', '}'),
        ('}', '}'),
        ('[', ']'),
        (']', ']'),
    ] {
        bind(
            &mut keymap,
            KeyEventRegister::v([EditorKeyEvent::Char('i'), EditorKeyEvent::Char(opening)]),
            SelectInnerBetween::new(opening, closing),
        );
    }
    bind(
        &mut keymap,
        KeyEventRegister::v([EditorKeyEvent::Char('y')]),
        CopySelection,
    );
    bind(
        &mut keymap,
        KeyEventRegister::n([EditorKeyEvent::Char('y'), EditorKeyEvent::Char('y')]),
        CopyLine,
    );

    keymap
}

fn bind<A>(keymap: &mut HashMap<KeyEventRegister, Action>, key: KeyEventRegister, action: A)
where
    A: Into<Action>,
{
    keymap.insert(key, action.into());
}

fn bind_normal_and_visual<A>(
    keymap: &mut HashMap<KeyEventRegister, Action>,
    key: EditorKeyEvent,
    action: A,
) where
    A: Into<Action> + Clone,
{
    bind(keymap, KeyEventRegister::n([key]), action.clone());
    bind(keymap, KeyEventRegister::v([key]), action);
}

pub(super) fn editor_key_event(key: KeyEvent) -> Option<EditorKeyEvent> {
    if key.modifiers == KeyModifiers::CONTROL {
        return match key.code {
            KeyCode::Char(ch) => Some(EditorKeyEvent::Ctrl(ch)),
            KeyCode::PageDown => Some(EditorKeyEvent::Ctrl('d')),
            KeyCode::PageUp => Some(EditorKeyEvent::Ctrl('u')),
            _ => None,
        };
    }

    match key.code {
        KeyCode::Char(ch) if is_plain_key(key) => Some(EditorKeyEvent::Char(ch)),
        KeyCode::Enter if is_plain_key(key) => Some(EditorKeyEvent::Enter),
        KeyCode::Down if is_plain_key(key) => Some(EditorKeyEvent::Down),
        KeyCode::Up if is_plain_key(key) => Some(EditorKeyEvent::Up),
        KeyCode::Right if is_plain_key(key) => Some(EditorKeyEvent::Right),
        KeyCode::Left if is_plain_key(key) => Some(EditorKeyEvent::Left),
        KeyCode::Backspace if is_plain_key(key) => Some(EditorKeyEvent::Backspace),
        KeyCode::Home if is_plain_key(key) => Some(EditorKeyEvent::Home),
        KeyCode::End if is_plain_key(key) => Some(EditorKeyEvent::End),
        KeyCode::PageDown if is_plain_key(key) => Some(EditorKeyEvent::Ctrl('d')),
        KeyCode::PageUp if is_plain_key(key) => Some(EditorKeyEvent::Ctrl('u')),
        _ => None,
    }
}

pub(super) fn plain_char(key: KeyEvent) -> Option<char> {
    if !is_plain_key(key) {
        return None;
    }

    match key.code {
        KeyCode::Char(ch) => Some(ch),
        _ => None,
    }
}

pub(super) fn is_plain_key(key: KeyEvent) -> bool {
    key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT
}
